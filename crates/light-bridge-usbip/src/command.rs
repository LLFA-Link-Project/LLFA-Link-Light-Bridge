//! Fixed usbip invocation, bounded output, cancellation, process-group kill/reap.
use std::{
    fs,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
pub const USBIP: &str = "/usr/bin/usbip";
pub fn executable() -> io::Result<&'static Path> {
    for path in [USBIP, "/usr/sbin/usbip"] {
        let path = Path::new(path);
        if path.exists() {
            trusted(path)?;
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "usbip missing at /usr/bin/usbip and /usr/sbin/usbip",
    ))
}
#[derive(Debug, Clone)]
pub enum Operation {
    Bind(String),
    Unbind(String),
    Attach(std::net::SocketAddr, String),
    Detach(u32),
}
impl Operation {
    pub fn arguments(&self) -> Vec<String> {
        match self {
            Self::Bind(b) => vec!["bind".into(), "--busid".into(), b.clone()],
            Self::Unbind(b) => vec!["unbind".into(), "--busid".into(), b.clone()],
            Self::Attach(a, b) => vec![
                "--tcp-port".into(),
                a.port().to_string(),
                "attach".into(),
                "--remote".into(),
                match a {
                    std::net::SocketAddr::V6(v) if v.scope_id() != 0 => {
                        format!("{}%{}", v.ip(), v.scope_id())
                    }
                    _ => a.ip().to_string(),
                },
                "--busid".into(),
                b.clone(),
            ],
            Self::Detach(p) => vec!["detach".into(), "--port".into(), p.to_string()],
        }
    }
}
pub fn trusted(path: &Path) -> io::Result<()> {
    let real = fs::canonicalize(path)?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "absolute trusted path required",
        ));
    }
    for p in path.ancestors().chain(real.ancestors()) {
        let m = fs::metadata(p)?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path must be root-owned and not group/world writable",
            ));
        }
    }
    Ok(())
}
fn nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: fd is borrowed from a live pipe; preserve its existing flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn drain(pipe: &mut dyn Read, output: &mut Vec<u8>) -> Result<(), String> {
    let mut b = [0; 1024];
    loop {
        match pipe.read(&mut b) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                output.extend_from_slice(&b[..n]);
                if output.len() > 32768 {
                    return Err("usbip output limit exceeded".into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}
pub fn run(operation: &Operation, timeout: Duration, cancel: &AtomicBool) -> Result<(), String> {
    let program = executable().map_err(|e| e.to_string())?;
    run_program(program, &operation.arguments(), timeout, cancel)
}
/// Fixed root-owned binary snapshot; /run may be mounted noexec.
pub fn temporary_executable(role: &str, automatic: bool) -> Result<PathBuf, String> {
    if !matches!(role, "export" | "import") {
        return Err("temporary role invalid".into());
    }
    let mode = if automatic {
        "temporary-auto"
    } else {
        "temporary"
    };
    Ok(PathBuf::from(format!(
        "/var/lib/llfa-link-light-bridge/{mode}-{role}/helper"
    )))
}
fn executable_mount_flags(flags: libc::c_ulong) -> Result<(), String> {
    if flags & libc::ST_NOEXEC != 0 {
        return Err("temporary executable filesystem is noexec; use an executable /var/lib deployment (no automatic remount or interpreter bypass)".into());
    }
    Ok(())
}
/// Check the existing parent before creating staging, then the copied binary before launching.
pub fn executable_mount(path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: valid CString and writable output; only initialized on success.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(format!(
            "cannot inspect executable mount: {}",
            io::Error::last_os_error()
        ));
    }
    executable_mount_flags(unsafe { stat.assume_init() }.f_flag)
}
fn verify_temporary_executable(path: &Path) -> Result<(), String> {
    trusted(path).map_err(|e| e.to_string())?;
    executable_mount(path)?;
    // Actually execute the protected copy: catches loader/architecture/LSM permission errors
    // before systemd is asked to start a long-lived privileged service.
    let version = capture(
        path,
        &["--version".into()],
        Duration::from_secs(3),
        &AtomicBool::new(false),
    )?;
    let version = String::from_utf8(version).map_err(|e| e.to_string())?;
    if !version.contains(&format!(
        "compatibility={}",
        crate::helper::COMPATIBILITY_ID
    )) {
        return Err("staged helper identity mismatch; retain staging for inspection".into());
    }
    Ok(())
}
/// Read-only recovery audit of a fixed transient helper unit. Never stop/kill a service.
pub fn temporary_unit_status(role: &str, automatic: bool) -> Result<Vec<u8>, String> {
    temporary_executable(role, automatic)?;
    if unsafe { libc::geteuid() } != 0 {
        return Err("explicit root authorization required".into());
    }
    let mode = if automatic {
        "temporary-auto"
    } else {
        "temporary"
    };
    let program = Path::new("/usr/bin/systemctl");
    trusted(program).map_err(|e| e.to_string())?;
    let unit = format!("llb-usbip-{mode}-{role}.service");
    match capture(
        program,
        &[
            "show".into(),
            "--property=LoadState,ActiveState,SubState,MainPID,ControlPID,Result,ExecMainStatus"
                .into(),
            unit.clone(),
        ],
        Duration::from_secs(5),
        &AtomicBool::new(false),
    ) {
        Ok(status) => Ok(status),
        Err(show_error) => {
            // --collect may already have unloaded a failed unit. A successful, empty
            // exact-name list proves absence; bus/query errors never imply stopped.
            let units = capture(
                program,
                &[
                    "list-units".into(),
                    "--all".into(),
                    "--full".into(),
                    "--plain".into(),
                    "--no-legend".into(),
                    "--no-pager".into(),
                    unit,
                ],
                Duration::from_secs(5),
                &AtomicBool::new(false),
            )?;
            if units.iter().any(|b| !b.is_ascii_whitespace()) {
                return Err(show_error);
            }
            Ok(b"LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nControlPID=0\n".to_vec())
        }
    }
}
/// Fixed transient service launch, used only by the explicitly root-authorized CLI.
pub fn start_temporary_service(role: &str, executable: &Path, policy: &Path) -> Result<(), String> {
    let args = temporary_service_arguments(role, executable, policy)?;
    // No implicit polkit/sudo prompt from a library call.
    if unsafe { libc::geteuid() } != 0 {
        return Err("explicit root authorization required".into());
    }
    for path in [executable, policy, Path::new("/usr/bin/systemd-run")] {
        trusted(path).map_err(|e| e.to_string())?;
    }
    verify_temporary_executable(executable)?;
    run_program(
        Path::new("/usr/bin/systemd-run"),
        &args,
        Duration::from_secs(10),
        &AtomicBool::new(false),
    )
}
pub fn start_auto_service(role: &str) -> Result<(), String> {
    if !matches!(role, "export" | "import") || unsafe { libc::geteuid() } != 0 {
        return Err("fixed role and explicit root authorization required".into());
    }
    let executable = temporary_executable(role, true)?;
    let auth = Path::new(&format!(
        "/run/llfa-link-light-bridge/temporary-auto-{role}"
    ))
    .join("authorization.json");
    for p in [&executable, &auth, &PathBuf::from("/usr/bin/systemd-run")] {
        trusted(p).map_err(|e| e.to_string())?;
    }
    verify_temporary_executable(&executable)?;
    let args = vec![
        format!("--unit=llb-usbip-temporary-auto-{role}"),
        "--collect".into(),
        "--service-type=exec".into(),
        "--property=User=root".into(),
        "--property=Group=root".into(),
        "--property=UMask=0077".into(),
        "--property=Restart=no".into(),
        "--property=NoNewPrivileges=yes".into(),
        executable.to_string_lossy().into_owned(),
        "serve-auto".into(),
        auth.to_string_lossy().into_owned(),
    ];
    capture(
        Path::new("/usr/bin/systemd-run"),
        &args,
        Duration::from_secs(10),
        &AtomicBool::new(false),
    )
    .map(|_| ())
}
fn temporary_service_arguments(
    role: &str,
    executable: &Path,
    policy: &Path,
) -> Result<Vec<String>, String> {
    if !matches!(role, "export" | "import") {
        return Err("temporary role invalid".into());
    }
    let expected = format!("/run/llfa-link-light-bridge/temporary-{role}");
    if executable != temporary_executable(role, false)?
        || policy != Path::new(&expected).join("policy.json")
    {
        return Err("fixed staging paths required".into());
    }
    let args = vec![
        format!("--unit=llb-usbip-temporary-{role}"),
        "--collect".into(),
        "--service-type=exec".into(),
        "--property=User=root".into(),
        "--property=Group=root".into(),
        "--property=UMask=0077".into(),
        "--property=Restart=no".into(),
        "--property=NoNewPrivileges=yes".into(),
        "--property=WorkingDirectory=/".into(),
        executable.to_string_lossy().into_owned(),
        "serve-temporary".into(),
        policy.to_string_lossy().into_owned(),
    ];
    Ok(args)
}
fn run_program(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(), String> {
    capture(program, args, timeout, cancel).map(|_| ())
}
/// Internal runner for fixed typed bootstrap commands. No request-supplied program/argv.
pub(crate) fn capture(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let result = (|| {
        nonblocking(stdout.as_raw_fd()).map_err(|e| e.to_string())?;
        nonblocking(stderr.as_raw_fd()).map_err(|e| e.to_string())?;
        let end = Instant::now() + timeout;
        let mut output = Vec::new();
        let mut errors = Vec::new();
        loop {
            drain(&mut stdout, &mut output)?;
            drain(&mut stderr, &mut errors)?;
            if output.len() + errors.len() > 32768 {
                return Err("command output limit exceeded".into());
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                drain(&mut stdout, &mut output)?;
                drain(&mut stderr, &mut errors)?;
                return if status.success() {
                    Ok(output)
                } else {
                    Err(format!(
                        "usbip exit {status}: {}",
                        String::from_utf8_lossy(&errors)
                    ))
                };
            }
            if cancel.load(Ordering::Acquire) || Instant::now() >= end {
                return Err(format!(
                    "usbip cancelled/timeout: {}",
                    String::from_utf8_lossy(&output)
                ));
            }
            thread::sleep(Duration::from_millis(2));
        }
    })();
    // SAFETY: negative pid addresses only the process group created above.
    if child.try_wait().ok().flatten().is_none() {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let reap_end = Instant::now() + Duration::from_millis(100);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < reap_end => thread::sleep(Duration::from_millis(1)),
            _ => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
                break;
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_launch_rejects_arbitrary_program_role_and_policy_paths() {
        let exe = Path::new("/var/lib/llfa-link-light-bridge/temporary-export/helper");
        let policy = Path::new("/run/llfa-link-light-bridge/temporary-export/policy.json");
        let args = temporary_service_arguments("export", exe, policy).unwrap();
        assert!(args.contains(&"--service-type=exec".into()));
        assert!(args.contains(&"--property=Restart=no".into()));
        assert!(args.contains(&"--collect".into()));
        assert_eq!(
            &args[args.len() - 3..],
            &[
                exe.to_str().unwrap(),
                "serve-temporary",
                policy.to_str().unwrap()
            ]
        );
        assert!(temporary_service_arguments("export;sh", exe, policy).is_err());
        assert!(temporary_service_arguments("export", Path::new("/usr/bin/sh"), policy).is_err());
        assert!(temporary_service_arguments("export", exe, Path::new("/tmp/user.json")).is_err());
        assert!(temporary_service_arguments("import", exe, policy).is_err());
    }
    #[test]
    fn noexec_is_detected_and_staging_paths_are_fixed() {
        assert!(executable_mount_flags(libc::ST_NOEXEC).is_err());
        executable_mount_flags(libc::ST_NODEV | libc::ST_NOSUID).unwrap();
        for role in ["export", "import"] {
            for auto in [false, true] {
                let path = temporary_executable(role, auto).unwrap();
                assert!(path.starts_with("/var/lib/llfa-link-light-bridge"));
                assert!(!path.starts_with("/run"));
            }
        }
        assert!(temporary_executable("../../tmp", true).is_err());
        assert!(temporary_service_arguments(
            "export",
            Path::new("/run/llfa-link-light-bridge/temporary-export/helper"),
            Path::new("/run/llfa-link-light-bridge/temporary-export/policy.json")
        )
        .is_err());
    }
    #[test]
    fn timeout_reaps_process_group_and_checks_exit() {
        let cancel = AtomicBool::new(false);
        let now = Instant::now();
        assert!(run_program(
            Path::new("/usr/bin/sleep"),
            &["5".into()],
            Duration::from_millis(10),
            &cancel
        )
        .unwrap_err()
        .contains("timeout"));
        assert!(now.elapsed() < Duration::from_secs(1));
        assert!(run_program(
            Path::new("/usr/bin/false"),
            &[],
            Duration::from_secs(1),
            &cancel
        )
        .unwrap_err()
        .contains("exit"));
    }
    #[test]
    fn args_are_fixed_and_ipv6_is_one_argument() {
        assert_eq!(
            Operation::Attach("[::1]:3240".parse().unwrap(), "1-3.3".into()).arguments(),
            [
                "--tcp-port",
                "3240",
                "attach",
                "--remote",
                "::1",
                "--busid",
                "1-3.3"
            ]
        );
    }
}
