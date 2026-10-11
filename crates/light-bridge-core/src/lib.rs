//! Session and tracking data shared by the two in-process libraries.
pub mod clock;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const VERSION: u32 = 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidArgument,
    InvalidState,
    WrongSession,
    Duplicate,
    Timeout,
    Disconnected,
    Backpressure,
    Protocol,
    Unsupported,
    UnsupportedExportGranularity,
    DeviceDenied,
    AmbiguousDevice,
    Inventory,
    UsbExport,
    UsbAttach,
    Enumeration,
    Ownership,
    InvalidPose,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointInfo {
    pub addresses: Vec<SocketAddr>,
    pub host_id: u64,
    pub correlation_id: u64,
}
impl EndpointInfo {
    pub fn validate(&self) -> Result<()> {
        if self.addresses.is_empty()
            || self.host_id == 0
            || self.correlation_id == 0
            || self.addresses.iter().any(|a| a.port() == 0)
        {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
}
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Connecting,
    Negotiated,
    PreparingUsb,
    UsbAttached,
    TrackingInitializing,
    Active,
    Stopping,
    Failed,
    Cleanup,
}
#[derive(Debug)]
pub struct Session {
    pub id: u64,
    pub state: State,
    deadline: Instant,
    timeout: Duration,
}
impl Session {
    pub fn new(timeout: Duration) -> Self {
        Self {
            id: 0,
            state: State::Idle,
            deadline: Instant::now(),
            timeout,
        }
    }
    pub fn start(&mut self, id: u64) -> Result<()> {
        if id == 0 || (self.state == State::Idle && self.id == id) {
            return Err(Error::InvalidArgument);
        }
        if self.state != State::Idle {
            return if self.id == id {
                Ok(())
            } else {
                Err(Error::WrongSession)
            };
        }
        self.id = id;
        self.state = State::Connecting;
        self.touch();
        Ok(())
    }
    pub fn check(&self, id: u64) -> Result<()> {
        if id != self.id || self.state == State::Idle {
            Err(Error::WrongSession)
        } else {
            Ok(())
        }
    }
    pub fn advance(&mut self, id: u64, next: State) -> Result<()> {
        self.check(id)?;
        if self.state == next {
            return Ok(());
        }
        let legal = matches!(
            (self.state, next),
            (State::Connecting, State::Negotiated)
                | (State::Negotiated, State::PreparingUsb)
                | (State::PreparingUsb, State::UsbAttached)
                | (State::UsbAttached, State::TrackingInitializing)
                | (State::TrackingInitializing, State::Active)
                | (State::Stopping, State::Cleanup)
                | (State::Failed, State::Cleanup)
                | (State::Cleanup, State::Idle)
        );
        if !legal {
            return Err(Error::InvalidState);
        }
        self.state = next;
        self.touch();
        Ok(())
    }
    pub fn touch(&mut self) {
        self.deadline = Instant::now() + self.timeout;
    }
    pub fn expired(&self) -> bool {
        self.state != State::Idle && Instant::now() >= self.deadline
    }
    pub fn stop(&mut self, id: u64) -> Result<()> {
        if self.state == State::Idle {
            return if id == self.id {
                Ok(())
            } else {
                Err(Error::WrongSession)
            };
        }
        self.check(id)?;
        self.state = State::Stopping;
        Ok(())
    }
    pub fn fail(&mut self) {
        self.state = State::Failed;
    }
    pub fn cleaned(&mut self) {
        self.state = State::Cleanup;
        self.state = State::Idle;
    }
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExternalPose {
    pub size: u32,
    pub abi_version: u32,
    pub session_id: u64,
    pub sequence: u64,
    pub tracked_device_id: u64,
    pub tracking_origin_id: u64,
    pub frame_id: u64,
    pub source_clock_id: u64,
    pub sample_timestamp_ns: i64,
    pub clock_uncertainty_ns: u64,
    /// 0 measured, 1 estimated, 2 query-time. Never relabel a query as measured.
    pub timestamp_semantics: u32,
    pub flags: u32,
    pub position_m: [f32; 3],
    pub orientation_xyzw: [f32; 4],
    pub linear_velocity_mps: [f32; 3],
    pub angular_velocity_radps: [f32; 3],
}
impl ExternalPose {
    pub fn mock(session_id: u64, sequence: u64, time: i64) -> Self {
        Self {
            size: std::mem::size_of::<Self>() as u32,
            abi_version: VERSION,
            session_id,
            sequence,
            tracked_device_id: 1,
            tracking_origin_id: 1,
            frame_id: 1,
            source_clock_id: 1,
            sample_timestamp_ns: time,
            clock_uncertainty_ns: 0,
            timestamp_semantics: 1,
            flags: 7,
            position_m: [1., 2., 3.],
            orientation_xyzw: [0., 0., 0., 1.],
            linear_velocity_mps: [0.; 3],
            angular_velocity_radps: [0.; 3],
        }
    }
}
/// Explicit mapping only. No implicit equivalence between Host and Monado clocks.
#[derive(Debug, Clone, Copy)]
pub struct ClockMapping {
    pub source_clock_id: u64,
    pub offset_ns: i64,
    pub uncertainty_ns: u64,
}
#[derive(Debug)]
pub struct PosePolicy {
    pub tracked_device_id: u64,
    pub max_age_ns: i64,
    pub max_uncertainty_ns: u64,
    pub future_tolerance_ns: i64,
}
impl Default for PosePolicy {
    fn default() -> Self {
        Self {
            tracked_device_id: 1,
            max_age_ns: 100_000_000,
            max_uncertainty_ns: 5_000_000,
            future_tolerance_ns: 1_000_000,
        }
    }
}
#[derive(Debug, Default)]
pub struct PoseValidator {
    pub policy: PosePolicy,
    last: Option<(u64, i64, u64, u64)>,
}
impl PoseValidator {
    pub fn reset(&mut self) {
        self.last = None;
    }
    pub fn validate(
        &mut self,
        p: &ExternalPose,
        session: u64,
        now: i64,
        map: ClockMapping,
    ) -> Result<i64> {
        if p.session_id != session {
            return Err(Error::WrongSession);
        }
        let t = p
            .sample_timestamp_ns
            .checked_add(map.offset_ns)
            .ok_or(Error::InvalidPose)?;
        let age = now.checked_sub(t).ok_or(Error::InvalidPose)?;
        let norm: f32 = p.orientation_xyzw.iter().map(|v| v * v).sum();
        if p.size != std::mem::size_of::<ExternalPose>() as u32
            || p.abi_version != VERSION
            || p.sequence == 0
            || p.tracked_device_id == 0
            || p.tracked_device_id != self.policy.tracked_device_id
            || p.tracking_origin_id == 0
            || p.frame_id == 0
            || p.timestamp_semantics > 2
            || p.flags & !31 != 0
            || p.flags & 7 != 7
            || p.source_clock_id != map.source_clock_id
            || age > self.policy.max_age_ns
            || age < -self.policy.future_tolerance_ns
            || p.clock_uncertainty_ns.saturating_add(map.uncertainty_ns)
                > self.policy.max_uncertainty_ns
            || !p
                .position_m
                .iter()
                .chain(p.orientation_xyzw.iter())
                .chain(p.linear_velocity_mps.iter())
                .chain(p.angular_velocity_radps.iter())
                .all(|v| v.is_finite())
            || !(0.98..=1.02).contains(&norm)
        {
            return Err(Error::InvalidPose);
        }
        if let Some((seq, last, origin, frame)) = self.last {
            if p.sequence <= seq
                || t < last
                || origin != p.tracking_origin_id
                || frame != p.frame_id
            {
                return Err(Error::InvalidPose);
            }
        }
        self.last = Some((p.sequence, t, p.tracking_origin_id, p.frame_id));
        Ok(t)
    }
}
/// Research extension only. Local arpara IMU remains outside this bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImuCaptureMode {
    None,
    DirectUsb,
    UsbMon,
}
pub trait ImuProvider {
    fn set_mode(&mut self, mode: ImuCaptureMode) -> Result<()>;
}
pub struct NoImu;
impl ImuProvider for NoImu {
    fn set_mode(&mut self, mode: ImuCaptureMode) -> Result<()> {
        if mode == ImuCaptureMode::None {
            Ok(())
        } else {
            Err(Error::Unsupported)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_order_and_duplicates() {
        let mut s = Session::new(Duration::from_secs(1));
        s.start(1).unwrap();
        s.start(1).unwrap();
        assert_eq!(s.advance(1, State::Active), Err(Error::InvalidState));
        assert_eq!(s.start(2), Err(Error::WrongSession));
        s.stop(1).unwrap();
        s.cleaned();
        s.stop(1).unwrap();
        s.start(2).unwrap();
        assert_eq!(s.check(1), Err(Error::WrongSession));
    }
    #[test]
    fn deadline_and_cancel() {
        let mut s = Session::new(Duration::ZERO);
        s.start(1).unwrap();
        assert!(s.expired());
        s.stop(1).unwrap();
        s.cleaned();
        assert!(!s.expired());
    }
    #[test]
    fn pose_rejects() {
        let m = ClockMapping {
            source_clock_id: 1,
            offset_ns: 10,
            uncertainty_ns: 0,
        };
        let p = ExternalPose::mock(1, 1, 100);
        let mut v = PoseValidator::default();
        assert_eq!(v.validate(&p, 1, 110, m), Ok(110));
        assert!(v.validate(&p, 1, 110, m).is_err());
        let mut p = ExternalPose::mock(1, 2, 90);
        assert!(v.validate(&p, 1, 110, m).is_err());
        p.sample_timestamp_ns = 111;
        p.tracking_origin_id = 2;
        assert!(v.validate(&p, 1, 120, m).is_err());
        v.reset();
        p.flags = 0;
        assert!(v.validate(&p, 1, 120, m).is_err());
        p.flags = 7;
        p.clock_uncertainty_ns = u64::MAX;
        assert!(v.validate(&p, 1, 120, m).is_err());
        p.clock_uncertainty_ns = 0;
        assert!(v.validate(&p, 2, 120, m).is_err());
        assert!(v.validate(&p, 1, 200_000_000, m).is_err());
    }
    #[test]
    fn bsb_stub() {
        assert_eq!(
            NoImu.set_mode(ImuCaptureMode::UsbMon),
            Err(Error::Unsupported)
        );
    }
}
