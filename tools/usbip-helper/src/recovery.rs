//! Explicit, conservative recovery of a failed automatic temporary launch.
//! No journal/socket/USB deletion, service stop, recursive removal or adoption.
use light_bridge_usbip::{command, kernel::Role};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, String>;
fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!("{} exists; retain and inspect", path.display())),
        Err(e) => Err(e.to_string()),
    }
}
fn stopped(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once('=').ok_or("invalid service properties")?;
        if values.insert(k, v).is_some() {
            return Err("duplicate service property".into());
        }
    }
    if !matches!(values.get("LoadState"), Some(&"loaded" | &"not-found"))
        || !matches!(values.get("ActiveState"), Some(&"inactive" | &"failed"))
        || !matches!(values.get("SubState"), Some(&"dead" | &"failed"))
        || values.get("MainPID") != Some(&"0")
        || values.get("ControlPID") != Some(&"0")
    {
        return Err("service not proven stopped/absent; no recovery".into());
    }
    Ok(())
}
#[derive(Clone)]
struct Entry {
    path: PathBuf,
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    nlink: u64,
    len: u64,
}
impl Entry {
    fn read(path: PathBuf) -> Result<Self> {
        let m = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            dev: m.dev(),
            ino: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            nlink: m.nlink(),
            len: m.len(),
        })
    }
    fn unchanged(&self) -> Result<()> {
        let e = Self::read(self.path.clone())?;
        if (e.dev, e.ino, e.mode, e.uid, e.gid, e.nlink, e.len)
            != (
                self.dev, self.ino, self.mode, self.uid, self.gid, self.nlink, self.len,
            )
        {
            return Err("staging entry changed during audit; retain".into());
        }
        Ok(())
    }
}
struct Stage {
    directory: Entry,
    files: Vec<Entry>,
}
fn scan(path: &Path, allowed: &[(&str, u32)], uid: u32) -> Result<Option<Stage>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(m) if !m.is_dir() || m.uid() != uid || m.mode() & 0o7777 != 0o700 => {
            return Err("staging directory ownership/type/mode invalid".into())
        }
        Ok(_) => (),
    }
    let directory = Entry::read(path.into())?;
    let mut files = Vec::new();
    for item in fs::read_dir(path).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item.file_name();
        let mode = allowed
            .iter()
            .find(|(n, _)| Some(*n) == name.to_str())
            .map(|(_, m)| *m)
            .ok_or("unknown staging entry; retain all files")?;
        let m = fs::symlink_metadata(item.path()).map_err(|e| e.to_string())?;
        if !m.is_file() || m.uid() != uid || m.mode() & 0o7777 != mode || m.nlink() != 1 {
            return Err("staged file ownership/type/mode/link count invalid".into());
        }
        if (name == "helper" && m.len() > 64 * 1024 * 1024)
            || (name == "authorization.json" && m.len() > 4096)
        {
            return Err("staged file size invalid".into());
        }
        files.push(Entry::read(item.path())?);
    }
    Ok(Some(Stage { directory, files }))
}
fn audit_units(role: &str) -> Result<()> {
    for automatic in [true, false] {
        let status = command::temporary_unit_status(role, automatic)?;
        eprintln!(
            "{} helper service: {}",
            if automatic { "automatic" } else { "manual" },
            String::from_utf8_lossy(&status)
        );
        stopped(&status)?;
    }
    Ok(())
}
fn audit_empty(role: &str) -> Result<()> {
    for name in [
        format!("{role}.json"),
        format!("{role}.json.new"),
        format!("{role}-environment.json"),
        format!("{role}-environment.json.new"),
    ] {
        absent(&Path::new("/var/lib/llfa-link-light-bridge").join(name))?;
    }
    for name in [
        format!("{role}.sock"),
        format!("{role}.auto.sock"),
        format!("temporary-{role}"),
    ] {
        absent(&Path::new("/run/llfa-link-light-bridge").join(name))?;
    }
    absent(&super::temporary::binary_stage(
        if role == "export" {
            Role::Export
        } else {
            Role::Import
        },
        false,
    ))
}
pub fn automatic(value: &std::ffi::OsStr) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("recovery requires explicit sudo authorization".into());
    }
    let role = match value.to_str() {
        Some("export") => Role::Export,
        Some("import") => Role::Import,
        _ => return Err("role must be export or import".into()),
    };
    let name = super::temporary::role_name(role);
    let root = Path::new("/var/lib/llfa-link-light-bridge");
    command::trusted(root).map_err(|e| e.to_string())?;
    command::trusted(Path::new("/run/llfa-link-light-bridge")).map_err(|e| e.to_string())?;
    let lock_path = root.join(format!("{name}.lock"));
    command::trusted(&lock_path).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)
        .map_err(|e| e.to_string())?;
    if !lock.metadata().map_err(|e| e.to_string())?.is_file()
        || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
    {
        return Err("helper role lock unavailable; no recovery".into());
    }
    audit_units(name)?;
    audit_empty(name)?;
    let config = super::temporary::auto_stage(role);
    let binary = super::temporary::binary_stage(role, true);
    let stages = [
        scan(
            &config,
            &[("authorization.json", 0o600), ("helper", 0o700)],
            0,
        )?,
        scan(&binary, &[("helper", 0o700)], 0)?,
    ];
    if stages.iter().all(Option::is_none) {
        return Err("no failed automatic staging found".into());
    }
    // Validate even an old authorization snapshot before deleting it. Partial staging without
    // this file is allowed, but malformed/unknown content is retained for operator inspection.
    let auth = config.join("authorization.json");
    if auth.exists() {
        let bytes = fs::read(&auth).map_err(|e| e.to_string())?;
        let auth: light_bridge_usbip::bootstrap::Authorization =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        auth.validate()?;
        if auth.role != role {
            return Err("staged authorization role mismatch".into());
        }
    }
    // Repeat service/journal checks under the same role lock before the first deletion.
    audit_units(name)?;
    audit_empty(name)?;
    for stage in stages.iter().flatten() {
        stage.directory.unchanged()?;
        for file in &stage.files {
            file.unchanged()?;
        }
    }
    for stage in stages.into_iter().flatten() {
        for file in stage.files {
            file.unchanged()?;
            fs::remove_file(file.path).map_err(|e| e.to_string())?;
        }
        fs::remove_dir(&stage.directory.path).map_err(|e| e.to_string())?;
        File::open(stage.directory.path.parent().unwrap())
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    println!("failed automatic staging removed after ownership/service/lock/empty-journal audit; no USB, socket or journal modified");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[test]
    fn service_audit_refuses_running_missing_or_duplicate_properties() {
        let stopped_unit="LoadState=loaded\nActiveState=failed\nSubState=failed\nMainPID=0\nControlPID=0\nResult=exit-code\nExecMainStatus=203\n";
        stopped(stopped_unit.as_bytes()).unwrap();
        stopped(
            stopped_unit
                .replace("loaded", "not-found")
                .replace("failed", "inactive")
                .replace("SubState=inactive", "SubState=dead")
                .as_bytes(),
        )
        .unwrap();
        for text in [
            stopped_unit.replace("MainPID=0", "MainPID=42"),
            stopped_unit.replace("ControlPID=0", "ControlPID=42"),
            stopped_unit.replace("ActiveState=failed", "ActiveState=activating"),
            stopped_unit.replace("LoadState=loaded", "LoadState=error"),
            stopped_unit.replace("MainPID=0\n", ""),
            format!("{stopped_unit}MainPID=0\n"),
        ] {
            assert!(stopped(text.as_bytes()).is_err());
        }
    }
    #[test]
    fn stage_audit_refuses_unknown_symlink_wrong_owner_mode_and_hardlink() {
        let path = std::env::temp_dir().join(format!("llb-recovery-test-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let allowed = [("helper", 0o700)];
        fs::write(path.join("helper"), "binary").unwrap();
        fs::set_permissions(path.join("helper"), fs::Permissions::from_mode(0o700)).unwrap();
        let scanned = scan(&path, &allowed, uid).unwrap().unwrap();
        assert!(scan(&path, &allowed, uid + 1).is_err());
        fs::write(path.join("unknown"), "").unwrap();
        assert!(scan(&path, &allowed, uid).is_err());
        fs::remove_file(path.join("unknown")).unwrap();
        let linked = path.with_extension("linked");
        fs::hard_link(path.join("helper"), &linked).unwrap();
        assert!(scan(&path, &allowed, uid).is_err());
        fs::remove_file(linked).unwrap();
        fs::set_permissions(path.join("helper"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(scan(&path, &allowed, uid).is_err());
        assert!(scanned.files[0].unchanged().is_err());
        fs::remove_file(path.join("helper")).unwrap();
        symlink("/usr/bin/true", path.join("helper")).unwrap();
        assert!(scan(&path, &allowed, uid).is_err());
        fs::remove_file(path.join("helper")).unwrap();
        fs::remove_dir(path).unwrap();
    }
    #[test]
    fn any_journal_or_socket_including_symlink_blocks_recovery() {
        let path = std::env::temp_dir().join(format!("llb-recovery-absent-{}", std::process::id()));
        absent(&path).unwrap();
        symlink("/nonexistent", &path).unwrap();
        assert!(absent(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
