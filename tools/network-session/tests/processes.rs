#[cfg(unix)]
#[test]
fn separate_processes_uds_tcp_and_independent_usb_ledgers() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_network-session"))
        .arg("--processes")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("head_valid=false"));
    assert!(output.contains("HOST resources=0"));
    assert!(output.contains("HMD resources=0"));
}
