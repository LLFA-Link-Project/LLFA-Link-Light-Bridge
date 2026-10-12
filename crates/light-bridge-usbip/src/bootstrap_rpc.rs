//! Fixed automatic helper RPC. A route descriptor is mandatory for preparation.
use crate::{
    bootstrap::{Authorization, Candidate, Commit, Plan, Request, Stage, Stamp, VERSION},
    fd_channel, helper,
    kernel::{KResult, Role},
};
use serde::{Deserialize, Serialize};
use std::{
    os::fd::{AsRawFd, RawFd},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", content = "body", deny_unknown_fields)]
pub enum Action {
    Describe,
    Inventory,
    PrepareExport {
        request: Request,
        serial: Option<String>,
    },
    PrepareImport {
        offer: Box<Plan>,
    },
    Commit(Commit),
    Release {
        stamp: Stamp,
        runtime_released: bool,
    },
    Shutdown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub compatibility_id: String,
    pub role: Role,
    pub action: Action,
}
impl Envelope {
    pub fn new(role: Role, action: Action) -> Self {
        Self {
            version: VERSION,
            compatibility_id: helper::COMPATIBILITY_ID.into(),
            role,
            action,
        }
    }
    pub fn validate(&self) -> KResult<()> {
        if self.version != VERSION || self.compatibility_id != helper::COMPATIBILITY_ID {
            return Err("automatic helper incompatible; update required".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", deny_unknown_fields)]
pub enum Reply {
    Status {
        plan: Option<Plan>,
        stage: Option<Stage>,
    },
    Candidates(Vec<Candidate>),
    Prepared(Plan),
    Committed(Plan),
    Released,
    ShuttingDown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub compatibility_id: String,
    pub package_version: String,
    pub authorization: Authorization,
    pub pending: bool,
    pub result: KResult<Reply>,
}
impl Response {
    pub fn new(authorization: Authorization, pending: bool, result: KResult<Reply>) -> Self {
        let result = result.map_err(|mut error| {
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
        Self {
            version: VERSION,
            compatibility_id: helper::COMPATIBILITY_ID.into(),
            package_version: env!("CARGO_PKG_VERSION").into(),
            authorization,
            pending,
            result,
        }
    }
    pub fn validate(&self, role: Role) -> KResult<()> {
        if self.version != VERSION
            || self.compatibility_id != helper::COMPATIBILITY_ID
            || self.package_version != env!("CARGO_PKG_VERSION")
            || self.authorization.role != role
        {
            return Err(format!(
                "automatic helper update required; actual {} package {} schema {}",
                self.compatibility_id, self.package_version, self.version
            ));
        }
        self.authorization.validate()
    }
}
pub fn socket(role: Role) -> PathBuf {
    PathBuf::from(format!(
        "/run/llfa-link-light-bridge/{}.auto.sock",
        if role == Role::Export {
            "export"
        } else {
            "import"
        }
    ))
}
/// Non-mutating discovery for the launcher. Unknown/stopped installations never become Missing.
pub fn availability(role: Role) -> serde_json::Value {
    let path = socket(role);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => match call(&path, Envelope::new(role, Action::Describe), None) {
            Ok(response) => {
                serde_json::json!({"state":if response.pending{"Busy"}else{"Ready"},"identity":response})
            }
            Err(error) => {
                serde_json::json!({"state":if error.contains("update required")||error.contains("incompatible"){"NeedsUpdate"}else{"Unknown"},"diagnostic":error,"expected_compatibility":helper::COMPATIBILITY_ID})
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let name = if role == Role::Export {
                "export"
            } else {
                "import"
            };
            let markers = [
                format!("/run/llfa-link-light-bridge/{name}.sock"),
                format!("/run/llfa-link-light-bridge/temporary-auto-{name}"),
                format!("/run/llfa-link-light-bridge/temporary-{name}"),
                format!("/etc/llfa-link-light-bridge/{name}.json"),
                format!("/etc/llfa-link-light-bridge/{name}-authorization.json"),
                "/usr/local/libexec/llb-usbip-helper".into(),
                "/usr/libexec/llb-usbip-helper".into(),
                "/etc/systemd/system/llb-usbip-helper@.service".into(),
                "/usr/lib/systemd/system/llb-usbip-helper@.service".into(),
            ];
            for marker in markers {
                match std::fs::symlink_metadata(&marker) {
                    Ok(_) => {
                        return serde_json::json!({"state":"InstalledStoppedOrManual","diagnostic":"start/update/inspect existing deployment; no temporary fallback","marker":marker})
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => {
                        return serde_json::json!({"state":"Unknown","diagnostic":e.to_string()})
                    }
                }
            }
            serde_json::json!({"state":"Missing","action":"authorize_temporary_helper","installer":"deferred"})
        }
        Err(e) => serde_json::json!({"state":"Unknown","diagnostic":e.to_string()}),
    }
}
pub fn call(path: &Path, request: Envelope, route_fd: Option<RawFd>) -> KResult<Response> {
    call_cancelable(path, request, route_fd, &AtomicBool::new(false))
}
pub fn call_cancelable(
    path: &Path,
    request: Envelope,
    route_fd: Option<RawFd>,
    cancel: &AtomicBool,
) -> KResult<Response> {
    request.validate()?;
    let wants_fd = matches!(
        request.action,
        Action::PrepareExport { .. } | Action::PrepareImport { .. }
    );
    if wants_fd != route_fd.is_some() {
        return Err("prepare requires exactly one route TCP fd; other operations forbid it".into());
    }
    if let Some(fd) = route_fd {
        fd_channel::route_from_fd(fd)?;
    }
    let mut stream = helper::trusted_connection(path, cancel)?;
    let end = Instant::now() + Duration::from_secs(3);
    fd_channel::send(
        &mut stream,
        &Envelope::new(request.role, Action::Describe),
        None,
        end,
        cancel,
    )?;
    let (hello, fd): (Response, _) = fd_channel::receive(&mut stream, end, cancel)?;
    if fd.is_some() {
        return Err("unexpected helper response fd".into());
    }
    hello.validate(request.role)?;
    hello.result.clone()?;
    let uid = unsafe { libc::geteuid() };
    if uid != 0 && hello.authorization.client_uid != uid {
        return Err("automatic helper runtime UID mismatch".into());
    }
    if matches!(request.action, Action::Describe) {
        return Ok(hello);
    }
    let end = Instant::now() + Duration::from_secs(if wants_fd { 180 } else { 15 });
    fd_channel::send(&mut stream, &request, route_fd, end, cancel)?;
    let (response, fd): (Response, _) = fd_channel::receive(&mut stream, end, cancel)?;
    if fd.is_some() {
        return Err("unexpected helper response descriptor".into());
    }
    response.validate(request.role)?;
    if response.authorization != hello.authorization {
        return Err("helper authorization changed during RPC".into());
    }
    Ok(response)
}
pub fn peer_route(fd: RawFd) -> KResult<crate::bootstrap::Route> {
    let r = fd_channel::route_from_fd(fd)?;
    if r.netns_cookie != fd_channel::local_namespace_cookie()? {
        return Err("cross-network-namespace route unsupported".into());
    }
    Ok(r)
}
/// C/adapter users retain their route FD; SCM_RIGHTS only transfers a duplicate.
pub fn borrowed_fd(fd: &std::os::fd::OwnedFd) -> RawFd {
    fd.as_raw_fd()
}
