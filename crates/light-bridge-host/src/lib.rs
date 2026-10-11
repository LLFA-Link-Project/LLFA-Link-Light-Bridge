//! WiVRn-process library. Runtime must release its tracking handles before stop.
use light_bridge_core::*;
use light_bridge_protocol::{Kind, Transport};
use light_bridge_usbip::{MockUsbIp, UsbDeviceIdentity, UsbTransferBackend};
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub struct HostServer {
    pub session: Session,
    transport: Transport,
    usb: Arc<Mutex<MockUsbIp>>,
    port: Option<u32>,
    prepare_requested: bool,
    last_control: u64,
    last_kind: Option<Kind>,
    stop_requested: bool,
    last_pose: u64,
}
impl HostServer {
    pub fn new(transport: Transport, usb: Arc<Mutex<MockUsbIp>>, timeout: Duration) -> Self {
        Self {
            session: Session::new(timeout),
            transport,
            usb,
            port: None,
            prepare_requested: false,
            last_control: 0,
            last_kind: None,
            stop_requested: false,
            last_pose: 0,
        }
    }
    pub fn start(&mut self, id: u64) -> Result<()> {
        if self.session.state != State::Idle {
            return self.session.start(id);
        }
        self.session.start(id)?;
        self.last_control = 0;
        self.last_kind = None;
        self.last_pose = 0;
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
    pub fn poll(&mut self) -> Result<()> {
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
                    if self.session.state != State::PreparingUsb {
                        return Err(Error::InvalidState);
                    }
                    let port = {
                        let mut usb = self.usb.lock().map_err(|_| Error::InvalidState)?;
                        let device = UsbDeviceIdentity::mock();
                        usb.observe_export_ready(m.session_id, &device)?;
                        usb.attach(m.session_id, &device)?
                    };
                    self.port = Some(port);
                    self.transport.send(Kind::UsbAttached, m.session_id, 4)?;
                    self.session.advance(m.session_id, State::UsbAttached)?;
                    self.session
                        .advance(m.session_id, State::TrackingInitializing)?;
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
        // Mock runtime has no real USB handles. Production adapter needs an explicit release barrier.
        if let Some(port) = self.port.take() {
            let _ = self.usb.lock().map(|mut u| u.detach(self.session.id, port));
        }
        // Both roles share an ownership ledger only in this mock backend.
        let _ = self.usb.lock().map(|mut u| u.unexport(self.session.id));
    }
}
