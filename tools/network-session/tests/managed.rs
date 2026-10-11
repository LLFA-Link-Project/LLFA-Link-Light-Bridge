//! Actual USB manager worker paths with cancellable Mock operations, no privileged OS mutations.
use light_bridge_core::*;
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::mock_pair;
use light_bridge_usbip::{manager::ManagedUsb, MockUsbIp, UsbDeviceIdentity, UsbTransferBackend};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
struct SlowUsb {
    ledger: Arc<Mutex<MockUsbIp>>,
    cancel: Arc<AtomicBool>,
    delay: Duration,
    import: bool,
    gate: Option<Arc<AtomicBool>>,
    started: Option<Arc<AtomicBool>>,
}
impl SlowUsb {
    fn wait(&self) -> Result<()> {
        let end = Instant::now() + self.delay;
        while Instant::now() < end {
            if self.cancel.load(Ordering::Acquire) {
                return Err(Error::Disconnected);
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}
impl ManagedUsb for SlowUsb {
    fn set_cancellation(&mut self, c: Arc<AtomicBool>) {
        self.cancel = c;
    }
}
impl UsbTransferBackend for SlowUsb {
    fn export(&mut self, s: u64, d: &UsbDeviceIdentity) -> Result<()> {
        self.wait()?;
        self.ledger.lock().unwrap().export(s, d)
    }
    fn attach(&mut self, s: u64, d: &UsbDeviceIdentity) -> Result<u32> {
        if let Some(started) = &self.started {
            started.store(true, Ordering::Release);
        }
        if let Some(gate) = &self.gate {
            while !gate.load(Ordering::Acquire) {
                if self.cancel.load(Ordering::Acquire) {
                    return Err(Error::Disconnected);
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.wait()?;
        let mut b = self.ledger.lock().unwrap();
        if self.import {
            b.observe_export_ready(s, d)?;
        }
        b.attach(s, d)
    }
    fn detach(&mut self, s: u64, p: u32) -> Result<()> {
        self.wait()?;
        self.ledger.lock().unwrap().detach(s, p)
    }
    fn unexport(&mut self, s: u64) -> Result<()> {
        self.wait()?;
        self.ledger.lock().unwrap().unexport(s)
    }
}
fn setup(
    delay: Duration,
) -> (
    HmdClient,
    HostServer,
    Arc<Mutex<MockUsbIp>>,
    Arc<Mutex<MockUsbIp>>,
) {
    let (a, b) = mock_pair();
    let export = Arc::new(Mutex::new(MockUsbIp::ready()));
    let import = Arc::new(Mutex::new(MockUsbIp::remote_importer()));
    let backend = |ledger, import| {
        Box::new(SlowUsb {
            ledger,
            import,
            gate: None,
            started: None,
            cancel: Arc::new(AtomicBool::new(false)),
            delay,
        })
    };
    let hmd = HmdClient::with_usb_backend(
        a,
        backend(export.clone(), false),
        UsbDeviceIdentity::mock(),
        Duration::from_secs(2),
    )
    .unwrap();
    let host = HostServer::with_usb_backend(
        b,
        backend(import.clone(), true),
        UsbDeviceIdentity::mock(),
        Duration::from_secs(2),
    )
    .unwrap();
    (hmd, host, export, import)
}
fn start(h: &mut HmdClient, s: &mut HostServer) {
    s.start(1).unwrap();
    s.prepare_tracking().unwrap();
    h.start(
        1,
        &EndpointInfo {
            host_id: 17,
            correlation_id: 1,
            addresses: vec!["127.0.0.1:42".parse().unwrap()],
        },
    )
    .unwrap();
}
fn pump(h: &mut HmdClient, s: &mut HostServer, state: State) {
    let end = Instant::now() + Duration::from_secs(2);
    while h.state() != state || s.state() != state {
        let now = Instant::now();
        h.poll().unwrap();
        s.poll().unwrap();
        assert!(
            now.elapsed() < Duration::from_millis(30),
            "USB blocked control poll"
        );
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn worker_attach_pose_release_and_cleanup() {
    let (mut h, mut s, export, import) = setup(Duration::from_millis(30));
    start(&mut h, &mut s);
    pump(&mut h, &mut s, State::TrackingInitializing);
    s.tracking_ready().unwrap();
    pump(&mut h, &mut s, State::Active);
    h.set_mock_clock(
        1_000_000,
        ClockMapping {
            source_clock_id: 1,
            offset_ns: 0,
            uncertainty_ns: 10,
        },
    )
    .unwrap();
    s.publish_pose(ExternalPose::mock(1, 1, 1_000_000)).unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while h.take_reference_pose().is_none() {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(s.stop(false), Err(Error::Ownership));
    s.stop(true).unwrap();
    pump(&mut h, &mut s, State::Idle);
    assert_eq!(export.lock().unwrap().resource_count(), 0);
    assert_eq!(import.lock().unwrap().resource_count(), 0);
}
#[test]
fn stop_during_export_finishes_ordered_cleanup_without_ready() {
    let (mut h, mut s, export, import) = setup(Duration::from_millis(30));
    start(&mut h, &mut s);
    h.poll().unwrap();
    s.poll().unwrap();
    h.poll().unwrap();
    assert_eq!(h.state(), State::PreparingUsb);
    s.stop(true).unwrap();
    pump(&mut h, &mut s, State::Idle);
    assert_eq!(export.lock().unwrap().resource_count(), 0);
    assert_eq!(import.lock().unwrap().resource_count(), 0);
}
#[test]
fn destroy_cancels_inflight_wait_and_joins() {
    let (mut h, mut s, export, _) = setup(Duration::from_secs(10));
    start(&mut h, &mut s);
    h.poll().unwrap();
    s.poll().unwrap();
    h.poll().unwrap();
    thread::sleep(Duration::from_millis(5));
    let now = Instant::now();
    drop(h);
    assert!(now.elapsed() < Duration::from_secs(1));
    assert_eq!(export.lock().unwrap().resource_count(), 0);
}
#[test]
fn stop_during_attach_waits_for_owned_detach_then_unexport() {
    let (a, b) = mock_pair();
    let export = Arc::new(Mutex::new(MockUsbIp::ready()));
    let import = Arc::new(Mutex::new(MockUsbIp::remote_importer()));
    let gate = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let backend = |ledger, import, gate, started| {
        Box::new(SlowUsb {
            ledger,
            import,
            gate,
            started,
            cancel: Arc::new(AtomicBool::new(false)),
            delay: Duration::ZERO,
        })
    };
    let mut h = HmdClient::with_usb_backend(
        a,
        backend(export.clone(), false, None, None),
        UsbDeviceIdentity::mock(),
        Duration::from_secs(2),
    )
    .unwrap();
    let mut s = HostServer::with_usb_backend(
        b,
        backend(
            import.clone(),
            true,
            Some(gate.clone()),
            Some(started.clone()),
        ),
        UsbDeviceIdentity::mock(),
        Duration::from_secs(2),
    )
    .unwrap();
    start(&mut h, &mut s);
    let end = Instant::now() + Duration::from_secs(1);
    while !started.load(Ordering::Acquire) {
        h.poll().unwrap();
        s.poll().unwrap();
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(s.state(), State::PreparingUsb);
    s.stop(true).unwrap();
    gate.store(true, Ordering::Release);
    pump(&mut h, &mut s, State::Idle);
    assert_eq!(export.lock().unwrap().resource_count(), 0);
    assert_eq!(import.lock().unwrap().resource_count(), 0);
}
#[test]
fn mismatched_offer_is_rejected_before_import_worker() {
    let (a, b) = mock_pair();
    let ledger = Arc::new(Mutex::new(MockUsbIp::remote_importer()));
    let backend = Box::new(SlowUsb {
        ledger: ledger.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        delay: Duration::ZERO,
        import: true,
        gate: None,
        started: None,
    });
    let mut s = HostServer::with_usb_backend(
        b,
        backend,
        UsbDeviceIdentity::mock(),
        Duration::from_secs(2),
    )
    .unwrap();
    s.start(1).unwrap();
    s.prepare_tracking().unwrap();
    a.send(light_bridge_protocol::Kind::Hello, 1, 1).unwrap();
    s.poll().unwrap();
    let mut d = UsbDeviceIdentity::mock();
    d.serial = "OTHER-TRACKER".into();
    a.export_ready(1, d).unwrap();
    assert_eq!(s.poll(), Err(Error::DeviceDenied));
    assert_eq!(ledger.lock().unwrap().resource_count(), 0);
}
#[test]
fn stop_before_prepare_needs_no_helper_or_usb_operation() {
    let (mut h, mut s, export, import) = setup(Duration::from_secs(10));
    s.start(1).unwrap();
    h.start(
        1,
        &EndpointInfo {
            host_id: 17,
            correlation_id: 1,
            addresses: vec!["127.0.0.1:42".parse().unwrap()],
        },
    )
    .unwrap();
    h.poll().unwrap();
    s.poll().unwrap();
    s.stop(true).unwrap();
    pump(&mut h, &mut s, State::Idle);
    assert_eq!(export.lock().unwrap().resource_count(), 0);
    assert_eq!(import.lock().unwrap().resource_count(), 0);
}
