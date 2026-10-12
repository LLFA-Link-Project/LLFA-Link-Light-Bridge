//! Explicitly authorized development deployment. No persistent unit or installer.
use light_bridge_usbip::{
    command,
    helper::{self, Action, Response},
    kernel::{Policy, Role},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, String>;
const ROOT: &str = "/run/llfa-link-light-bridge";
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Export => "export",
        Role::Import => "import",
    }
}
pub fn stage(role: Role) -> PathBuf {
    Path::new(ROOT).join(format!("temporary-{}", role_name(role)))
}
fn socket(role: Role) -> PathBuf {
    Path::new(ROOT).join(format!("{}.sock", role_name(role)))
}
fn read_policy(path: &Path) -> Result<Policy> {
    if !path.is_absolute() {
        return Err("absolute policy path required".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("policy must be a regular non-symlink file".into());
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("policy too large".into());
    }
    let policy: Policy = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    policy.validate()?;
    Ok(policy)
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}
fn installation_markers(role: Role) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/libexec/llb-usbip-helper"),
        PathBuf::from("/etc/systemd/system/llb-usbip-helper@.service"),
        PathBuf::from("/usr/lib/systemd/system/llb-usbip-helper@.service"),
        PathBuf::from(format!(
            "/etc/llfa-link-light-bridge/{}.json",
            role_name(role)
        )),
    ]
}
pub fn check(path: &Path) -> Result<()> {
    let policy = read_policy(path)?;
    if !exists(&socket(policy.role))? {
        if exists(&stage(policy.role))? {
            return Err("temporary deployment remains without a live socket; inspect transient unit/stage before retrying".into());
        }
        let installed = installation_markers(policy.role)
            .iter()
            .map(|p| exists(p))
            .collect::<Result<Vec<_>>>()?
            .contains(&true);
        return Err(if installed {
            "installed helper appears stopped/incomplete; start or diagnose it, do not use a temporary fallback"
        } else { "helper not installed/running: choose installer (not implemented) or explicit temporary-start" }.into());
    }
    let response = helper::probe_helper(&socket(policy.role), &policy)?;
    println!(
        "compatible helper: {} build={} role={} pending={}",
        response.identity.compatibility_id,
        response.identity.build_revision,
        role_name(policy.role),
        response.pending
    );
    if response.pending {
        return Err("existing ownership journal; do not begin another session".into());
    }
    Ok(())
}
fn protected_dir(path: &Path, mode: u32) -> Result<()> {
    match fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o777 != mode {
        return Err("root-owned protected directory required".into());
    }
    command::trusted(path).map_err(|e| e.to_string())
}
fn write_new(path: &Path, mode: u32, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}
pub fn start(path: &Path) -> Result<()> {
    // This command is invoked explicitly through sudo; it never elevates itself.
    if unsafe { libc::geteuid() } != 0 {
        return Err("temporary-start requires explicit sudo authorization".into());
    }
    let policy = read_policy(path)?;
    if let Ok(uid) = std::env::var("SUDO_UID") {
        if uid.parse::<u32>().map_err(|_| "invalid SUDO_UID")? != policy.client_uid {
            return Err("policy client_uid must equal the authorizing sudo user".into());
        }
    }
    if let Ok(gid) = std::env::var("SUDO_GID") {
        if gid.parse::<u32>().map_err(|_| "invalid SUDO_GID")? != policy.client_gid {
            return Err(
                "policy client_gid must equal the authorizing sudo user's primary GID".into(),
            );
        }
    }
    // Existing peer wins. A mismatch/error must not start a second helper.
    if exists(&socket(policy.role))? {
        return check(path);
    }
    for marker in installation_markers(policy.role) {
        if exists(&marker)? {
            return Err(
                "installed helper stopped/incomplete; diagnose or update it, no temporary fallback"
                    .into(),
            );
        }
    }
    command::executable().map_err(|e| e.to_string())?;
    protected_dir(Path::new(ROOT), 0o711)?;
    protected_dir(Path::new("/var/lib/llfa-link-light-bridge"), 0o700)?;
    let state = Path::new("/var/lib/llfa-link-light-bridge");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state.join(format!("{}.lock", role_name(policy.role))))
        .map_err(|e| e.to_string())?;
    command::trusted(&state.join(format!("{}.lock", role_name(policy.role))))
        .map_err(|e| e.to_string())?;
    // Same lock as serve, held throughout staging to exclude other helpers/launchers.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("another helper/launcher owns this role".into());
    }
    for name in [
        format!("{}.json", role_name(policy.role)),
        format!("{}.json.new", role_name(policy.role)),
    ] {
        if exists(&state.join(name))? {
            return Err("residual ownership journal: audit before starting".into());
        }
    }
    let dir = stage(policy.role);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("staging refused (no overwrite): {e}"))?;
    protected_dir(&dir, 0o700)?;
    // Copy the actual running executable inode, not a replaceable pathname. Administrator
    // explicitly trusts this development build; this is not an authenticated installer.
    let mut binary = Vec::new();
    File::open("/proc/self/exe")
        .map_err(|e| e.to_string())?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut binary)
        .map_err(|e| e.to_string())?;
    if binary.len() > 64 * 1024 * 1024 {
        return Err("development binary exceeds staging limit".into());
    }
    write_new(&dir.join("helper"), 0o700, &binary)?;
    write_new(
        &dir.join("policy.json"),
        0o600,
        &serde_json::to_vec(&policy).map_err(|e| e.to_string())?,
    )?;
    // Releasing the lock allows serve to acquire it. Concurrent acquisition still fails
    // closed; no staged file or socket is overwritten and readiness must be proved below.
    drop(lock);
    command::start_temporary_service(
        role_name(policy.role),
        &dir.join("helper"),
        &dir.join("policy.json"),
    )?;
    let end = Instant::now() + Duration::from_secs(5);
    while !exists(&socket(policy.role))? {
        if Instant::now() >= end {
            return Err(
                "temporary service not ready; retain stage for audit; inspect journalctl".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    check(path)?;
    println!(
        "temporary service ready: llb-usbip-temporary-{}.service; no persistent registration",
        role_name(policy.role)
    );
    Ok(())
}
pub fn stop(path: &Path) -> Result<()> {
    let policy = read_policy(path)?;
    let cancel = AtomicBool::new(false);
    let mut stream = helper::trusted_connection(&socket(policy.role), &cancel)?;
    let end = Instant::now() + Duration::from_secs(3);
    let mut request = helper::describe_request(&policy);
    helper::send(&mut stream, &request, end, &cancel)?;
    let response: Response = helper::receive(&mut stream, end, &cancel)?;
    helper::validate_response(&response, &policy)?;
    response.result?;
    if response.pending {
        return Err("runtime cleanup incomplete: retain helper and journal".into());
    }
    request.action = Action::Shutdown;
    request.runtime_released = true;
    helper::send(&mut stream, &request, end, &cancel)?;
    let response: Response = helper::receive(&mut stream, end, &cancel)?;
    helper::validate_response(&response, &policy)?;
    response.result?;
    let end = Instant::now() + Duration::from_secs(5);
    while exists(&socket(policy.role))? || exists(&stage(policy.role))? {
        if Instant::now() >= end {
            return Err(
                "shutdown acknowledged, but socket/stage cleanup unconfirmed; inspect logs".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("temporary helper stopped after confirmed empty journal; staged files removed");
    Ok(())
}
pub fn remove_stage(role: Role) -> Result<()> {
    let dir = stage(role);
    command::trusted(&dir).map_err(|e| e.to_string())?;
    // Only fixed, owned files; never recursive removal or journal deletion.
    for name in ["policy.json", "helper"] {
        let path = dir.join(name);
        command::trusted(&path).map_err(|e| e.to_string())?;
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    fs::remove_dir(dir).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_reading_rejects_relative_symlink_oversize_and_hmd() {
        assert!(read_policy(Path::new("relative.json")).is_err());
        let dir = std::env::temp_dir().join(format!("llb-temporary-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let file = dir.join("policy.json");
        fs::write(&file, vec![b' '; 4097]).unwrap();
        assert!(read_policy(&file).unwrap_err().contains("large"));
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_policy(&link).is_err());
        let mut p: serde_json::Value = serde_json::from_str(include_str!(
            "../../../examples/usbip/export-policy.template.json"
        ))
        .unwrap();
        fs::write(&file, serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(read_policy(&file).is_ok());
        p["device"]["vid"] = 0x2c30.into();
        p["device"]["pid"] = 0x1050.into();
        fs::write(&file, serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(read_policy(&file).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(dir).unwrap();
        assert_eq!(
            stage(Role::Export),
            Path::new(ROOT).join("temporary-export")
        );
    }
}
