//! Executable runtime stand-ins; never intended to run as production daemons.
use light_bridge_core::*;
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::{
    ipc::{self, Notification},
    network,
};
use light_bridge_usbip::MockUsbIp;
use network_session::{MonadoAdapter, WivrnClientAdapter, WivrnServerAdapter};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(5);
fn announce(text: impl std::fmt::Display) {
    println!("{text}");
    std::io::stdout().flush().unwrap();
}
pub fn host() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = EndpointInfo {
        addresses: vec![listener.local_addr().unwrap()],
        host_id: 17,
        correlation_id: 42,
    };
    announce(endpoint.addresses[0]); // Stand-in advertisement, not a WiVRn packet.
    let transport = network::accept(&listener, &endpoint, TIMEOUT).unwrap();
    let usb = Arc::new(Mutex::new(MockUsbIp::remote_importer()));
    let mut server = WivrnServerAdapter {
        bridge: HostServer::new(transport, usb.clone(), TIMEOUT),
        tracking_handles_open: false,
        tracking_initializations: 0,
    };
    server.bridge.start(42).unwrap();
    server.prepare_before_enumeration().unwrap();
    let end = Instant::now() + TIMEOUT;
    let mut sequence = 0;
    loop {
        server.bridge.poll().unwrap();
        if server.bridge.state() == State::TrackingInitializing {
            server.enumerate_after_attach().unwrap();
        }
        if server.bridge.needs_runtime_release() {
            server.release_and_stop().unwrap();
        }
        if server.bridge.state() == State::Active && !server.bridge.needs_runtime_release() {
            sequence += 1;
            server
                .bridge
                .publish_pose(ExternalPose::mock(42, sequence, 999_900 + sequence as i64))
                .unwrap();
        }
        if server.bridge.state() == State::Idle {
            break;
        }
        assert!(Instant::now() < end, "host lifecycle timeout");
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(usb.lock().unwrap().resource_count(), 0);
    announce("HOST resources=0");
    // Wait for the parent to acknowledge completion before closing the TCP sockets.
    let mut ack = String::new();
    std::io::stdin().read_line(&mut ack).unwrap();
    assert_eq!(ack.trim(), "done");
}
pub fn hmd(path: &Path) {
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    announce("READY");
    let end = Instant::now() + TIMEOUT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("{e}"),
        }
        assert!(Instant::now() < end, "IPC accept timeout");
        thread::sleep(Duration::from_millis(1));
    };
    let endpoint = match ipc::receive(&mut stream, TIMEOUT).unwrap() {
        Notification::Start(endpoint) => endpoint,
        _ => panic!("expected Start"),
    };
    let transport = network::connect(&endpoint, TIMEOUT).unwrap();
    let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
    let mut monado = MonadoAdapter::from_connected(
        HmdClient::new(transport, usb.clone(), TIMEOUT),
        endpoint.host_id,
    );
    monado.notification(Notification::Start(endpoint)).unwrap();
    monado
        .bridge
        .set_mock_clock(
            1_000_000,
            ClockMapping {
                source_clock_id: 1,
                offset_ns: 100,
                uncertainty_ns: 10,
            },
        )
        .unwrap();
    loop {
        monado.poll().unwrap();
        if monado.reference.is_some() {
            break;
        }
        assert!(Instant::now() < end, "pose timeout");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(monado.reference.unwrap().position_m, [1., 2., 3.]);
    assert!(!monado.head_pose_valid);
    announce("POSE reference=[1,2,3] head_valid=false");
    let stop = ipc::receive(&mut stream, TIMEOUT).unwrap();
    monado.notification(stop).unwrap();
    while monado.bridge.state() != State::Idle {
        monado.poll().unwrap();
        assert!(Instant::now() < end, "stop timeout");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(usb.lock().unwrap().resource_count(), 0);
    announce("HMD resources=0");
    let mut ack = String::new();
    std::io::stdin().read_line(&mut ack).unwrap();
    assert_eq!(ack.trim(), "done");
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct PrivateDirectory(PathBuf);
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("notification.sock"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
pub fn run() {
    let path = std::env::temp_dir().join(format!(
        "llb-network-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // Atomically create our own directory with restricted permissions.
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let directory = PrivateDirectory(path);
    assert_eq!(
        std::fs::metadata(&directory.0)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let socket = directory.0.join("notification.sock");
    let exe = std::env::current_exe().unwrap();
    let spawn = |args: &[&std::ffi::OsStr]| {
        ChildGuard(
            Command::new(&exe)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    };
    let mut host = spawn(&["--host".as_ref()]);
    let mut host_output = BufReader::new(host.0.stdout.take().unwrap());
    let mut address = String::new();
    host_output.read_line(&mut address).unwrap();
    let endpoint = EndpointInfo {
        addresses: vec![address.trim().parse().unwrap()],
        host_id: 17,
        correlation_id: 42,
    };
    let mut hmd = spawn(&["--hmd".as_ref(), socket.as_os_str()]);
    let mut hmd_output = BufReader::new(hmd.0.stdout.take().unwrap());
    let mut line = String::new();
    hmd_output.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "READY");
    let mut ipc = UnixStream::connect(&socket).unwrap();
    WivrnClientAdapter::connected(&mut ipc, endpoint.clone()).unwrap();
    line.clear();
    hmd_output.read_line(&mut line).unwrap();
    assert!(line.starts_with("POSE "));
    print!("{line}");
    WivrnClientAdapter::disconnected(&mut ipc, &endpoint).unwrap();
    line.clear();
    hmd_output.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "HMD resources=0");
    print!("{line}");
    line.clear();
    host_output.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "HOST resources=0");
    print!("{line}");
    host.0.stdin.as_mut().unwrap().write_all(b"done\n").unwrap();
    hmd.0.stdin.as_mut().unwrap().write_all(b"done\n").unwrap();
    assert!(host.0.wait().unwrap().success());
    assert!(hmd.0.wait().unwrap().success());
    println!("Separate processes: WiVRn Client IPC -> HMD TCP -> Host prepare/attach -> Monado reference -> stop: OK (Mock USB/runtime)");
}
