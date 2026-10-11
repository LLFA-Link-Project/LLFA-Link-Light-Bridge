//! Versioned frames with bounded in-memory and two-lane TCP transports.
#[cfg(unix)]
pub mod ipc;
pub mod network;
use light_bridge_core::{Error, ExternalPose, Result, VERSION};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;
pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_FRAME: usize = 1024;
pub const QUEUE_CAPACITY: usize = 16;
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hello = 1,
    Prepare = 2,
    UsbExportReady = 3,
    UsbAttached = 4,
    TrackingReady = 5,
    Stop = 6,
    Stopped = 7,
    Heartbeat = 8,
    StopRequest = 9,
    Pose = 10,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub kind: Kind,
    pub session_id: u64,
    pub operation_id: u64,
    pub pose: Option<ExternalPose>,
    pub device: Option<light_bridge_usbip::UsbDeviceIdentity>,
}
impl Message {
    pub fn control(kind: Kind, session_id: u64, operation_id: u64) -> Self {
        Self {
            kind,
            session_id,
            operation_id,
            pose: None,
            device: None,
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.session_id == 0
            || self.operation_id == 0
            || (self.kind == Kind::Pose) != self.pose.is_some()
            || (self.kind == Kind::UsbExportReady) != self.device.is_some()
        {
            return Err(Error::Protocol);
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"LLFB");
        b.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        b.extend_from_slice(&(self.kind as u32).to_be_bytes());
        b.extend_from_slice(&self.session_id.to_be_bytes());
        b.extend_from_slice(&self.operation_id.to_be_bytes());
        if let Some(p) = self.pose {
            if p.size != std::mem::size_of::<ExternalPose>() as u32
                || p.abi_version != VERSION
                || p.session_id != self.session_id
                || p.sequence != self.operation_id
            {
                return Err(Error::Protocol);
            }
            for n in [
                p.tracked_device_id,
                p.tracking_origin_id,
                p.frame_id,
                p.source_clock_id,
                p.sample_timestamp_ns as u64,
                p.clock_uncertainty_ns,
            ] {
                b.extend_from_slice(&n.to_be_bytes());
            }
            for n in [p.timestamp_semantics, p.flags] {
                b.extend_from_slice(&n.to_be_bytes());
            }
            for v in p
                .position_m
                .iter()
                .chain(p.orientation_xyzw.iter())
                .chain(p.linear_velocity_mps.iter())
                .chain(p.angular_velocity_radps.iter())
            {
                b.extend_from_slice(&v.to_bits().to_be_bytes());
            }
        }
        if let Some(device) = &self.device {
            b.extend(serde_json::to_vec(device).map_err(|_| Error::Protocol)?);
        }
        if b.len() + 4 > MAX_FRAME {
            return Err(Error::Protocol);
        }
        let mut frame = (b.len() as u32).to_be_bytes().to_vec();
        frame.extend(b);
        Ok(frame)
    }
    pub fn decode(frame: &[u8]) -> Result<Self> {
        if frame.len() < 32 || frame.len() > MAX_FRAME {
            return Err(Error::Protocol);
        }
        let mut r = Reader {
            bytes: frame,
            pos: 0,
        };
        if r.u32()? as usize != frame.len() - 4
            || r.take(4)? != b"LLFB"
            || r.u32()? != PROTOCOL_VERSION
        {
            return Err(Error::Protocol);
        }
        let kind = match r.u32()? {
            1 => Kind::Hello,
            2 => Kind::Prepare,
            3 => Kind::UsbExportReady,
            4 => Kind::UsbAttached,
            5 => Kind::TrackingReady,
            6 => Kind::Stop,
            7 => Kind::Stopped,
            8 => Kind::Heartbeat,
            9 => Kind::StopRequest,
            10 => Kind::Pose,
            _ => return Err(Error::Protocol),
        };
        let session_id = r.u64()?;
        let operation_id = r.u64()?;
        if session_id == 0 || operation_id == 0 {
            return Err(Error::Protocol);
        }
        let pose = if kind == Kind::Pose {
            let mut p = ExternalPose::mock(session_id, operation_id, 0);
            p.tracked_device_id = r.u64()?;
            p.tracking_origin_id = r.u64()?;
            p.frame_id = r.u64()?;
            p.source_clock_id = r.u64()?;
            p.sample_timestamp_ns = r.u64()? as i64;
            p.clock_uncertainty_ns = r.u64()?;
            p.timestamp_semantics = r.u32()?;
            p.flags = r.u32()?;
            for v in p
                .position_m
                .iter_mut()
                .chain(p.orientation_xyzw.iter_mut())
                .chain(p.linear_velocity_mps.iter_mut())
                .chain(p.angular_velocity_radps.iter_mut())
            {
                *v = f32::from_bits(r.u32()?);
            }
            Some(p)
        } else {
            None
        };
        let device = if kind == Kind::UsbExportReady {
            let bytes = r.take(frame.len() - r.pos)?;
            Some(serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?)
        } else {
            None
        };
        if r.pos != frame.len() {
            return Err(Error::Protocol);
        }
        Ok(Self {
            kind,
            session_id,
            operation_id,
            pose,
            device,
        })
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Protocol)?;
        let v = self.bytes.get(self.pos..end).ok_or(Error::Protocol)?;
        self.pos = end;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Protocol)?,
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Protocol)?,
        ))
    }
}
pub struct Transport {
    network: Option<network::Workers>,
    control_tx: SyncSender<Vec<u8>>,
    control_rx: Receiver<Vec<u8>>,
    pub pose_tx: SyncSender<Vec<u8>>,
    pub pose_rx: Option<Receiver<Vec<u8>>>,
}
pub fn mock_pair() -> (Transport, Transport) {
    let (a, b) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (c, d) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (e, f) = mpsc::sync_channel(QUEUE_CAPACITY);
    let (g, h) = mpsc::sync_channel(QUEUE_CAPACITY);
    (
        Transport {
            network: None,
            control_tx: a,
            control_rx: d,
            pose_tx: e,
            pose_rx: Some(h),
        },
        Transport {
            network: None,
            control_tx: c,
            control_rx: b,
            pose_tx: g,
            pose_rx: Some(f),
        },
    )
}
pub fn send(sender: &SyncSender<Vec<u8>>, message: &Message) -> Result<()> {
    sender.try_send(message.encode()?).map_err(|e| match e {
        TrySendError::Full(_) => Error::Backpressure,
        TrySendError::Disconnected(_) => Error::Disconnected,
    })
}
impl Transport {
    pub fn send(&self, kind: Kind, session: u64, op: u64) -> Result<()> {
        send(&self.control_tx, &Message::control(kind, session, op))
    }
    pub fn export_ready(
        &self,
        session: u64,
        device: light_bridge_usbip::UsbDeviceIdentity,
    ) -> Result<()> {
        send(
            &self.control_tx,
            &Message {
                kind: Kind::UsbExportReady,
                session_id: session,
                operation_id: 3,
                pose: None,
                device: Some(device),
            },
        )
    }
    pub fn receive(&self) -> Result<Option<Message>> {
        match self.control_rx.try_recv() {
            Ok(b) => Ok(Some(Message::decode(&b)?)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Error::Disconnected),
        }
    }
    pub fn receive_timeout(&self, timeout: Duration) -> Result<Message> {
        let b = self.control_rx.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => Error::Timeout,
            _ => Error::Disconnected,
        })?;
        Message::decode(&b)
    }
    pub fn publish(&self, p: ExternalPose) -> Result<()> {
        send(
            &self.pose_tx,
            &Message {
                kind: Kind::Pose,
                session_id: p.session_id,
                operation_id: p.sequence,
                pose: Some(p),
                device: None,
            },
        )
    }
}
/// Complete a small setup/notification buffer under one absolute deadline.
/// Stream must be nonblocking; this runs only on an adapter control worker.
pub(crate) fn transfer<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    bytes: &mut [u8],
    write: bool,
    end: std::time::Instant,
) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if std::time::Instant::now() >= end {
            return Err(Error::Timeout);
        }
        let result = if write {
            stream.write(&bytes[offset..])
        } else {
            stream.read(&mut bytes[offset..])
        };
        match result {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(_) => return Err(Error::Disconnected),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_and_malformed_frames() {
        let m = Message {
            kind: Kind::Pose,
            session_id: 1,
            operation_id: 1,
            pose: Some(ExternalPose::mock(1, 1, 42)),
            device: None,
        };
        let b = m.encode().unwrap();
        assert_eq!(Message::decode(&b).unwrap(), m);
        for n in 0..b.len() {
            assert!(Message::decode(&b[..n]).is_err());
        }
        let mut v = b.clone();
        v[11] = 3;
        assert!(Message::decode(&v).is_err());
        let mut v = b.clone();
        v.push(0);
        assert!(Message::decode(&v).is_err());
        let mut v = b;
        v[15] = 255;
        assert!(Message::decode(&v).is_err());
    }
    #[test]
    fn bounded_transport_timeout_disconnect() {
        let (a, b) = mock_pair();
        assert_eq!(a.receive_timeout(Duration::ZERO), Err(Error::Timeout));
        for _ in 0..QUEUE_CAPACITY {
            a.send(Kind::Hello, 1, 1).unwrap();
        }
        assert_eq!(a.send(Kind::Hello, 1, 1), Err(Error::Backpressure));
        drop(b);
        assert_eq!(a.send(Kind::Hello, 1, 1), Err(Error::Disconnected));
    }
    #[test]
    fn device_offer_is_required_and_strictly_decoded() {
        let mut m = Message::control(Kind::UsbExportReady, 1, 3);
        assert_eq!(m.encode(), Err(Error::Protocol));
        m.device = Some(light_bridge_usbip::UsbDeviceIdentity::mock());
        let b = m.encode().unwrap();
        assert_eq!(Message::decode(&b).unwrap(), m);
        let mut b = b;
        b[11] = 1;
        assert_eq!(Message::decode(&b), Err(Error::Protocol));
    }
}
