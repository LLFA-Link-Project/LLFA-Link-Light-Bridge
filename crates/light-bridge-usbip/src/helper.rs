//! Fixed-schema local privileged helper. Runtime clients never run a root shell.
use crate::{
    kernel::{Engine, Journal, KResult, KernelOps, Policy, Role},
    UsbDeviceIdentity, UsbTransferBackend,
};
use light_bridge_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Debug, Serialize, Deserialize)]
pub enum Action {
    Describe,
    Shutdown,
    Acquire,
    Release,
    Inspect,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub compatibility_id: String,
    pub instance: u64,
    pub session: u64,
    pub action: Action,
    pub device: UsbDeviceIdentity,
    pub runtime_released: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub identity: HelperIdentity,
    pub result: KResult<Option<u32>>,
    pub pending: bool,
}
pub const RPC_VERSION: u32 = 2;
/// Bump whenever the privileged helper contract or required behavior changes.
pub const COMPATIBILITY_ID: &str = "llfa-usbip-helper-2-20261012";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperIdentity {
    pub compatibility_id: String,
    pub package_version: String,
    pub build_revision: String,
    pub policy: Policy,
}
impl HelperIdentity {
    pub fn current(policy: &Policy) -> Self {
        Self {
            compatibility_id: COMPATIBILITY_ID.into(),
            package_version: env!("CARGO_PKG_VERSION").into(),
            build_revision: option_env!("LLB_BUILD_REVISION")
                .unwrap_or("unspecified")
                .into(),
            policy: policy.clone(),
        }
    }
}
pub fn validate_response(response: &Response, policy: &Policy) -> KResult<()> {
    if response.version != RPC_VERSION
        || response.identity.compatibility_id != COMPATIBILITY_ID
        || response.identity.package_version != env!("CARGO_PKG_VERSION")
    {
        return Err(format!(
            "helper update required: expected {COMPATIBILITY_ID}/{} RPC {RPC_VERSION}; actual {}/{} RPC {} build {}",
            env!("CARGO_PKG_VERSION"), response.identity.compatibility_id,
            response.identity.package_version, response.version, response.identity.build_revision
        ));
    }
    if &response.identity.policy != policy {
        return Err("helper policy differs from runtime policy".into());
    }
    Ok(())
}
pub fn describe_request(policy: &Policy) -> Request {
    Request {
        version: RPC_VERSION,
        compatibility_id: COMPATIBILITY_ID.into(),
        instance: 1,
        session: 0,
        action: Action::Describe,
        device: policy.device.clone(),
        runtime_released: false,
    }
}
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut n = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the live socket fd and correctly sized ucred buffer are valid.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut n,
        )
    };
    if rc != 0 || n as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}
