//! Fixed usbip invocation, bounded output, cancellation, process-group kill/reap.
use std::{
    fs,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::Path,
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
fn run_program(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(), String> {
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
        loop {
            drain(&mut stdout, &mut output)?;
            drain(&mut stderr, &mut output)?;
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                drain(&mut stdout, &mut output)?;
                drain(&mut stderr, &mut output)?;
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "usbip exit {status}: {}",
                        String::from_utf8_lossy(&output)
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
