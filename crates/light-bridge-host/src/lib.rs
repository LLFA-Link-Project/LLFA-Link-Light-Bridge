//! WiVRn-process library. Runtime must release its tracking handles before stop.
use light_bridge_core::clock::{monotonic_ns, TimingConfig};
use light_bridge_core::*;
use light_bridge_protocol::{Kind, Transport};
use light_bridge_usbip::{
    manager::{Kind as UsbKind, ManagedUsb, Task, UsbManager},
    MockUsbIp, UsbDeviceIdentity, UsbTransferBackend,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub struct HostServer {
    pub session: Session,
    transport: Transport,
    usb: Arc<Mutex<MockUsbIp>>,
    port: Option<u32>,
    managed: Option<UsbManager>,
    selected: UsbDeviceIdentity,
    pending_attach: bool,
    pending_detach: bool,
    prepare_requested: bool,
    last_control: u64,
    last_kind: Option<Kind>,
    stop_requested: bool,
    last_pose: u64,
    timing: Option<TimingConfig>,
    last_clock_request: u64,
}
impl HostServer {
    pub fn new(transport: Transport, usb: Arc<Mutex<MockUsbIp>>, timeout: Duration) -> Self {
        Self {
            session: Session::new(timeout),
            transport,
            usb,
            port: None,
            managed: None,
            selected: UsbDeviceIdentity::mock(),
            pending_attach: false,
            pending_detach: false,
            prepare_requested: false,
            last_control: 0,
            last_kind: None,
            stop_requested: false,
            last_pose: 0,
            timing: None,
            last_clock_request: 0,
        }
    }
    pub fn with_usb_backend(
        transport: Transport,
        backend: Box<dyn ManagedUsb>,
        device: UsbDeviceIdentity,
        timeout: Duration,
    ) -> Result<Self> {
        if (device.vid, device.pid) == (0x2c30, 0x1050) {
            return Err(Error::DeviceDenied);
        }
        let mut server = Self::new(
            transport,
            Arc::new(Mutex::new(MockUsbIp::default())),
            timeout,
        );
        server.managed = Some(UsbManager::new(backend));
        server.selected = device;
        Ok(server)
    }
    fn attached(&mut self, session: u64, port: u32) -> Result<()> {
        self.port = Some(port);
        if self.session.state == State::Stopping {
            return self.stop(true);
        }
        self.transport.send(Kind::UsbAttached, session, 4)?;
        self.session.advance(session, State::UsbAttached)?;
        self.session.advance(session, State::TrackingInitializing)
    }
    fn poll_usb(&mut self) -> Result<()> {
        while let Some(done) = match &self.managed {
            Some(m) => m.poll()?,
            None => None,
        } {
            self.session.check(done.session)?;
            match done.kind {
                UsbKind::Attach => self.pending_attach = false,
                UsbKind::Detach => self.pending_detach = false,
                _ => return Err(Error::InvalidState),
            };
            let result = done.result?;
            match done.kind {
                UsbKind::Attach => self.attached(done.session, result.ok_or(Error::UsbAttach)?)?,
                UsbKind::Detach => {
                    self.port = None;
                    self.transport.send(Kind::Stop, done.session, 6)?;
                }
                _ => return Err(Error::InvalidState),
            }
        }
        Ok(())
    }
    pub fn start(&mut self, id: u64) -> Result<()> {
        if self.session.state != State::Idle {
            return self.session.start(id);
        }
        self.session.start(id)?;
        self.last_control = 0;
        self.last_kind = None;
        self.last_pose = 0;
        self.last_clock_request = 0;
        self.prepare_requested = false;
        self.stop_requested = false;
        self.transport.send(Kind::Hello, id, 1)
    }
    pub fn prepare_tracking(&mut self) -> Result<()> {
        if self.session.state == State::Idle {
            return Err(Error::InvalidState);
        }
        self.prepare_requested = true;
        self.try_prepare()
    }
    fn try_prepare(&mut self) -> Result<()> {
        if self.prepare_requested && self.session.state == State::Negotiated {
            self.transport.send(Kind::Prepare, self.session.id, 2)?;
            self.session.advance(self.session.id, State::PreparingUsb)?;
        }
        Ok(())
    }
    pub fn configure_timing(&mut self, config: TimingConfig) -> Result<()> {
        if self.session.state != State::Idle {
            return Err(Error::InvalidState);
        }
        config.validate()?;
        monotonic_ns()?;
        self.timing = Some(config);
        Ok(())
    }
    pub fn poll(&mut self) -> Result<()> {
        self.poll_usb()?;
        if self.session.expired() && self.session.state != State::Stopping {
            self.stop_requested = true;
            return Err(Error::Timeout);
        }
        loop {
            let m = match self.transport.receive() {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(e) => {
                    self.stop_requested = true;
                    self.session.fail();
                    return Err(e);
                }
            };
            if m.kind == Kind::Stopped
                && m.session_id == self.session.id
                && self.session.state == State::Idle
            {
                continue;
            }
            self.session.check(m.session_id)?;
            if self.session.state == State::Stopping && m.kind == Kind::Hello {
                continue;
            }
            if m.kind == Kind::ClockRequest {
                if matches!(self.session.state, State::Stopping | State::Failed) {
                    continue;
                }
                let config = self.timing.ok_or(Error::InvalidState)?;
                let mut x = m.clock.ok_or(Error::Protocol)?;
                if x.source_clock_id != config.source_clock_id
                    || x.local_clock_id != config.local_clock_id
                {
                    return Err(Error::Protocol);
                }
                if m.operation_id <= self.last_clock_request {
                    continue;
                }
                x.t2 = monotonic_ns()?;
                x.t3 = monotonic_ns()?;
                match self
                    .transport
                    .clock(Kind::ClockReply, m.session_id, m.operation_id, x)
                {
                    Ok(()) => self.last_clock_request = m.operation_id,
                    Err(Error::Backpressure) => {}
                    Err(e) => return Err(e),
                }
                continue;
            }
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
                Kind::Hello => {
                    self.session.advance(m.session_id, State::Negotiated)?;
                    self.try_prepare()?;
                }
                Kind::UsbExportReady => {
                    if self.session.state == State::Stopping {
                        continue;
                    }
                    if self.session.state != State::PreparingUsb {
                        return Err(Error::InvalidState);
                    }
                    let device = m.device.ok_or(Error::Protocol)?;
                    if device != self.selected {
                        return Err(Error::DeviceDenied);
                    }
                    if let Some(manager) = &self.managed {
                        manager.submit(Task::Attach(m.session_id, device))?;
                        self.pending_attach = true;
                    } else {
                        let port = {
                            let mut usb = self.usb.lock().map_err(|_| Error::InvalidState)?;
                            usb.observe_export_ready(m.session_id, &device)?;
                            usb.attach(m.session_id, &device)?
                        };
                        self.attached(m.session_id, port)?;
                    }
                }
                Kind::Stopped => {
                    if self.session.state != State::Stopping {
                        return Err(Error::InvalidState);
                    }
                    self.session.cleaned();
                }
                Kind::StopRequest => {
                    self.stop_requested = true;
                }
                _ => return Err(Error::InvalidState),
            }
            self.last_control = m.operation_id;
            self.last_kind = Some(m.kind);
        }
        Ok(())
    }
    pub fn tracking_ready(&mut self) -> Result<()> {
        if self.session.state == State::Active {
            return Ok(());
        }
        if self.session.state != State::TrackingInitializing {
            return Err(Error::InvalidState);
        }
        self.transport
            .send(Kind::TrackingReady, self.session.id, 5)?;
        self.session.advance(self.session.id, State::Active)
    }
    pub fn publish_pose(&mut self, p: ExternalPose) -> Result<()> {
        if self.session.state != State::Active || self.stop_requested {
            return Err(Error::InvalidState);
        }
        if p.session_id != self.session.id {
            return Err(Error::WrongSession);
        }
        if p.sequence <= self.last_pose {
            return Err(Error::Duplicate);
        }
        if let Some(config) = self.timing {
            if p.tracked_device_id != config.tracked_device_id
                || p.source_clock_id != config.source_clock_id
                || p.sample_timestamp_ns < 0
                || p.timestamp_semantics > 2
            {
                return Err(Error::InvalidPose);
            }
        }
        self.transport.publish(p)?;
        self.last_pose = p.sequence;
        Ok(())
    }
    pub fn stop(&mut self, runtime_released: bool) -> Result<()> {
        if self.session.state == State::Idle {
            return Ok(());
        }
        if !runtime_released {
            return Err(Error::Ownership);
        }
        self.stop_requested = true;
        self.session.stop(self.session.id)?;
        if let Some(manager) = &self.managed {
            if self.pending_attach || self.pending_detach {
                return Ok(());
            }
            if let Some(port) = self.port {
                manager.submit(Task::Detach(self.session.id, port))?;
                self.pending_detach = true;
                return Ok(());
            }
            return self.transport.send(Kind::Stop, self.session.id, 6);
        }
        if let Some(port) = self.port {
            self.usb
                .lock()
                .map_err(|_| Error::InvalidState)?
                .detach(self.session.id, port)?;
            self.port = None;
        }
        self.usb
            .lock()
            .map_err(|_| Error::InvalidState)?
            .cancel_remote_offer(self.session.id)?;
        self.transport.send(Kind::Stop, self.session.id, 6)
    }
    pub fn needs_runtime_release(&self) -> bool {
        self.stop_requested
    }
    pub fn heartbeat(&self) -> Result<()> {
        self.transport.send(Kind::Heartbeat, self.session.id, 8)
    }
    pub fn state(&self) -> State {
        self.session.state
    }
}
impl Drop for HostServer {
    fn drop(&mut self) {
        if self.managed.is_some() {
            // Never detach real USB on Drop without the runtime release barrier.
            self.managed.take();
            return;
        }
        // Mock runtime has no real USB handles. Production adapter needs an explicit release barrier.
        if let Some(port) = self.port.take() {
            let _ = self.usb.lock().map(|mut u| u.detach(self.session.id, port));
        }
        // Both roles share an ownership ledger only in this mock backend.
        let _ = self.usb.lock().map(|mut u| u.unexport(self.session.id));
    }
}
