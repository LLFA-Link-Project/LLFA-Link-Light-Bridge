//! Automatic root helper: bootstrap socket plus gated legacy kernel-operation socket.
use light_bridge_usbip::{
    bootstrap::{self, Authorization, Engine as AutoEngine},
    bootstrap_rpc::{self, Action, Envelope, Reply, Response},
    command,
    environment::{FileEnvironmentJournal, LinuxEnvironment},
    fd_channel, helper,
    kernel::{Engine, FileJournal, KResult, LinuxOps, Role},
};
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
type Kernel = Engine<LinuxOps, FileJournal>;
type Auto = AutoEngine<LinuxEnvironment, FileEnvironmentJournal>;
struct Listener {
    listener: UnixListener,
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl Listener {
    fn bind(path: PathBuf, gid: u32) -> KResult<Self> {
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("socket already exists or unavailable (never unlink): {e}"))?;
        use std::os::unix::ffi::OsStrExt;
        let p = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        if unsafe { libc::chown(p.as_ptr(), 0, gid) } != 0 {
            return Err("socket chown failed; inspect residual path".into());
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o660)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let m = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            path,
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn remove(self) -> KResult<()> {
        let m = fs::symlink_metadata(&self.path).map_err(|e| e.to_string())?;
        if m.dev() != self.dev || m.ino() != self.ino {
            return Err("owned socket replaced; retain for audit".into());
        }
        drop(self.listener);
        fs::remove_file(self.path).map_err(|e| e.to_string())
    }
}
fn directory(path: &Path, mode: u32) -> KResult<()> {
    match fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o777 != mode {
        return Err("protected root directory required".into());
    }
    command::trusted(path).map_err(|e| e.to_string())
}
fn state_path(role: Role) -> PathBuf {
    PathBuf::from(format!(
        "/var/lib/llfa-link-light-bridge/{}.json",
        super::temporary::role_name(role)
    ))
}
fn kernel_empty(role: Role, kernel: &Option<Kernel>) -> KResult<bool> {
    if kernel.as_ref().is_some_and(|k| k.record().is_some()) {
        return Ok(false);
    }
    for p in [
        state_path(role),
        state_path(role).with_extension("json.new"),
    ] {
        match fs::symlink_metadata(p) {
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}
pub fn serve(path: &Path) -> KResult<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("automatic helper requires root; runtimes remain unprivileged".into());
    }
    command::trusted(path).map_err(|e| e.to_string())?;
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("authorization too large".into());
    }
    let authorization: Authorization = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    authorization.validate()?;
    let role = authorization.role;
    let temporary = path == super::temporary::auto_stage(role).join("authorization.json");
    directory(Path::new("/run/llfa-link-light-bridge"), 0o711)?;
    directory(Path::new("/var/lib/llfa-link-light-bridge"), 0o700)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state_path(role).with_extension("lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("helper role already owned".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let mut auto = Auto::new(
        authorization.clone(),
        LinuxEnvironment {
            cancel: cancel.clone(),
        },
        FileEnvironmentJournal {
            path: PathBuf::from(format!(
                "/var/lib/llfa-link-light-bridge/{}-environment.json",
                super::temporary::role_name(role)
            )),
        },
    )?;
    let listener = Listener::bind(bootstrap_rpc::socket(role), authorization.client_gid)?;
    let mut manual: Option<Listener> = None;
    let mut kernel: Option<Kernel> = None;
    let manual_path = PathBuf::from(format!(
        "/run/llfa-link-light-bridge/{}.sock",
        super::temporary::role_name(role)
    ));
    eprintln!(
        "Automatic helper ready: {} compatibility={}",
        listener.path.display(),
        helper::COMPATIBILITY_ID
    );
    loop {
        match listener.listener.accept() {
            Ok((mut stream, _)) => {
                let uid = helper::peer_uid(&stream).map_err(|e| e.to_string())?;
                if uid != authorization.client_uid && uid != 0 {
                    continue;
                }
                for _ in 0..2 {
                    let end = Instant::now() + Duration::from_secs(10);
                    let (request, route): (Envelope, _) =
                        match fd_channel::receive(&mut stream, end, &cancel) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("auto receive: {e}");
                                break;
                            }
                        };
                    let describe = matches!(request.action, Action::Describe);
                    let shutdown = matches!(request.action, Action::Shutdown);
                    let result = (|| -> KResult<Reply> {
                        request.validate()?;
                        if request.role != role || (uid != authorization.client_uid && !describe) {
                            return Err("automatic helper peer/role denied".into());
                        }
                        let prepare = matches!(
                            request.action,
                            Action::PrepareExport { .. } | Action::PrepareImport { .. }
                        );
                        if prepare != route.is_some() {
                            return Err("exactly one TCP route FD required for Prepare only".into());
                        }
                        match request.action {
                            Action::Describe => Ok(Reply::Status {
                                plan: auto.lease.as_ref().map(|l| l.plan.clone()),
                                stage: auto.lease.as_ref().map(|l| l.stage),
                            }),
                            Action::Inventory => {
                                use bootstrap::Ops;
                                Ok(Reply::Candidates(bootstrap::candidates(
                                    auto.ops.inventory()?,
                                )?))
                            }
                            Action::PrepareExport { request, serial } => {
                                if role != Role::Export || !kernel_empty(role, &kernel)? {
                                    return Err(
                                        "export role or residual USB ownership denied".into()
                                    );
                                }
                                let route =
                                    bootstrap_rpc::peer_route(route.as_ref().unwrap().as_raw_fd())?;
                                Ok(Reply::Prepared(auto.prepare(
                                    request,
                                    route,
                                    None,
                                    serial.as_deref(),
                                )?))
                            }
                            Action::PrepareImport { offer } => {
                                if role != Role::Import || !kernel_empty(role, &kernel)? {
                                    return Err(
                                        "import role or residual USB ownership denied".into()
                                    );
                                }
                                let route =
                                    bootstrap_rpc::peer_route(route.as_ref().unwrap().as_raw_fd())?;
                                Ok(Reply::Prepared(auto.prepare(
                                    offer.request.clone(),
                                    route,
                                    Some(*offer),
                                    None,
                                )?))
                            }
                            Action::Commit(c) => {
                                let plan = auto.commit(&c)?;
                                if kernel.is_none() {
                                    if !kernel_empty(role, &kernel)? {
                                        return Err("old USB journal blocks commit".into());
                                    }
                                    let engine = Kernel::new(
                                        plan.policy.clone(),
                                        LinuxOps {
                                            cancel: cancel.clone(),
                                            remote: plan.policy.remote,
                                        },
                                        FileJournal {
                                            path: state_path(role),
                                        },
                                    )?;
                                    if engine.record().is_some() {
                                        return Err("old USB lease cannot be adopted".into());
                                    }
                                    let socket = Listener::bind(
                                        manual_path.clone(),
                                        authorization.client_gid,
                                    )?;
                                    kernel = Some(engine);
                                    manual = Some(socket);
                                }
                                Ok(Reply::Committed(plan))
                            }
                            Action::Release {
                                stamp,
                                runtime_released,
                            } => {
                                auto.release(
                                    &stamp,
                                    runtime_released,
                                    kernel_empty(role, &kernel)?,
                                )?;
                                kernel = None;
                                if let Some(socket) = manual.take() {
                                    socket.remove()?;
                                }
                                Ok(Reply::Released)
                            }
                            Action::Shutdown => {
                                if !temporary
                                    || auto.lease.is_some()
                                    || !kernel_empty(role, &kernel)?
                                {
                                    return Err("shutdown denied: installed helper or owned environment/USB resources".into());
                                }
                                Ok(Reply::ShuttingDown)
                            }
                        }
                    })();
                    let done = shutdown && result.is_ok();
                    if let Err(error) = &result {
                        eprintln!("auto preparation: {error}");
                    }
                    let response = Response::new(
                        authorization.clone(),
                        auto.lease.is_some() || !kernel_empty(role, &kernel)?,
                        result,
                    );
                    let _ = fd_channel::send(
                        &mut stream,
                        &response,
                        None,
                        Instant::now() + Duration::from_secs(3),
                        &cancel,
                    );
                    if done {
                        if let Some(socket) = manual.take() {
                            socket.remove()?;
                        }
                        listener.remove()?;
                        super::temporary::remove_auto_stage(role)?;
                        return Ok(());
                    }
                    if !describe || response.result.is_err() {
                        break;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.to_string()),
        }
        if let (Some(listener), Some(engine)) = (manual.as_ref(), kernel.as_mut()) {
            match listener.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(lease) = &auto.lease {
                        manual_request(stream, engine, &lease.plan.request.stamp, &cancel);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e.to_string()),
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn manual_request(
    mut stream: UnixStream,
    engine: &mut Kernel,
    stamp: &bootstrap::Stamp,
    cancel: &AtomicBool,
) {
    let Ok(uid) = helper::peer_uid(&stream) else {
        return;
    };
    if uid != engine.policy.client_uid && uid != 0 {
        return;
    }
    let end = Instant::now() + Duration::from_secs(12);
    for _ in 0..2 {
        let request: helper::Request = match helper::receive(&mut stream, end, cancel) {
            Ok(r) => r,
            Err(_) => break,
        };
        let describe = matches!(request.action, helper::Action::Describe);
        if matches!(request.action, helper::Action::Shutdown) {
            break;
        }
        if !describe && (request.instance != stamp.instance || request.session != stamp.session) {
            let response = helper::Response {
                version: helper::RPC_VERSION,
                identity: helper::HelperIdentity::current(&engine.policy),
                result: Err(
                    "kernel operation not authorized for committed bootstrap session".into(),
                ),
                pending: engine.record().is_some(),
            };
            let _ = helper::send(&mut stream, &response, end, cancel);
            break;
        }
        let response = helper::dispatch(engine, uid, request);
        let _ = helper::send(&mut stream, &response, end, cancel);
        if !describe || response.result.is_err() {
            break;
        }
    }
}
