#[cfg(unix)]
mod processes;
#[cfg(unix)]
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|s| s == "--host") {
        return processes::host();
    }
    if args.first().is_some_and(|s| s == "--hmd") {
        return processes::hmd(std::path::Path::new(args.get(1).expect("UDS path")));
    }
    if args.first().is_some_and(|s| s == "--processes") {
        return processes::run();
    }
    use network_session::NetworkRuntime;
    use std::time::Duration;
    println!("REAL UDS/TCP, MOCK USB/pose/runtime adapters; same-process harness.");
    let mut runtime = NetworkRuntime::new(Duration::from_secs(2)).expect("socket setup");
    for id in 1..=3 {
        runtime
            .connect_session(id, id % 2 == 1)
            .expect("prepare/attach/ready");
        let pose = runtime.deliver_pose().expect("Monado reference delivery");
        runtime
            .stop(id % 2 == 1)
            .expect("release/detach/unexport/ack");
        println!(
            "session={id} reference={:?} head_valid={} resources={}",
            pose.position_m,
            runtime.monado.head_pose_valid,
            runtime.usb.lock().unwrap().resource_count()
        );
    }
}
#[cfg(not(unix))]
fn main() {
    eprintln!("UDS test harness requires Unix");
    std::process::exit(1);
}
