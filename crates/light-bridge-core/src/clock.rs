//! Session-scoped monotonic clock mapping. Four-timestamp exchanges do not discipline OS clocks.
use crate::{Error, Result, VERSION};
use std::collections::VecDeque;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TimingConfig {
    pub size: u32,
    pub abi_version: u32,
    pub tracked_device_id: u64,
    pub source_clock_id: u64,
    pub local_clock_id: u64,
    pub max_age_ns: i64,
    pub future_tolerance_ns: i64,
    pub max_uncertainty_ns: u64,
    pub sync_interval_ns: i64,
    pub request_timeout_ns: i64,
    pub mapping_validity_ns: i64,
    /// Assumed bound on relative frequency error, parts per billion. Not a measured hardware guarantee.
    pub max_drift_ppb: u64,
}
impl TimingConfig {
    pub fn validate(&self) -> Result<()> {
        if self.size != std::mem::size_of::<Self>() as u32
            || self.abi_version != VERSION
            || self.tracked_device_id == 0
            || self.source_clock_id == 0
            || self.local_clock_id == 0
            || self.max_age_ns <= 0
            || self.future_tolerance_ns < 0
            || self.max_uncertainty_ns == 0
            || self.sync_interval_ns <= 0
            || self.request_timeout_ns <= 0
            || self.mapping_validity_ns < self.sync_interval_ns
            || self.max_drift_ppb == 0
            || self.max_drift_ppb > 1_000_000
            || self.mapping_validity_ns > 60_000_000_000
            || self.request_timeout_ns > self.mapping_validity_ns
        {
            return Err(Error::InvalidArgument);
        }
        Ok(())
    }
    /// Experimental defaults; acceptance limits must be calibrated with real hardware.
    pub fn experimental(device: u64, source: u64, local: u64) -> Self {
        Self {
            size: std::mem::size_of::<Self>() as u32,
            abi_version: VERSION,
            tracked_device_id: device,
            source_clock_id: source,
            local_clock_id: local,
            max_age_ns: 100_000_000,
            future_tolerance_ns: 1_000_000,
            max_uncertainty_ns: 5_000_000,
            sync_interval_ns: 100_000_000,
            request_timeout_ns: 500_000_000,
            mapping_validity_ns: 2_000_000_000,
            max_drift_ppb: 200_000,
        }
    }
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ClockStatus {
    pub size: u32,
    pub abi_version: u32,
    pub synchronized: u32,
    pub sample_count: u32,
    pub source_clock_id: u64,
    pub local_clock_id: u64,
    pub generation: u64,
    pub source_anchor_ns: i64,
    pub local_anchor_ns: i64,
    pub drift_ppb: i64,
    pub rtt_ns: u64,
    pub uncertainty_ns: u64,
    pub valid_until_ns: i64,
}
impl Default for ClockStatus {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>() as u32,
            abi_version: VERSION,
            synchronized: 0,
            sample_count: 0,
            source_clock_id: 0,
            local_clock_id: 0,
            generation: 0,
            source_anchor_ns: 0,
            local_anchor_ns: 0,
            drift_ppb: 0,
            rtt_ns: 0,
            uncertainty_ns: u64::MAX,
            valid_until_ns: 0,
        }
    }
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReferenceTiming {
    pub size: u32,
    pub abi_version: u32,
    pub session_id: u64,
    pub sequence: u64,
    pub local_clock_id: u64,
    pub mapping_generation: u64,
    pub local_sample_timestamp_ns: i64,
    pub local_receive_timestamp_ns: i64,
    pub total_uncertainty_ns: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exchange {
    pub source_clock_id: u64,
    pub local_clock_id: u64,
    pub t1: i64,
    pub t2: i64,
    pub t3: i64,
}
#[derive(Clone, Copy)]
struct Point {
    host: i64,
    local: i64,
    error: u64,
    rtt: u64,
}
pub struct Synchronizer {
    pub config: TimingConfig,
    points: VecDeque<Point>,
    status: ClockStatus,
    last_now: Option<i64>,
}
impl Synchronizer {
    pub fn new(config: TimingConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            points: VecDeque::new(),
            status: ClockStatus::default(),
            last_now: None,
        })
    }
    pub fn reset(&mut self) {
        let generation = self.status.generation.saturating_add(1);
        self.points.clear();
        self.status = ClockStatus {
            generation,
            ..ClockStatus::default()
        };
        self.last_now = None;
    }
    pub fn check_now(&mut self, now: i64) -> Result<()> {
        if now < 0 || self.last_now.is_some_and(|t| now < t) {
            self.reset();
            return Err(Error::InvalidPose);
        }
        self.last_now = Some(now);
        Ok(())
    }
    pub fn observe(&mut self, x: Exchange, t4: i64) -> Result<()> {
        self.check_now(t4)?;
        if x.source_clock_id != self.config.source_clock_id
            || x.local_clock_id != self.config.local_clock_id
            || x.t1 < 0
            || x.t2 < 0
            || x.t3 < x.t2
            || t4 <= x.t1
        {
            self.reset();
            return Err(Error::InvalidPose);
        }
        let round = i128::from(t4) - i128::from(x.t1);
        let service = i128::from(x.t3) - i128::from(x.t2);
        if round > i128::from(self.config.request_timeout_ns) || service > round {
            return Err(Error::InvalidPose);
        }
        let rtt = (round - service) as u64;
        let error = (rtt / 2)
            .saturating_add(1)
            .saturating_add((round * i128::from(self.config.max_drift_ppb) / 1_000_000_000) as u64);
        if error > self.config.max_uncertainty_ns {
            return Err(Error::InvalidPose);
        }
        let p = Point {
            host: ((i128::from(x.t2) + i128::from(x.t3)) / 2) as i64,
            local: ((i128::from(x.t1) + i128::from(t4)) / 2) as i64,
            error,
            rtt,
        };
        if let Some(last) = self.points.back() {
            if p.host <= last.host || p.local <= last.local {
                self.reset();
                return Err(Error::InvalidPose);
            }
            let dh = i128::from(p.host) - i128::from(last.host);
            let dl = i128::from(p.local) - i128::from(last.local);
            let allowance = i128::from(p.error)
                + i128::from(last.error)
                + dh * i128::from(self.config.max_drift_ppb) / 1_000_000_000;
            if (dl - dh).abs() > allowance {
                self.reset();
                return Err(Error::InvalidPose);
            }
        }
        self.points.push_back(p);
        if self.points.len() > 8 {
            self.points.pop_front();
        }
        let first = *self.points.front().unwrap();
        let span = i128::from(p.host) - i128::from(first.host);
        // Short spans cannot reliably distinguish frequency from network jitter.
        let drift = if span >= 100_000_000 {
            let d = (i128::from(p.local) - i128::from(first.local) - span) * 1_000_000_000 / span;
            d.clamp(
                -i128::from(self.config.max_drift_ppb),
                i128::from(self.config.max_drift_ppb),
            ) as i64
        } else {
            0
        };
        let uncertainty = self
            .points
            .iter()
            .map(|v| {
                let dh = i128::from(v.host) - i128::from(p.host);
                let predicted = i128::from(p.local) + dh + dh * i128::from(drift) / 1_000_000_000;
                let residual = (predicted - i128::from(v.local)).unsigned_abs();
                v.error
                    .saturating_add(u64::try_from(residual).unwrap_or(u64::MAX))
            })
            .max()
            .unwrap();
        self.status = ClockStatus {
            size: std::mem::size_of::<ClockStatus>() as u32,
            abi_version: VERSION,
            synchronized: u32::from(
                self.points.len() >= 3 && uncertainty <= self.config.max_uncertainty_ns,
            ),
            sample_count: self.points.len() as u32,
            source_clock_id: x.source_clock_id,
            local_clock_id: x.local_clock_id,
            generation: self.status.generation.saturating_add(1),
            source_anchor_ns: p.host,
            local_anchor_ns: p.local,
            drift_ppb: drift,
            rtt_ns: p.rtt,
            uncertainty_ns: uncertainty,
            valid_until_ns: t4
                .checked_add(self.config.mapping_validity_ns)
                .ok_or(Error::InvalidPose)?,
        };
        Ok(())
    }
    pub fn status(&self, now: i64) -> ClockStatus {
        let mut s = self.status;
        if now < s.local_anchor_ns || now > s.valid_until_ns {
            s.synchronized = 0;
        }
        let horizon = (i128::from(now) - i128::from(s.local_anchor_ns)).unsigned_abs();
        let growth = horizon * u128::from(self.config.max_drift_ppb) * 2 / 1_000_000_000;
        s.uncertainty_ns = s
            .uncertainty_ns
            .saturating_add(u64::try_from(growth).unwrap_or(u64::MAX));
        if s.uncertainty_ns > self.config.max_uncertainty_ns {
            s.synchronized = 0;
        }
        s
    }
    pub fn convert(&self, host: i64, now: i64) -> Result<(i64, u64, u64)> {
        let s = self.status(now);
        if s.synchronized == 0 || host < 0 {
            return Err(Error::InvalidPose);
        }
        let delta = i128::from(host) - i128::from(s.source_anchor_ns);
        let local =
            i128::from(s.local_anchor_ns) + delta + delta * i128::from(s.drift_ppb) / 1_000_000_000;
        // 2*bound covers error in the fitted rate relative to the assumed real rate.
        let horizon = delta
            .unsigned_abs()
            .max((i128::from(now) - i128::from(s.local_anchor_ns)).unsigned_abs());
        let growth = horizon * u128::from(self.config.max_drift_ppb) * 2 / 1_000_000_000;
        let uncertainty = self
            .status
            .uncertainty_ns
            .saturating_add(u64::try_from(growth).unwrap_or(u64::MAX));
        if uncertainty > self.config.max_uncertainty_ns {
            return Err(Error::InvalidPose);
        }
        Ok((
            i64::try_from(local).map_err(|_| Error::InvalidPose)?,
            uncertainty,
            s.generation,
        ))
    }
}
/// Linux CLOCK_MONOTONIC nanoseconds. Runtime adapters must verify the source clock uses this domain.
pub fn monotonic_ns() -> Result<i64> {
    #[cfg(target_os = "linux")]
    {
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: initialized timespec, valid writable pointer.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) } != 0 {
            return Err(Error::InvalidState);
        }
        i64::try_from(i128::from(t.tv_sec) * 1_000_000_000 + i128::from(t.tv_nsec))
            .map_err(|_| Error::InvalidState)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(Error::Unsupported)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample(host: i64, offset: i64, drift: i64, delay: i64) -> (Exchange, i64) {
        let local = host + offset + host * drift / 1_000_000_000;
        (
            Exchange {
                source_clock_id: 2,
                local_clock_id: 3,
                t1: local - delay,
                t2: host,
                t3: host,
            },
            local + delay,
        )
    }
    #[test]
    fn offset_drift_jitter_expiry_and_reset() {
        let mut s = Synchronizer::new(TimingConfig::experimental(17, 2, 3)).unwrap();
        for n in 1..=8 {
            let (x, t) = sample(n * 1_000_000_000, 20_000_000, 50_000, 100_000);
            s.observe(x, t).unwrap();
        }
        let (mapped, u, _) = s.convert(8_001_000_000, 8_022_000_000).unwrap();
        assert!((mapped - 8_021_400_050).abs() < 2);
        assert!(u >= 100_000);
        assert_eq!(s.status(8_022_000_000).drift_ppb, 50_000);
        assert!(s.convert(8_001_000_000, 20_000_000_000).is_err());
        let (x, t) = sample(9_000_000_000, 400_000_000, 50_000, 100_000);
        assert!(s.observe(x, t).is_err());
        assert_eq!(s.status(t).synchronized, 0);
        assert!(s.check_now(1).is_ok()); // reset establishes a fresh epoch, not an old mapping.
        assert!(s.convert(1, 1).is_err());
    }
    #[test]
    fn asymmetry_is_in_uncertainty_and_bad_domains_rejected() {
        let mut s = Synchronizer::new(TimingConfig::experimental(17, 2, 3)).unwrap();
        for n in 1..=3 {
            let (mut x, t) = sample(n * 1_000_000_000, 20_000_000, 0, 200_000);
            x.t1 -= 100_000;
            s.observe(x, t).unwrap();
        }
        let (mapped, u, _) = s.convert(3_000_000_000, 3_020_200_000).unwrap();
        assert!((mapped - 3_020_000_000).unsigned_abs() <= u);
        let (mut x, t) = sample(4_000_000_000, 20_000_000, 0, 100_000);
        x.source_clock_id = 99;
        assert!(s.observe(x, t).is_err());
        assert_eq!(s.status(t).synchronized, 0);
    }
    #[test]
    fn varying_delay_and_asymmetry_keep_truth_inside_error_budget() {
        let mut s = Synchronizer::new(TimingConfig::experimental(17, 2, 3)).unwrap();
        for n in 1..=16 {
            let host = n * 1_000_000_000;
            let (mut x, mut t) = sample(host, 20_000_000, -70_000, 100_000);
            x.t1 -= (n % 3) * 30_000;
            t += (n % 4) * 40_000;
            s.observe(x, t).unwrap();
            if n >= 3 {
                let (mapped, u, _) = s.convert(host, t).unwrap();
                let truth = host + 20_000_000 + host * (-70_000) / 1_000_000_000;
                assert!((mapped - truth).unsigned_abs() <= u);
            }
        }
        // Replayed/backwards source points cannot establish a new mapping.
        let (x, t) = sample(16_000_000_000, 20_000_000, -70_000, 300_000);
        assert!(s.observe(x, t).is_err());
        assert_eq!(s.status(t).synchronized, 0);
    }
    #[test]
    fn high_rtt_overflow_and_backwards_clock_fail_closed() {
        let mut s = Synchronizer::new(TimingConfig::experimental(17, 2, 3)).unwrap();
        let (x, t) = sample(1_000_000_000, 0, 0, 10_000_000);
        assert!(s.observe(x, t).is_err());
        assert!(s.check_now(1).is_err());
        let mut c = TimingConfig::experimental(17, 2, 3);
        c.max_drift_ppb = u64::MAX;
        assert!(Synchronizer::new(c).is_err());
    }
}
