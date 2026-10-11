fn main() {
    let mut runtime = mock_session::MockRuntime::default();
    runtime.connect(1, true).expect("mock preparation");
    let pose = runtime.pose(1).expect("mock reference pose");
    println!(
        "Host -> HMD -> Mock Monado reference adapter: {:?}, session={}, sequence={}",
        pose.position_m, pose.session_id, pose.sequence
    );
    runtime.stop().expect("ordered cleanup");
    assert_eq!(runtime.usb.lock().unwrap().resource_count(), 0);
    println!("Stopped; mock resources=0 (no real USB or runtime integration)");
}
