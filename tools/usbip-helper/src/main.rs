#[cfg(target_os = "linux")]
mod auto_server;
#[cfg(target_os = "linux")]
mod temporary;
#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = entry() {
        eprintln!("usbip-helper: {e}");
        std::process::exit(1);
    }
}
#[cfg(target_os = "linux")]
fn entry() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|v| v.to_str()) {
        Some("temporary-start-auto") => temporary::start_auto(&args[1..]),
        Some("check-auto") if args.len()==2 => temporary::check_auto(&args[1]),
        Some("temporary-stop-auto") if args.len()==3 && args[2]=="--runtime-stopped" => temporary::stop_auto(&args[1]),
        Some("serve-auto") if args.len()==2 => auto_server::serve(std::path::Path::new(&args[1])),
        Some("temporary-start") if args.len() == 3 && args[2] == "--acknowledge-development-build" => {
            temporary::start(std::path::Path::new(&args[1]))
        }
        Some("check") if args.len() == 2 => temporary::check(std::path::Path::new(&args[1])),
        Some("temporary-stop") if args.len() == 3 && args[2] == "--runtime-stopped" => {
            temporary::stop(std::path::Path::new(&args[1]))
        }
        Some("serve" | "serve-temporary") if args.len() == 2 => {
            serve(std::path::Path::new(&args[1]), args[0] == "serve-temporary");
            Ok(())
        }
        Some("--version") if args.len() == 1 => {
            println!("usbip-helper {} compatibility={} RPC={} build={}", env!("CARGO_PKG_VERSION"),
                light_bridge_usbip::helper::COMPATIBILITY_ID, light_bridge_usbip::helper::RPC_VERSION,
                option_env!("LLB_BUILD_REVISION").unwrap_or("unspecified"));
            Ok(())
        }
        _ => Err("Usage: usbip-helper check POLICY | temporary-start POLICY --acknowledge-development-build (sudo) | temporary-stop POLICY --runtime-stopped | serve ROOT_POLICY | --version".into()),
    }
}
#[cfg(target_os = "linux")]
fn serve(policy_path: &std::path::Path, temporary: bool) {
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
    // SAFETY: geteuid has no pointer arguments or side effects.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("helper requires root; runtime libraries do not");
        std::process::exit(1);
    }
    let policy = helper::load_policy(policy_path).expect("trusted exact policy");
    let role = if policy.role == Role::Export {
        "export"
    } else {
        "import"
    };
    if temporary {
        assert_eq!(
            policy_path,
            temporary::stage(engine_role(role)).join("policy.json")
        );
    }
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
    let socket_identity = fs::symlink_metadata(&socket).unwrap();
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
            Ok(uid) if uid == engine.policy.client_uid || uid == 0 => uid,
            _ => continue,
        };
        let end = Instant::now() + Duration::from_secs(12);
        for _ in 0..2 {
            let request: Request = match helper::receive(&mut stream, end, &cancel) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("request: {e}");
                    break;
                }
            };
            let shutdown = matches!(request.action, helper::Action::Shutdown);
            let describe = matches!(request.action, helper::Action::Describe);
            if shutdown && !temporary {
                eprintln!("shutdown of an installed helper is not a runtime RPC");
                break;
            }
            let response = helper::dispatch(&mut engine, uid, request);
            if let Err(e) = &response.result {
                eprintln!("operation: {e}");
            }
            let _ = helper::send(&mut stream, &response, end, &cancel);
            if shutdown && response.result.is_ok() {
                drop(listener);
                let current = fs::symlink_metadata(&socket).unwrap();
                if current.dev() == socket_identity.dev() && current.ino() == socket_identity.ino()
                {
                    fs::remove_file(&socket).unwrap();
                } else {
                    panic!("socket replaced; retain deployment for audit");
                }
                temporary::remove_stage(engine_role(role)).expect("retain stage on cleanup error");
                return;
            }
            if !describe || response.result.is_err() {
                break;
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn engine_role(role: &str) -> light_bridge_usbip::kernel::Role {
    if role == "export" {
        light_bridge_usbip::kernel::Role::Export
    } else {
        light_bridge_usbip::kernel::Role::Import
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("USB/IP kernel helper requires Linux");
    std::process::exit(1);
}
