#[test]
fn generated_header_matches_checked_in_abi() {
    // Both inputs are compile-time reads: no Python or standalone generator executable.
    let generated = include_str!(env!("LLB_GENERATED_HEADER"));
    let snapshot = include_str!("../light_bridge.h");
    assert_eq!(
        generated, snapshot,
        "Refresh the ABI snapshot with LLB_HEADER_DIR=<absolute path to crates/light-bridge-ffi> cargo build -p light-bridge-ffi"
    );
}