fn transfer(
    stream: &mut UnixStream,
    b: &mut [u8],
    write: bool,
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<()> {
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut offset = 0;
    while offset < b.len() {
        if cancel.load(Ordering::Acquire) || Instant::now() >= end {
            return Err("helper cancelled/timeout".into());
        }
        match if write {
            stream.write(&b[offset..])
        } else {
            stream.read(&mut b[offset..])
        } {
            Ok(0) => return Err("helper disconnected".into()),
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
pub fn send<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<()> {
    let b = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if b.len() > 4096 {
        return Err("helper packet too large".into());
    }
    let mut frame = (b.len() as u32).to_be_bytes().to_vec();
    frame.extend(b);
    transfer(stream, &mut frame, true, end, cancel)
}
pub fn receive<T: for<'a> Deserialize<'a>>(
    stream: &mut UnixStream,
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<T> {
    let mut prefix = [0; 4];
    transfer(stream, &mut prefix, false, end, cancel)?;
    let n = u32::from_be_bytes(prefix) as usize;
    if n == 0 || n > 4096 {
        return Err("helper packet size invalid".into());
    }
    let mut b = vec![0; n];
    transfer(stream, &mut b, false, end, cancel)?;
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}
pub fn dispatch<O: KernelOps, J: Journal>(
    engine: &mut Engine<O, J>,
    uid: u32,
    r: Request,
) -> Response {
    let result = (|| {
        if (uid != engine.policy.client_uid && !(uid == 0 && matches!(r.action, Action::Describe)))
            || r.version != RPC_VERSION
            || r.compatibility_id != COMPATIBILITY_ID
            || r.instance == 0
            || (r.session == 0 && !matches!(r.action, Action::Describe | Action::Shutdown))
            || r.device != engine.policy.device
        {
            return Err("peer/version/owner/device denied".into());
        }
        match r.action {
            Action::Describe => Ok(None),
            Action::Shutdown => {
                if !r.runtime_released || engine.record().is_some() {
                    Err(
                        "shutdown denied: runtime release and empty ownership journal required"
                            .into(),
                    )
                } else {
                    Ok(None)
                }
            }
            Action::Acquire => engine.acquire(r.instance, r.session),
            Action::Release => engine
                .release(r.instance, r.session, r.runtime_released)
                .map(|_| None),
            Action::Inspect => Ok(engine
                .record()
                .and_then(|r| r.port.as_ref().map(|p| p.number))),
        }
    })();
    let result = result.map_err(|mut error: String| {
        if error.len() > 2000 {
            let mut n = 2000;
            while !error.is_char_boundary(n) {
                n -= 1;
            }
            error.truncate(n);
            error.push_str(" [truncated]");
        }
        error
    });
    Response {
        version: RPC_VERSION,
        identity: HelperIdentity::current(&engine.policy),
        result,
        pending: engine.record().is_some(),
    }
}
/// Validate a root-owned policy file; command/executable/socket/journal paths are not request fields.
pub fn load_policy(path: &Path) -> KResult<Policy> {
    crate::command::trusted(path).map_err(|e| e.to_string())?;
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    if b.len() > 4096 {
        return Err("policy too large".into());
    }
    let p: Policy = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    p.validate()?;
    Ok(p)
}
fn connect_cancelable(path: &Path, cancel: &AtomicBool) -> io::Result<UnixStream> {
    use std::os::{
        fd::{FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is a plain C address structure with a valid zero initialization.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "UDS pathname"));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= end {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "helper connect cancelled/timeout",
            ));
        }
        // SAFETY: socket arguments valid; ownership transfers to OwnedFd exactly once.
        let raw = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1)
            as libc::socklen_t;
        // SAFETY: properly sized address lives throughout connect; fd remains owned.
        if unsafe { libc::connect(raw, (&address as *const libc::sockaddr_un).cast(), len) } == 0 {
            return Ok(UnixStream::from(fd));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        drop(fd);
        thread::sleep(Duration::from_millis(1));
    }
}
pub fn trusted_connection(path: &Path, cancel: &AtomicBool) -> KResult<UnixStream> {
    use std::os::unix::fs::FileTypeExt;
    // Validate both pathname ancestors and the actual peer. Do not follow socket symlinks.
    crate::command::trusted(path.parent().ok_or("socket parent missing")?)
        .map_err(|e| e.to_string())?;
    let m = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.file_type().is_socket() || m.uid() != 0 || m.mode() & 0o007 != 0 {
        return Err("helper socket requires root ownership and no other-user access".into());
    }
    let stream = connect_cancelable(path, cancel).map_err(|e| e.to_string())?;
    if peer_uid(&stream).map_err(|e| e.to_string())? != 0 {
        return Err("helper peer is not root".into());
    }
    Ok(stream)
}
/// Non-mutating handshake. Never interprets timeout/refusal as an absent installation.
pub fn probe_helper(path: &Path, policy: &Policy) -> KResult<Response> {
    let cancel = AtomicBool::new(false);
    let mut stream = trusted_connection(path, &cancel)?;
    let end = Instant::now() + Duration::from_secs(3);
    send(&mut stream, &describe_request(policy), end, &cancel)?;
    let response: Response = receive(&mut stream, end, &cancel)
        .map_err(|e| format!("helper handshake failed; update or diagnose helper: {e}"))?;
    validate_response(&response, policy)?;
    response.result.clone()?;
    Ok(response)
}
fn verified_exchange(
    stream: &mut UnixStream,
    policy: &Policy,
    request: &Request,
    cancel: &AtomicBool,
) -> KResult<Response> {
    let end = Instant::now() + Duration::from_secs(10);
    send(stream, &describe_request(policy), end, cancel)?;
    let hello: Response = receive(stream, end, cancel)?;
    validate_response(&hello, policy)?;
    hello.result?;
    send(stream, request, end, cancel)?;
    let response: Response = receive(stream, end, cancel)?;
    validate_response(&response, policy)?;
    Ok(response)
}
/// In-process USB backend: fixed helper socket + expected policy, independent session ownership.
/// Blocking methods are meant for UsbManager control workers, never a pose query/callback.
pub struct KernelUsbIp {
    pub policy: Policy,
    pub instance: u64,
    socket: PathBuf,
    cancel: Arc<AtomicBool>,
}
impl KernelUsbIp {
    pub fn new(socket: impl Into<PathBuf>, policy: Policy, instance: u64) -> Result<Self> {
        policy.validate().map_err(|_| Error::DeviceDenied)?;
        if instance == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self {
            policy,
            instance,
            socket: socket.into(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn set_cancel(&mut self, cancel: Arc<AtomicBool>) {
        self.cancel = cancel;
    }
    pub fn preflight(&self) -> Result<()> {
        let response = probe_helper(&self.socket, &self.policy).map_err(|e| {
            eprintln!("USB/IP helper preflight: {e}");
            Error::Protocol
        })?;
        if response.pending {
            return Err(Error::Ownership);
        }
        Ok(())
    }
    fn request(
        &mut self,
        session: u64,
        device: &UsbDeviceIdentity,
        action: Action,
        released: bool,
    ) -> Result<Option<u32>> {
        if device != &self.policy.device {
            return Err(Error::DeviceDenied);
        }
        let mut stream =
            trusted_connection(&self.socket, &self.cancel).map_err(|_| Error::Ownership)?;
        let r = Request {
            version: RPC_VERSION,
            compatibility_id: COMPATIBILITY_ID.into(),
            instance: self.instance,
            session,
            action,
            device: device.clone(),
            runtime_released: released,
        };
        // Same connection for handshake and mutation: a new peer must pass again.
        let response =
            verified_exchange(&mut stream, &self.policy, &r, &self.cancel).map_err(|e| {
                eprintln!("USB/IP helper handshake: {e}");
                Error::Protocol
            })?;
        response.result.map_err(|e| {
            eprintln!("USB/IP helper: {e}");
            if self.policy.role == Role::Export {
                Error::UsbExport
            } else {
                Error::UsbAttach
            }
        })
    }
}
impl UsbTransferBackend for KernelUsbIp {
    fn export(&mut self, session: u64, d: &UsbDeviceIdentity) -> Result<()> {
        if self.policy.role != Role::Export {
            return Err(Error::InvalidState);
        }
        self.request(session, d, Action::Acquire, false).map(|_| ())
    }
    fn attach(&mut self, session: u64, d: &UsbDeviceIdentity) -> Result<u32> {
        if self.policy.role != Role::Import {
            return Err(Error::InvalidState);
        }
        self.request(session, d, Action::Acquire, false)?
            .ok_or(Error::UsbAttach)
    }
    fn detach(&mut self, session: u64, port: u32) -> Result<()> {
        if self.policy.role != Role::Import {
            return Err(Error::InvalidState);
        }
        let d = self.policy.device.clone();
        let observed = self.request(session, &d, Action::Inspect, false)?;
        if observed != Some(port) {
            return Err(Error::Ownership);
        }
        self.request(session, &d, Action::Release, true).map(|_| ())
    }
    fn unexport(&mut self, session: u64) -> Result<()> {
        if self.policy.role != Role::Export {
            return Err(Error::InvalidState);
        }
        let d = self.policy.device.clone();
        self.request(session, &d, Action::Release, true).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incompatible_peer_receives_no_mutation_on_the_verified_connection() {
        let mut device = UsbDeviceIdentity::mock();
        device.busid = "1-3.3".into();
        device.sysfs_path = "/sys/devices/mock/1-3.3".into();
        let policy = Policy {
            role: Role::Export,
            client_uid: 1000,
            client_gid: 1000,
            device,
            remote: "192.0.2.1:3240".parse().unwrap(),
        };
        for compatible in [false, true] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let p = policy.clone();
            let worker = thread::spawn(move || {
                let cancel = AtomicBool::new(false);
                let end = Instant::now() + Duration::from_secs(1);
                let describe: Request = receive(&mut server, end, &cancel).unwrap();
                assert!(matches!(describe.action, Action::Describe));
                let mut hello = Response {
                    version: RPC_VERSION,
                    identity: HelperIdentity::current(&p),
                    result: Ok(None),
                    pending: false,
                };
                if !compatible {
                    hello.identity.compatibility_id = "incompatible".into();
                }
                send(&mut server, &hello, end, &cancel).unwrap();
                let next = receive::<Request>(&mut server, end, &cancel);
                if compatible {
                    assert!(matches!(next.unwrap().action, Action::Acquire));
                    send(&mut server, &hello, end, &cancel).unwrap();
                } else {
                    assert!(next.is_err());
                }
            });
            let mut acquire = describe_request(&policy);
            acquire.action = Action::Acquire;
            acquire.session = 1;
            let result = verified_exchange(&mut client, &policy, &acquire, &AtomicBool::new(false));
            assert_eq!(result.is_ok(), compatible);
            drop(client);
            worker.join().unwrap();
        }
    }
    #[test]
    fn identity_and_exact_policy_are_required_even_with_equal_package_versions() {
        let mut device = UsbDeviceIdentity::mock();
        device.busid = "1-3.3".into();
        device.sysfs_path = "/sys/devices/mock/1-3.3".into();
        let policy = Policy {
            role: Role::Export,
            client_uid: 1000,
            client_gid: 1000,
            device,
            remote: "192.0.2.1:3240".parse().unwrap(),
        };
        let mut response = Response {
            version: RPC_VERSION,
            identity: HelperIdentity::current(&policy),
            result: Ok(None),
            pending: false,
        };
        validate_response(&response, &policy).unwrap();
        response.identity.compatibility_id = "old".into();
        assert!(validate_response(&response, &policy)
            .unwrap_err()
            .contains("update required"));
        response.identity = HelperIdentity::current(&policy);
        response.identity.policy.client_uid += 1;
        assert!(validate_response(&response, &policy)
            .unwrap_err()
            .contains("policy"));
        response.identity = HelperIdentity::current(&policy);
        response.version = 1;
        assert!(validate_response(&response, &policy).is_err());
    }
    #[test]
    fn peer_credentials_packet_limits_and_cancel() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), unsafe { libc::geteuid() });
        b.write_all(&u32::MAX.to_be_bytes()).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(
            receive::<Response>(&mut a, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap_err()
                .contains("size")
        );
        cancel.store(true, Ordering::Release);
        assert!(
            receive::<Response>(&mut a, Instant::now() + Duration::from_secs(1), &cancel)
                .unwrap_err()
                .contains("cancel")
        );
    }
    #[test]
    fn runtime_proxy_rejects_an_unprivileged_lookalike_helper() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::net::UnixListener;
        let path = std::env::temp_dir().join(format!(
            "llb-lookalike-{}-{}.sock",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let mut device = UsbDeviceIdentity::mock();
        device.busid = "1-3.3".into();
        device.sysfs_path = "/sys/devices/mock/1-3.3".into();
        let policy = Policy {
            role: Role::Export,
            client_uid: unsafe { libc::geteuid() },
            client_gid: unsafe { libc::getegid() },
            device: device.clone(),
            remote: "192.0.2.1:3240".parse().unwrap(),
        };
        let mut client = KernelUsbIp::new(&path, policy, 9).unwrap();
        assert_eq!(client.export(1, &device), Err(Error::Ownership));
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }
}
