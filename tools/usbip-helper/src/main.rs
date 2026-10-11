#[cfg(target_os = "linux")]
fn main() {
    use light_bridge_usbip::{
        helper::{self, Request},
        kernel::{Engine, FileJournal, LinuxOps, Role},
    };
    use std::{
        fs,
        os::{
            fd::AsRawFd,
            unix::{
                fs::{DirBuilderExt, MetadataExt, PermissionsExt},
                net::UnixListener,
            },
        },
        path::PathBuf,
        sync::{atomic::AtomicBool, Arc},
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "serve" {
        eprintln!(
            "Usage: usbip-helper serve /etc/llfa-link-light-bridge/export.json (or import.json)"
        );
        std::process::exit(2);
    }
    // SAFETY: geteuid has no pointer arguments or side effects.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("helper requires root; runtime libraries do not");
        std::process::exit(1);
    }
    let policy = helper::load_policy(std::path::Path::new(&args[1])).expect("trusted exact policy");
    let role = if policy.role == Role::Export {
        "export"
    } else {
        "import"
    };
    let root = PathBuf::from("/var/lib/llfa-link-light-bridge");
    for path in [&root, &PathBuf::from("/run/llfa-link-light-bridge")] {
        let mode = if path == &root { 0o700 } else { 0o711 };
        match fs::DirBuilder::new().mode(mode).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => panic!("{e}"),
        }
        let m = fs::symlink_metadata(path).unwrap();
        assert!(
            m.is_dir() && m.uid() == 0 && m.mode() & 0o777 == mode,
            "private root directory required"
        );
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(format!("{role}.lock")))
        .unwrap();
    // SAFETY: live file fd, nonblocking exclusive lock serializes our helper instance.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "helper already owns role"
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let remote = policy.remote;
    let mut engine = Engine::new(
        policy,
        LinuxOps {
            cancel: cancel.clone(),
            remote,
        },
        FileJournal {
            path: root.join(format!("{role}.json")),
        },
    )
    .expect("journal/policy");
    if let Some(r) = engine.record() {
        eprintln!("Recovery inspection required; no automatic adoption: {r:?}");
    }
    let socket = PathBuf::from(format!("/run/llfa-link-light-bridge/{role}.sock"));
    // Do not unlink a pre-existing path; operator removes only the stale socket after auditing.
    let listener = UnixListener::bind(&socket).expect("helper socket path must be unused");
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    let socket_c = std::ffi::CString::new(socket.as_os_str().as_bytes()).unwrap();
    // SAFETY: CString owns a NUL-terminated pathname; root selects policy-controlled gid.
    assert_eq!(
        unsafe { libc::chown(socket_c.as_ptr(), 0, engine.policy.client_gid) },
        0
    );
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
    eprintln!(
        "Listening: {} (authorized UID {})",
        socket.display(),
        engine.policy.client_uid
    );
    for accepted in listener.incoming() {
        let mut stream = match accepted {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept: {e}");
                continue;
            }
        };
        let uid = match helper::peer_uid(&stream) {
            Ok(uid) if uid == engine.policy.client_uid => uid,
            _ => continue,
        };
        let end = Instant::now() + Duration::from_secs(12);
        let request: Request = match helper::receive(&mut stream, end, &cancel) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("request: {e}");
                continue;
            }
        };
        let response = helper::dispatch(&mut engine, uid, request);
        if let Err(e) = &response.result {
            eprintln!("operation: {e}");
        }
        let _ = helper::send(&mut stream, &response, end, &cancel);
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("USB/IP kernel helper requires Linux");
    std::process::exit(1);
}
