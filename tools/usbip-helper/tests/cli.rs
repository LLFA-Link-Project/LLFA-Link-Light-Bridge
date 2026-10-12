#![cfg(target_os = "linux")]
use std::process::Command;

#[test]
fn cli_requires_explicit_development_approval_and_root_without_mutating() {
    let binary = env!("CARGO_BIN_EXE_usbip-helper");
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout)
        .contains(light_bridge_usbip::helper::COMPATIBILITY_ID));
    let unapproved = Command::new(binary)
        .args(["temporary-start", "/nonexistent-policy.json"])
        .output()
        .unwrap();
    assert!(!unapproved.status.success());
    assert!(String::from_utf8_lossy(&unapproved.stderr).contains("Usage"));
    // A regular user cannot reach policy/staging/systemd operations, even with acknowledgement.
    if unsafe { libc::geteuid() } != 0 {
        let denied = Command::new(binary)
            .args([
                "temporary-start",
                "/nonexistent-policy.json",
                "--acknowledge-development-build",
            ])
            .output()
            .unwrap();
        assert!(!denied.status.success());
        assert!(String::from_utf8_lossy(&denied.stderr).contains("explicit sudo authorization"));
    }
    let stop = Command::new(binary)
        .args(["temporary-stop", "/nonexistent-policy.json"])
        .output()
        .unwrap();
    assert!(!stop.status.success());
    assert!(String::from_utf8_lossy(&stop.stderr).contains("Usage"));
}
