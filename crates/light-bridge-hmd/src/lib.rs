//! Monado-process library. Real LLFA adapter is intentionally not patched in Phase 1.
use light_bridge_core::*;
use light_bridge_protocol::{Kind, Message, Transport};
use light_bridge_usbip::{MockUsbIp, UsbDeviceIdentity, UsbTransferBackend};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
pub type SharedUsb = Arc<Mutex<MockUsbIp>>;
struct PoseState {
    now_ns: i64,
    session: u64,
    active: bool,
    mapping: ClockMapping,
    validator: PoseValidator,
    latest: Option<ExternalPose>,
    rejected: u64,
}
pub struct HmdClient {
    pub session: Session,
    transport: Transport,
    usb: SharedUsb,
    pose: Arc<Mutex<PoseState>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    last_control: u64,
    last_kind: Option<Kind>,
}
impl HmdClient {
    pub fn new(mut transport: Transport, usb: SharedUsb, timeout: Duration) -> Self {
        let pose = Arc::new(Mutex::new(PoseState {
            now_ns: 0,
            session: 0,
            active: false,
            mapping: ClockMapping {
                source_clock_id: 1,
                offset_ns: 0,
                uncertainty_ns: u64::MAX,
            },
            validator: PoseValidator::default(),
            latest: None,
            rejected: 0,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let p = pose.clone();
        let c = cancel.clone();
        let rx = transport.pose_rx.take().expect("unused pose receiver");
        let worker = thread::spawn(move || {
            while !c.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(b) => {
                        let mut state = p.lock().unwrap_or_else(|e| e.into_inner());
                        let sample = Message::decode(&b).ok().and_then(|m| {
                            if m.kind == Kind::Pose {
                                m.pose
                            } else {
                                None
                            }
                        });
                        if let Some(sample) = sample {
                            let session = state.session;
                            let map = state.mapping;
                            // Synthetic receiver clock is controlled explicitly by the mock adapter.
                            let now = state.now_ns;
                            if state.active
                                && state.validator.validate(&sample, session, now, map).is_ok()
                            {
                                state.latest = Some(sample);
                            } else {
                                state.rejected += 1;
                            }
                        } else {
                            state.rejected += 1;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
        });
        Self {
            session: Session::new(timeout),
            transport,
            usb,
            pose,
            cancel,
            worker: Some(worker),
            last_control: 0,
            last_kind: None,
        }
    }
    pub fn start(&mut self, id: u64, endpoint: &EndpointInfo) -> Result<()> {
        endpoint.validate()?;
        if endpoint.correlation_id != id {
            return Err(Error::WrongSession);
        }
        if self.session.state != State::Idle {
            return self.session.start(id);
        }
        self.session.start(id)?;
        self.last_control = 0;
        self.last_kind = None;
        {
            let mut p = self.pose.lock().map_err(|_| Error::InvalidState)?;
            p.session = id;
            p.mapping.uncertainty_ns = u64::MAX;
            p.active = false;
            p.latest = None;
            p.validator.reset();
        }
        self.transport.send(Kind::Hello, id, 1)
    }
    pub fn set_mock_clock(&mut self, now_ns: i64, mapping: ClockMapping) -> Result<()> {
        let mut p = self.pose.lock().map_err(|_| Error::InvalidState)?;
        p.now_ns = now_ns;
        if p.mapping.source_clock_id != mapping.source_clock_id
            || p.mapping.offset_ns != mapping.offset_ns
        {
            p.validator.reset();
            p.latest = None;
        }
        p.mapping = mapping;
        Ok(())
    }
    pub fn poll(&mut self) -> Result<()> {
        if self.session.expired() && self.session.state != State::Stopping {
            self.request_stop()?;
            return Err(Error::Timeout);
        }
        loop {
            let m = match self.transport.receive() {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    self.disable_pose();
                    self.session.fail();
                    return Err(e);
                }
            };
            // A retransmitted stop must still be acknowledged after cleanup.
            if m.kind == Kind::Stop
                && m.session_id == self.session.id
                && self.session.state == State::Idle
            {
                self.transport.send(Kind::Stopped, m.session_id, 7)?;
                continue;
            }
            self.session.check(m.session_id)?;
            if m.kind == Kind::Heartbeat {
                self.session.touch();
                continue;
            }
            if m.operation_id < self.last_control {
                return Err(Error::Duplicate);
            }
            if m.operation_id == self.last_control {
                if self.last_kind != Some(m.kind) {
                    return Err(Error::Protocol);
                }
                continue;
            }
            match m.kind {
                Kind::Hello => self.session.advance(m.session_id, State::Negotiated)?,
                Kind::Prepare => {
                    self.session.advance(m.session_id, State::PreparingUsb)?;
                    self.usb
                        .lock()
                        .map_err(|_| Error::InvalidState)?
                        .export(m.session_id, &UsbDeviceIdentity::mock())?;
                    self.transport.send(Kind::UsbExportReady, m.session_id, 3)?;
                }
                Kind::UsbAttached => {
                    self.session.advance(m.session_id, State::UsbAttached)?;
                    self.session
                        .advance(m.session_id, State::TrackingInitializing)?;
                }
                Kind::TrackingReady => {
                    self.session.advance(m.session_id, State::Active)?;
                    self.pose.lock().map_err(|_| Error::InvalidState)?.active = true;
                }
                Kind::Stop => {
                    self.session.stop(m.session_id)?;
                    self.disable_pose();
                    self.usb
                        .lock()
                        .map_err(|_| Error::InvalidState)?
                        .unexport(m.session_id)?;
                    self.transport.send(Kind::Stopped, m.session_id, 7)?;
                    self.session.cleaned();
                }
                _ => return Err(Error::InvalidState),
            }
            self.last_control = m.operation_id;
            self.last_kind = Some(m.kind);
        }
        Ok(())
    }
    fn disable_pose(&mut self) {
        let mut p = self.pose.lock().unwrap_or_else(|e| e.into_inner());
        p.active = false;
        p.latest = None;
        p.validator.reset();
    }
    pub fn request_stop(&mut self) -> Result<()> {
        if self.session.state == State::Idle {
            return Ok(());
        }
        self.session.stop(self.session.id)?;
        self.disable_pose();
        self.transport.send(Kind::StopRequest, self.session.id, 6)
    }
    /// Borrow-free latest reference tracker pose, never an uncalibrated head pose.
    pub fn take_reference_pose(&mut self) -> Option<ExternalPose> {
        let mut p = self.pose.lock().ok()?;
        let sample = p.latest.take()?;
        let time = sample
            .sample_timestamp_ns
            .checked_add(p.mapping.offset_ns)?;
        let age = p.now_ns.checked_sub(time)?;
        if !p.active
            || sample
                .clock_uncertainty_ns
                .saturating_add(p.mapping.uncertainty_ns)
                > p.validator.policy.max_uncertainty_ns
            || age > p.validator.policy.max_age_ns
            || age < -p.validator.policy.future_tolerance_ns
        {
            p.rejected += 1;
            return None;
        }
        Some(sample)
    }
    pub fn heartbeat(&self) -> Result<()> {
        self.transport.send(Kind::Heartbeat, self.session.id, 8)
    }
    pub fn rejected_poses(&self) -> u64 {
        self.pose.lock().map(|p| p.rejected).unwrap_or(0)
    }
    pub fn state(&self) -> State {
        self.session.state
    }
}
impl Drop for HmdClient {
    fn drop(&mut self) {
        self.disable_pose();
        self.cancel.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        let _ = self.usb.lock().map(|mut u| u.unexport(self.session.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use light_bridge_protocol::mock_pair;
    use std::time::Instant;
    fn setup() -> (HmdClient, Transport, SharedUsb) {
        let (a, b) = mock_pair();
        let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
        let mut h = HmdClient::new(a, usb.clone(), Duration::from_secs(2));
        h.start(
            1,
            &EndpointInfo {
                addresses: vec!["[::1]:4242".parse().unwrap()],
                host_id: 1,
                correlation_id: 1,
            },
        )
        .unwrap();
        b.receive().unwrap();
        for (kind, op) in [
            (Kind::Hello, 1),
            (Kind::Prepare, 2),
            (Kind::UsbAttached, 4),
            (Kind::TrackingReady, 5),
        ] {
            b.send(kind, 1, op).unwrap();
            h.poll().unwrap();
        }
        h.set_mock_clock(
            1_000_000,
            ClockMapping {
                source_clock_id: 1,
                offset_ns: 0,
                uncertainty_ns: 10,
            },
        )
        .unwrap();
        (h, b, usb)
    }
    fn wait_rejections(h: &HmdClient, n: u64) {
        let end = Instant::now() + Duration::from_secs(1);
        while h.rejected_poses() < n && Instant::now() < end {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(h.rejected_poses(), n);
    }
    #[test]
    fn worker_rejects_old_session_stale_lost_uncertain_and_origin_change() {
        let (mut h, b, _) = setup();
        let mut p = ExternalPose::mock(2, 1, 1_000_000);
        b.publish(p).unwrap();
        wait_rejections(&h, 1);
        p.session_id = 1;
        p.sample_timestamp_ns = -200_000_000;
        b.publish(p).unwrap();
        wait_rejections(&h, 2);
        p.sample_timestamp_ns = 1_000_000;
        p.flags = 0;
        b.publish(p).unwrap();
        wait_rejections(&h, 3);
        p.flags = 7;
        p.clock_uncertainty_ns = 100_000_000;
        b.publish(p).unwrap();
        wait_rejections(&h, 4);
        p.clock_uncertainty_ns = 0;
        b.publish(p).unwrap();
        let end = Instant::now() + Duration::from_secs(1);
        let received = loop {
            if let Some(v) = h.take_reference_pose() {
                break v;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(received, p);
        p.sequence = 2;
        p.tracking_origin_id = 2;
        b.publish(p).unwrap();
        wait_rejections(&h, 5);
        p.tracking_origin_id = 1;
        p.sample_timestamp_ns = 999_999;
        b.publish(p).unwrap();
        wait_rejections(&h, 6);
        p.sample_timestamp_ns = 1_000_000;
        p.sequence = 1;
        b.publish(p).unwrap();
        wait_rejections(&h, 7);
        assert!(h.take_reference_pose().is_none());
    }
    #[test]
    fn duplicate_control_and_stop_ack() {
        let (mut h, b, usb) = setup();
        b.send(Kind::TrackingReady, 1, 5).unwrap();
        h.poll().unwrap();
        b.send(Kind::UsbAttached, 1, 5).unwrap();
        assert_eq!(h.poll(), Err(Error::Protocol));
        b.send(Kind::Stop, 2, 6).unwrap();
        assert_eq!(h.poll(), Err(Error::WrongSession));
        b.send(Kind::Stop, 1, 6).unwrap();
        h.poll().unwrap();
        assert_eq!(h.state(), State::Idle);
        b.send(Kind::Stop, 1, 6).unwrap();
        h.poll().unwrap();
        assert_eq!(usb.lock().unwrap().resource_count(), 0);
    }
    #[test]
    fn disconnect_invalidates_buffer_and_joins_worker() {
        let (mut h, b, usb) = setup();
        drop(b);
        assert_eq!(h.poll(), Err(Error::Disconnected));
        assert!(h.take_reference_pose().is_none());
        let begin = Instant::now();
        drop(h);
        assert!(begin.elapsed() < Duration::from_secs(1));
        assert_eq!(usb.lock().unwrap().resource_count(), 0);
    }
}
