//! Headless Mock WiVRn client/server and Monado reference adapter harness.
use light_bridge_core::*;
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::mock_pair;
use light_bridge_usbip::MockUsbIp;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub struct MockRuntime {
    pub hmd: HmdClient,
    pub host: HostServer,
    pub usb: Arc<Mutex<MockUsbIp>>,
}
impl Default for MockRuntime {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}
impl MockRuntime {
    pub fn new(timeout: Duration) -> Self {
        let (a, b) = mock_pair();
        let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
        Self {
            hmd: HmdClient::new(a, usb.clone(), timeout),
            host: HostServer::new(b, usb.clone(), timeout),
            usb,
        }
    }
    pub fn connect(&mut self, id: u64, prepare_first: bool) -> Result<()> {
        self.host.start(id)?;
        if prepare_first {
            self.host.prepare_tracking()?;
        }
        // Mock WiVRn Client endpoint notification: a Rust DTO, not a real UDS integration.
        self.hmd.start(
            id,
            &EndpointInfo {
                addresses: vec!["127.0.0.1:4242".parse().unwrap()],
                host_id: 1,
                correlation_id: id,
            },
        )?;
        self.hmd.poll()?;
        self.host.poll()?;
        if !prepare_first {
            self.host.prepare_tracking()?;
        }
        self.hmd.poll()?;
        self.host.poll()?;
        self.hmd.poll()?;
        if self.host.state() != State::TrackingInitializing
            || self.hmd.state() != State::TrackingInitializing
        {
            return Err(Error::InvalidState);
        }
        self.host.tracking_ready()?;
        self.hmd.poll()?;
        Ok(())
    }
    pub fn pose(&mut self, id: u64) -> Result<ExternalPose> {
        self.hmd.set_mock_clock(
            1_000_000,
            ClockMapping {
                source_clock_id: 1,
                offset_ns: 100,
                uncertainty_ns: 10,
            },
        )?;
        let p = ExternalPose::mock(id, 1, 999_900);
        self.host.publish_pose(p)?;
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            if let Some(p) = self.hmd.take_reference_pose() {
                return Ok(p);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(Error::Timeout)
    }
    pub fn stop(&mut self) -> Result<()> {
        self.host.stop(true)?;
        self.hmd.poll()?;
        self.host.poll()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_to_hmd_monado_mock_reference_and_reconnect() {
        let mut r = MockRuntime::default();
        for id in 1..=3 {
            r.connect(id, id % 2 == 1).unwrap();
            r.host.prepare_tracking().unwrap();
            r.host.tracking_ready().unwrap();
            let p = r.pose(id).unwrap();
            assert_eq!(p.position_m, [1., 2., 3.]);
            assert_eq!(p.timestamp_semantics, 1);
            assert_eq!(r.host.publish_pose(p), Err(Error::Duplicate));
            assert_eq!(r.host.stop(false), Err(Error::Ownership));
            r.stop().unwrap();
            r.stop().unwrap();
            assert_eq!(r.hmd.state(), State::Idle);
            assert_eq!(r.host.state(), State::Idle);
            assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
            assert!(r.hmd.take_reference_pose().is_none());
        }
    }
    #[test]
    fn attach_failure_and_cleanup() {
        let mut r = MockRuntime::default();
        r.usb.lock().unwrap().fail_attach = true;
        assert_eq!(r.connect(1, true), Err(Error::UsbAttach));
        r.stop().unwrap();
        assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
    }
    #[test]
    fn enumeration_failure_and_export_failure() {
        for export in [false, true] {
            let mut r = MockRuntime::default();
            {
                let mut u = r.usb.lock().unwrap();
                if export {
                    u.fail_export = true;
                } else {
                    u.enumeration_ready = false;
                }
            }
            assert_eq!(
                r.connect(1, true),
                Err(if export {
                    Error::UsbExport
                } else {
                    Error::Enumeration
                })
            );
            r.stop().unwrap();
            assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
        }
    }
    #[test]
    fn hmd_cancel_requires_runtime_release() {
        let mut r = MockRuntime::default();
        r.connect(1, true).unwrap();
        r.hmd.request_stop().unwrap();
        r.hmd.request_stop().unwrap();
        r.host.poll().unwrap();
        assert!(r.host.needs_runtime_release());
        assert_eq!(
            r.host.publish_pose(ExternalPose::mock(1, 1, 1)),
            Err(Error::InvalidState)
        );
        r.stop().unwrap();
    }
    #[test]
    fn timeout_and_destroy_worker() {
        let mut r = MockRuntime::new(Duration::ZERO);
        r.host.start(1).unwrap();
        assert_eq!(r.host.poll(), Err(Error::Timeout));
        assert!(r.host.needs_runtime_release());
        let begin = Instant::now();
        drop(r);
        assert!(begin.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn disconnected_peer() {
        let r = MockRuntime::default();
        let MockRuntime { mut hmd, host, .. } = r;
        drop(host);
        assert_eq!(hmd.poll(), Err(Error::Disconnected));
    }
    #[test]
    fn heartbeat_loss_can_still_complete_ordered_cleanup() {
        let mut r = MockRuntime::new(Duration::from_millis(100));
        r.connect(1, true).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(r.hmd.poll(), Err(Error::Timeout));
        assert_eq!(r.host.poll(), Err(Error::Timeout));
        r.stop().unwrap();
        assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
        assert_eq!(r.hmd.state(), State::Idle);
        assert_eq!(r.host.state(), State::Idle);
    }
    #[test]
    fn active_drop_cleans_mock_ledger_in_both_orders() {
        for hmd_first in [true, false] {
            let mut r = MockRuntime::default();
            r.connect(1, true).unwrap();
            let MockRuntime { hmd, host, usb } = r;
            if hmd_first {
                drop(hmd);
                drop(host);
            } else {
                drop(host);
                drop(hmd);
            }
            assert_eq!(usb.lock().unwrap().resource_count(), 0);
        }
    }
}
