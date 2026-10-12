#![cfg(target_os = "linux")]
use light_bridge_usbip::{
    auto_worker::{Backend, Worker},
    bootstrap_rpc::{Action, Envelope, Response},
    kernel::{KResult, Role},
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
struct Slow(Arc<AtomicBool>);
impl Backend for Slow {
    fn transact(&mut self, _: Envelope, _: Option<i32>, cancel: &AtomicBool) -> KResult<Response> {
        self.0.store(true, Ordering::Release);
        while !cancel.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err("cancelled; root ownership requires inspection".into())
    }
}
#[test]
fn bounded_queue_cancel_join_is_independent_of_slow_setup() {
    let entered = Arc::new(AtomicBool::new(false));
    let worker = Worker::with_backend(Slow(entered.clone()));
    worker
        .submit(1, Envelope::new(Role::Export, Action::Describe), None)
        .unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while !entered.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    for id in 2..6 {
        worker
            .submit(id, Envelope::new(Role::Export, Action::Describe), None)
            .unwrap();
    }
    assert!(worker
        .submit(6, Envelope::new(Role::Export, Action::Describe), None)
        .unwrap_err()
        .contains("full"));
    assert!(worker.poll().unwrap().is_none());
    let start = Instant::now();
    drop(worker);
    assert!(start.elapsed() < Duration::from_millis(200));
}
