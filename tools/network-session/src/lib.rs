//! Phase 2 runtime adapter stand-ins. Real UDS/TCP, independent Mock USB ledgers.
//! These adapters are a test harness, not implementations of upstream runtime hooks.
#![cfg(unix)]
use light_bridge_core::*;
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::{
    ipc::{self, Notification},
    network,
};
use light_bridge_usbip::MockUsbIp;
use std::{
    net::TcpListener,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
const SETUP_TIMEOUT: Duration = Duration::from_secs(2);

pub struct WivrnClientAdapter;
impl WivrnClientAdapter {
    /// Called as soon as Bridge port/host/correlation are known, before tracking init.
    pub fn connected(stream: &mut UnixStream, endpoint: EndpointInfo) -> Result<()> {
        ipc::send(stream, &Notification::Start(endpoint), SETUP_TIMEOUT)
    }
    pub fn disconnected(stream: &mut UnixStream, endpoint: &EndpointInfo) -> Result<()> {
        ipc::send(
            stream,
            &Notification::Stop {
                host_id: endpoint.host_id,
                correlation_id: endpoint.correlation_id,
            },
            SETUP_TIMEOUT,
        )
    }
}
pub struct MonadoAdapter {
    pub bridge: HmdClient,
    host_id: u64,
    pub reference: Option<ExternalPose>,
    pub head_pose_valid: bool,
}
impl MonadoAdapter {
    pub fn from_connected(bridge: HmdClient, host_id: u64) -> Self {
        Self {
            bridge,
            host_id,
            reference: None,
            head_pose_valid: false,
        }
    }

    pub fn notification(&mut self, n: Notification) -> Result<()> {
        match n {
            Notification::Start(endpoint) => {
                if endpoint.host_id != self.host_id {
                    return Err(Error::WrongSession);
                }
                self.bridge.start(endpoint.correlation_id, &endpoint)
            }
            Notification::Stop {
                host_id,
                correlation_id,
            } => {
                if host_id != self.host_id || correlation_id != self.bridge.session.id {
                    return Err(Error::WrongSession);
                }
                self.reference = None;
                self.bridge.request_stop()
            }
        }
    }
    pub fn poll(&mut self) -> Result<()> {
        let result = self.bridge.poll();
        if self.bridge.state() != State::Active {
            self.reference = None;
        }
        if let Some(p) = self.bridge.take_reference_pose() {
            self.reference = Some(p);
        }
        result
    }
}
pub struct WivrnServerAdapter {
    pub bridge: HostServer,
    pub tracking_handles_open: bool,
    pub tracking_initializations: usize,
}
impl WivrnServerAdapter {
    /// Preparation is separate from runtime enumeration; early and late notification work.
    pub fn prepare_before_enumeration(&mut self) -> Result<()> {
        self.bridge.prepare_tracking()
    }
    pub fn enumerate_after_attach(&mut self) -> Result<()> {
        if self.bridge.state() != State::TrackingInitializing {
            return Err(Error::InvalidState);
        }
        self.tracking_handles_open = true;
        self.tracking_initializations += 1;
        self.bridge.tracking_ready()
    }
    pub fn release_and_stop(&mut self) -> Result<()> {
        self.tracking_handles_open = false;
        self.bridge.stop(!self.tracking_handles_open)
    }
}
pub struct NetworkRuntime {
    pub monado: MonadoAdapter,
    pub server: WivrnServerAdapter,
    pub usb: Arc<Mutex<MockUsbIp>>,
    pub host_usb: Arc<Mutex<MockUsbIp>>,
    client_ipc: UnixStream,
    monado_ipc: UnixStream,
    pub endpoint: EndpointInfo,
}
impl NetworkRuntime {
    pub fn new(timeout: Duration) -> Result<Self> {
        // Independent ledgers communicate export readiness over TCP. All USB remains synthetic.
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| Error::Disconnected)?;
        let endpoint = EndpointInfo {
            addresses: vec![listener.local_addr().map_err(|_| Error::Disconnected)?],
            host_id: 17,
            correlation_id: 1,
        };
        let (mut client_ipc, mut monado_ipc) =
            UnixStream::pair().map_err(|_| Error::Disconnected)?;
        WivrnClientAdapter::connected(&mut client_ipc, endpoint.clone())?;
        let notified = match ipc::receive(&mut monado_ipc, SETUP_TIMEOUT)? {
            Notification::Start(e) => e,
            _ => return Err(Error::Protocol),
        };
        let expected = endpoint.clone();
        let accept = thread::spawn(move || network::accept(&listener, &expected, SETUP_TIMEOUT));
        let hmd_transport = network::connect(&notified, SETUP_TIMEOUT);
        let host_transport = accept.join().map_err(|_| Error::InvalidState)??;
        let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
        let host_usb = Arc::new(Mutex::new(MockUsbIp::remote_importer()));
        Ok(Self {
            monado: MonadoAdapter {
                bridge: HmdClient::new(hmd_transport?, usb.clone(), timeout),
                host_id: endpoint.host_id,
                reference: None,
                head_pose_valid: false,
            },
            server: WivrnServerAdapter {
                bridge: HostServer::new(host_transport, host_usb.clone(), timeout),
                tracking_handles_open: false,
                tracking_initializations: 0,
            },
            usb,
            host_usb,
            client_ipc,
            monado_ipc,
            endpoint,
        })
    }
    pub fn pump_until(&mut self, predicate: impl Fn(&Self) -> bool) -> Result<()> {
        let end = Instant::now() + SETUP_TIMEOUT;
        while !predicate(self) {
            self.monado.poll()?;
            self.server.bridge.poll()?;
            if Instant::now() >= end {
                return Err(Error::Timeout);
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    pub fn connect_session(&mut self, id: u64, prepare_first: bool) -> Result<()> {
        self.endpoint.correlation_id = id;
        self.server.bridge.start(id)?;
        if prepare_first {
            self.server.prepare_before_enumeration()?;
        }
        WivrnClientAdapter::connected(&mut self.client_ipc, self.endpoint.clone())?;
        self.monado
            .notification(ipc::receive(&mut self.monado_ipc, SETUP_TIMEOUT)?)?;
        if !prepare_first {
            self.pump_until(|r| {
                r.server.bridge.state() == State::Negotiated
                    && r.monado.bridge.state() == State::Negotiated
            })?;
            self.server.prepare_before_enumeration()?;
        }
        self.pump_until(|r| {
            r.server.bridge.state() == State::TrackingInitializing
                && r.monado.bridge.state() == State::TrackingInitializing
        })?;
        self.server.enumerate_after_attach()?;
        self.pump_until(|r| r.monado.bridge.state() == State::Active)
    }
    pub fn deliver_pose(&mut self) -> Result<ExternalPose> {
        self.monado.bridge.set_mock_clock(
            1_000_000,
            ClockMapping {
                source_clock_id: 1,
                offset_ns: 100,
                uncertainty_ns: 10,
            },
        )?;
        self.server.bridge.publish_pose(ExternalPose::mock(
            self.endpoint.correlation_id,
            1,
            999_900,
        ))?;
        self.pump_until(|r| r.monado.reference.is_some())?;
        self.monado.reference.ok_or(Error::InvalidPose)
    }
    pub fn stop(&mut self, from_client: bool) -> Result<()> {
        if from_client {
            WivrnClientAdapter::disconnected(&mut self.client_ipc, &self.endpoint)?;
            self.monado
                .notification(ipc::receive(&mut self.monado_ipc, SETUP_TIMEOUT)?)?;
            self.pump_until(|r| r.server.bridge.needs_runtime_release())?;
        }
        self.server.release_and_stop()?;
        self.pump_until(|r| {
            r.monado.bridge.state() == State::Idle && r.server.bridge.state() == State::Idle
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_sockets_adapter_lifecycle_both_orders_reconnect_and_cleanup() {
        let mut r = NetworkRuntime::new(Duration::from_secs(2)).unwrap();
        assert_eq!(r.server.enumerate_after_attach(), Err(Error::InvalidState));
        for id in 1..=3 {
            r.connect_session(id, id % 2 == 1).unwrap();
            let p = r.deliver_pose().unwrap();
            assert_eq!(p.position_m, [1., 2., 3.]);
            assert_eq!(p.session_id, id);
            assert!(!r.monado.head_pose_valid);
            assert_eq!(r.server.bridge.stop(false), Err(Error::Ownership));
            assert_eq!(
                r.monado.notification(Notification::Stop {
                    host_id: 17,
                    correlation_id: id + 1
                }),
                Err(Error::WrongSession)
            );
            r.stop(id % 2 == 1).unwrap();
            assert!(!r.server.tracking_handles_open);
            assert!(r.monado.reference.is_none());
            assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
            assert_eq!(r.host_usb.lock().unwrap().resource_count(), 0);
        }
        assert_eq!(r.server.tracking_initializations, 3);
    }
    #[test]
    fn independent_attach_failure_clears_offer_and_reconnects() {
        let mut r = NetworkRuntime::new(Duration::from_secs(2)).unwrap();
        r.host_usb.lock().unwrap().fail_attach = true;
        assert_eq!(r.connect_session(1, true), Err(Error::UsbAttach));
        r.stop(false).unwrap();
        assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
        assert_eq!(r.host_usb.lock().unwrap().resource_count(), 0);
        r.host_usb.lock().unwrap().fail_attach = false;
        r.connect_session(2, false).unwrap();
        r.deliver_pose().unwrap();
        r.stop(true).unwrap();
        assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
        assert_eq!(r.host_usb.lock().unwrap().resource_count(), 0);
    }
    #[test]
    fn timeout_requests_runtime_release_then_cleans() {
        let mut r = NetworkRuntime::new(Duration::from_secs(1)).unwrap();
        r.connect_session(1, true).unwrap();
        // Expire the actual library session deadlines, not a mocked poll result.
        thread::sleep(Duration::from_millis(1100));
        assert_eq!(r.server.bridge.poll(), Err(Error::Timeout));
        assert!(r.server.bridge.needs_runtime_release());
        assert_eq!(r.monado.bridge.poll(), Err(Error::Timeout));
        r.stop(false).unwrap();
        assert_eq!(r.usb.lock().unwrap().resource_count(), 0);
        assert_eq!(r.host_usb.lock().unwrap().resource_count(), 0);
    }
    #[test]
    fn disconnect_invalidates_pose_and_join_is_bounded() {
        let mut r = NetworkRuntime::new(Duration::from_secs(2)).unwrap();
        r.connect_session(1, true).unwrap();
        r.deliver_pose().unwrap();
        let NetworkRuntime {
            mut monado,
            server,
            usb,
            ..
        } = r;
        drop(server);
        let end = Instant::now() + SETUP_TIMEOUT;
        loop {
            match monado.poll() {
                Err(Error::Disconnected) => break,
                Ok(()) => {}
                other => panic!("{other:?}"),
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(monado.reference.is_none());
        let start = Instant::now();
        drop(monado);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(usb.lock().unwrap().resource_count(), 0);
    }
}
