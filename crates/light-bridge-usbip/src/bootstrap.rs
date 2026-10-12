//! Automatic preparation; authorization is distinct from an immutable session policy.
use crate::{
    inventory::InventoryReport,
    kernel::{KResult, Policy, Role},
    UsbDeviceIdentity,
};
use light_bridge_core::clock::TimingConfig;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
};

pub const VERSION: u32 = 1;
pub const MAX_PACKET: usize = 8192;
pub const MAX_CANDIDATES: usize = 16;
pub const PROFILE: (u16, u16) = (0x28de, 0x2300);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub local: SocketAddr,
    pub peer: SocketAddr,
    pub netns_cookie: u64,
}
impl Route {
    pub fn validate(&self, allow_loopback: bool) -> KResult<()> {
        for endpoint in [self.local, self.peer] {
            if endpoint.port() == 0
                || endpoint.ip().is_unspecified()
                || endpoint.ip().is_multicast()
                || (!allow_loopback && endpoint.ip().is_loopback())
            {
                return Err("unsupported/unauthorized connected route".into());
            }
            let lan = match endpoint.ip() {
                IpAddr::V4(v) => {
                    v.is_private() || v.is_link_local() || (allow_loopback && v.is_loopback())
                }
                IpAddr::V6(v) => {
                    v.octets()[0] & 0xfe == 0xfc
                        || v.segments()[0] & 0xffc0 == 0xfe80
                        || (allow_loopback && v.is_loopback())
                }
            };
            if !lan {
                return Err(
                    "automatic setup initially permits private/link-local LAN routes only".into(),
                );
            }
            if let IpAddr::V6(ip) = endpoint.ip() {
                if ip.segments()[0] & 0xffc0 == 0xfe80 && endpoint.scope_id() == 0 {
                    return Err("link-local route needs a local interface scope".into());
                }
            }
        }
        if self.netns_cookie == 0 || self.local.is_ipv4() != self.peer.is_ipv4() {
            return Err("route family/network namespace invalid".into());
        }
        Ok(())
    }
    pub fn reversed_ips(&self, other: &Self) -> bool {
        self.local.ip() == other.peer.ip()
            && self.peer.ip() == other.local.ip()
            && self.local.port() == other.peer.port()
            && self.peer.port() == other.local.port()
    }
    pub fn usb_endpoint(&self, role: Role, port: u16) -> SocketAddr {
        let mut addr = if role == Role::Export {
            self.local
        } else {
            self.peer
        };
        addr.set_port(port);
        addr
    }
}
// SocketAddr::scope_id is only available on V6; keep scope resolution local.
trait Scope {
    fn scope_id(&self) -> u32;
}
impl Scope for SocketAddr {
    fn scope_id(&self) -> u32 {
        match self {
            Self::V6(v) => v.scope_id(),
            _ => 0,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub instance: u64,
    pub session: u64,
    pub host_id: u64,
    pub generation: u64,
}
impl Stamp {
    pub fn validate(&self) -> KResult<()> {
        if [self.instance, self.session, self.host_id, self.generation].contains(&0) {
            Err("zero bootstrap identity".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub device: u64,
    pub source: u64,
    pub local: u64,
    pub max_age_ns: i64,
    pub future_tolerance_ns: i64,
    pub max_uncertainty_ns: u64,
    pub sync_interval_ns: i64,
    pub request_timeout_ns: i64,
    pub mapping_validity_ns: i64,
    pub max_drift_ppb: u64,
}
impl Timing {
    pub fn experimental(device: u64, source: u64, local: u64) -> Self {
        Self::from_core(TimingConfig::experimental(device, source, local))
    }
    pub fn from_core(c: TimingConfig) -> Self {
        Self {
            device: c.tracked_device_id,
            source: c.source_clock_id,
            local: c.local_clock_id,
            max_age_ns: c.max_age_ns,
            future_tolerance_ns: c.future_tolerance_ns,
            max_uncertainty_ns: c.max_uncertainty_ns,
            sync_interval_ns: c.sync_interval_ns,
            request_timeout_ns: c.request_timeout_ns,
            mapping_validity_ns: c.mapping_validity_ns,
            max_drift_ppb: c.max_drift_ppb,
        }
    }
    pub fn core(&self) -> TimingConfig {
        TimingConfig {
            size: std::mem::size_of::<TimingConfig>() as u32,
            abi_version: 1,
            tracked_device_id: self.device,
            source_clock_id: self.source,
            local_clock_id: self.local,
            max_age_ns: self.max_age_ns,
            future_tolerance_ns: self.future_tolerance_ns,
            max_uncertainty_ns: self.max_uncertainty_ns,
            sync_interval_ns: self.sync_interval_ns,
            request_timeout_ns: self.request_timeout_ns,
            mapping_validity_ns: self.mapping_validity_ns,
            max_drift_ppb: self.max_drift_ppb,
        }
    }
    pub fn validate(&self) -> KResult<()> {
        self.core().validate().map_err(|e| format!("timing: {e:?}"))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub stamp: Stamp,
    pub host_route: Route,
    pub timing: Timing,
}
impl Request {
    pub fn validate(&self) -> KResult<()> {
        self.stamp.validate()?;
        self.timing.validate()?;
        self.host_route.validate(true)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    pub role: Role,
    pub client_uid: u32,
    pub client_gid: u32,
    pub allow_loopback: bool,
    pub allow_package_install: bool,
}
impl Authorization {
    pub fn validate(&self) -> KResult<()> {
        if self.client_uid == 0 || self.client_gid == 0 {
            Err("runtime must be a non-root user/group".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub device: UsbDeviceIdentity,
    pub product: String,
    pub interfaces: u8,
}
pub fn candidates(report: InventoryReport) -> KResult<Vec<Candidate>> {
    if !report.issues.is_empty() {
        return Err(format!(
            "incomplete inventory: {}",
            report.issues.join("; ")
        ));
    }
    let mut out = Vec::new();
    if report
        .devices
        .iter()
        .any(|d| d.device_driver.as_deref() == Some("usbip-host"))
    {
        return Err(
            "existing USB/IP exports; dedicated daemon must not expose foreign resources".into(),
        );
    }
    let mut serials = BTreeSet::new();
    for d in report.devices {
        if (d.identity.vid, d.identity.pid) != PROFILE
            || d.via_vhci
            || d.device_driver.as_deref() != Some("usb")
            || d.interface_count == 0
        {
            continue;
        }
        let p = Policy {
            role: Role::Export,
            client_uid: 1,
            client_gid: 1,
            device: d.identity.clone(),
            remote: "127.0.0.1:3240".parse().unwrap(),
        };
        p.validate()?;
        if !serials.insert(d.identity.serial.clone()) {
            return Err("duplicate Tracker serial; selection unsafe".into());
        }
        out.push(Candidate {
            device: d.identity,
            product: d.product,
            interfaces: d.interface_count,
        });
    }
    if out.len() > MAX_CANDIDATES {
        return Err("too many Tracker candidates".into());
    }
    if serde_json::to_vec(&out).map_err(|e| e.to_string())?.len() > MAX_PACKET - 1024 {
        return Err("candidate metadata exceeds bounded selection packet".into());
    }
    Ok(out)
}
pub fn select(candidates: &[Candidate], serial: Option<&str>) -> KResult<Candidate> {
    if let Some(serial) = serial {
        return candidates
            .iter()
            .find(|c| c.device.serial == serial)
            .cloned()
            .ok_or("selected serial absent or denied".into());
    }
    match candidates {
        [one] => Ok(one.clone()),
        [] => Err("no supported Tracker connected".into()),
        _ => Err("selection required: multiple Tracker candidates".into()),
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub request: Request,
    pub route: Route,
    pub policy: Policy,
    pub lease: String,
    pub peer_lease: Option<String>,
}
pub fn valid_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
impl Plan {
    pub fn validate(&self) -> KResult<()> {
        self.request.validate()?;
        self.route.validate(true)?;
        self.policy.validate()?;
        if !valid_token(&self.lease) || (self.policy.device.vid, self.policy.device.pid) != PROFILE
        {
            return Err("invalid preparation lease/profile".into());
        }
        match self.policy.role {
            Role::Export if !self.route.reversed_ips(&self.request.host_route) => {
                return Err("export route differs from WiVRn connection".into())
            }
            Role::Import if self.route != self.request.host_route => {
                return Err("import route differs from WiVRn connection".into())
            }
            _ => (),
        }
        if match self.policy.role {
            Role::Export => self.peer_lease.is_some(),
            Role::Import => !self.peer_lease.as_deref().is_some_and(valid_token),
        } {
            return Err("preparation peer lease invalid".into());
        }
        if self.policy.remote
            != self
                .route
                .usb_endpoint(self.policy.role, self.policy.remote.port())
        {
            return Err("USB/IP destination not observed Pi peer".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    pub stamp: Stamp,
    pub export_lease: String,
    pub import_lease: String,
}
impl Commit {
    pub fn validates(&self, plan: &Plan) -> bool {
        self.stamp == plan.request.stamp
            && valid_token(&self.export_lease)
            && valid_token(&self.import_lease)
            && self.export_lease != self.import_lease
            && &plan.lease
                == if plan.policy.role == Role::Export {
                    &self.export_lease
                } else {
                    &self.import_lease
                }
            && plan
                .peer_lease
                .as_ref()
                .map_or(true, |peer| peer == &self.export_lease)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", deny_unknown_fields)]
pub enum Packet {
    Prepare(Request),
    Candidates {
        stamp: Stamp,
        candidates: Vec<Candidate>,
    },
    Select {
        stamp: Stamp,
        serial: String,
    },
    Offer(Plan),
    Prepared(Plan),
    Commit(Commit),
    Abort {
        stamp: Stamp,
        reason: String,
    },
    Progress {
        stamp: Stamp,
        stage: String,
    },
}
impl Packet {
    pub fn validate(&self) -> KResult<()> {
        match self {
            Self::Prepare(r) => r.validate(),
            Self::Candidates { stamp, candidates } => {
                stamp.validate()?;
                if candidates.len() > MAX_CANDIDATES {
                    return Err("too many candidates".into());
                }
                for candidate in candidates {
                    if (candidate.device.vid, candidate.device.pid) != PROFILE
                        || candidate.interfaces == 0
                        || candidate.product.len() > 1024
                    {
                        return Err("candidate profile invalid".into());
                    }
                    Policy {
                        role: Role::Export,
                        client_uid: 1,
                        client_gid: 1,
                        device: candidate.device.clone(),
                        remote: "127.0.0.1:3240".parse().unwrap(),
                    }
                    .validate()?;
                }
                Ok(())
            }
            Self::Select { stamp, serial } => {
                stamp.validate()?;
                if serial.is_empty() || serial.len() > 128 || serial.chars().any(char::is_control) {
                    Err("selected serial invalid".into())
                } else {
                    Ok(())
                }
            }
            Self::Offer(p) if p.policy.role == Role::Export => p.validate(),
            Self::Prepared(p) if p.policy.role == Role::Import => p.validate(),
            Self::Commit(c) => {
                c.stamp.validate()?;
                if valid_token(&c.export_lease)
                    && valid_token(&c.import_lease)
                    && c.export_lease != c.import_lease
                {
                    Ok(())
                } else {
                    Err("commit leases invalid".into())
                }
            }
            Self::Abort { stamp, reason } => {
                stamp.validate()?;
                if reason.len() <= 512 {
                    Ok(())
                } else {
                    Err("abort reason too long".into())
                }
            }
            Self::Progress { stamp, stage } => {
                stamp.validate()?;
                if stage.len() <= 128 {
                    Ok(())
                } else {
                    Err("progress too long".into())
                }
            }
            _ => Err("packet role invalid".into()),
        }
    }
}
/// Host-side bounded coordinator; a Commit cannot be generated before both prepared plans.
pub struct Coordinator {
    request: Request,
    export: Option<Plan>,
    import: Option<Plan>,
    aborted: bool,
}
impl Coordinator {
    pub fn new(request: Request) -> KResult<Self> {
        request.validate()?;
        Ok(Self {
            request,
            export: None,
            import: None,
            aborted: false,
        })
    }
    pub fn offer(&mut self, p: Plan) -> KResult<()> {
        p.validate()?;
        if self.aborted || p.request != self.request || p.policy.role != Role::Export {
            return Err("wrong/stale export offer".into());
        }
        if self.export.as_ref().is_some_and(|old| old != &p) {
            return Err("export offer changed within session".into());
        }
        self.export = Some(p);
        Ok(())
    }
    pub fn prepared(&mut self, p: Plan) -> KResult<()> {
        p.validate()?;
        let export = self.export.as_ref().ok_or("export not prepared")?;
        if self.aborted
            || p.request != self.request
            || p.policy.role != Role::Import
            || p.policy.device != export.policy.device
            || p.policy.remote.ip() != export.policy.remote.ip()
            || p.policy.remote.port() != export.policy.remote.port()
            || p.peer_lease.as_ref() != Some(&export.lease)
        {
            return Err("import plan not matched to export".into());
        }
        if self.import.as_ref().is_some_and(|old| old != &p) {
            return Err("import plan changed within session".into());
        }
        self.import = Some(p);
        Ok(())
    }
    pub fn commit(&self) -> KResult<Commit> {
        if self.aborted {
            return Err("bootstrap aborted".into());
        }
        let c = Commit {
            stamp: self.request.stamp.clone(),
            export_lease: self
                .export
                .as_ref()
                .ok_or("export not prepared")?
                .lease
                .clone(),
            import_lease: self
                .import
                .as_ref()
                .ok_or("import not prepared")?
                .lease
                .clone(),
        };
        if !c.validates(self.export.as_ref().unwrap())
            || !c.validates(self.import.as_ref().unwrap())
        {
            return Err("commit plan invalid".into());
        }
        Ok(c)
    }
    pub fn abort(&mut self) {
        self.aborted = true;
    }
    pub fn abort_for(&mut self, stamp: &Stamp) -> KResult<()> {
        if stamp != &self.request.stamp {
            return Err("stale bootstrap abort".into());
        }
        self.abort();
        Ok(())
    }
}

pub trait Ops {
    fn inventory(&mut self) -> KResult<InventoryReport>;
    fn token(&mut self) -> KResult<String>;
    fn boot_id(&self) -> KResult<String>;
    fn prerequisites(&mut self, auth: &Authorization) -> KResult<()>;
    fn module(&mut self, role: Role) -> KResult<()>;
    fn export_available(&mut self, _plan: &Plan) -> KResult<()> {
        Ok(())
    }
    fn firewall(&mut self, plan: &Plan) -> KResult<Firewall>;
    fn daemon(&mut self, plan: &Plan) -> KResult<Daemon>;
    fn probe(&mut self, endpoint: SocketAddr) -> KResult<()>;
    fn stop_daemon(&mut self, plan: &Plan, daemon: &Daemon) -> KResult<()>;
    fn remove_firewall(&mut self, plan: &Plan, firewall: &Firewall) -> KResult<()>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Daemon {
    pub pid: u32,
    pub start_ticks: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Firewall {
    pub handle: u64,
    pub snapshot: serde_json::Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Planned,
    Prerequisites,
    Module,
    FirewallPending,
    Firewall,
    DaemonPending,
    Prepared,
    Committed,
    Releasing,
    DaemonStopped,
    FirewallRemoved,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub plan: Plan,
    pub boot_id: String,
    pub stage: Stage,
    pub firewall: Option<Firewall>,
    pub daemon: Option<Daemon>,
    pub commit: Option<Commit>,
}
pub trait Journal {
    fn load(&self) -> KResult<Option<Lease>>;
    fn store(&mut self, lease: Option<&Lease>) -> KResult<()>;
}
pub struct Engine<O: Ops, J: Journal> {
    pub authorization: Authorization,
    pub ops: O,
    journal: J,
    pub lease: Option<Lease>,
    recovered: bool,
    last_stamp: Option<Stamp>,
}
impl<O: Ops, J: Journal> Engine<O, J> {
    pub fn new(authorization: Authorization, ops: O, journal: J) -> KResult<Self> {
        authorization.validate()?;
        let lease = journal.load()?;
        let recovered = lease.is_some();
        Ok(Self {
            authorization,
            ops,
            journal,
            lease,
            recovered,
            last_stamp: None,
        })
    }
    fn save(&mut self, stage: Stage) -> KResult<()> {
        self.lease.as_mut().unwrap().stage = stage;
        self.journal.store(self.lease.as_ref())
    }
    pub fn prepare(
        &mut self,
        request: Request,
        route: Route,
        offer: Option<Plan>,
        serial: Option<&str>,
    ) -> KResult<Plan> {
        request.validate()?;
        route.validate(self.authorization.allow_loopback)?;
        if self.last_stamp.as_ref() == Some(&request.stamp) {
            return Err("completed bootstrap stamp cannot be reused".into());
        }
        if !self.recovered {
            if let Some(lease) = &self.lease {
                let same_selection = serial.map_or(true, |s| s == lease.plan.policy.device.serial);
                let same_offer = match (&offer, self.authorization.role) {
                    (None, Role::Export) => true,
                    (Some(p), Role::Import) => {
                        p.validate().is_ok()
                            && p.request == request
                            && p.policy.role == Role::Export
                            && p.policy.device == lease.plan.policy.device
                            && p.policy.remote.port() == lease.plan.policy.remote.port()
                            && p.policy.remote.ip() == lease.plan.policy.remote.ip()
                            && lease.plan.peer_lease.as_ref() == Some(&p.lease)
                    }
                    _ => false,
                };
                if lease.stage == Stage::Prepared
                    && lease.plan.request == request
                    && lease.plan.route == route
                    && same_selection
                    && same_offer
                {
                    return Ok(lease.plan.clone());
                }
            }
        }
        if self.lease.is_some() || self.recovered {
            return Err("existing environment lease; no automatic adoption/retry".into());
        }
        let role = self.authorization.role;
        let peer_lease = offer.as_ref().map(|p| p.lease.clone());
        let (device, port) = if role == Role::Export {
            if !route.reversed_ips(&request.host_route) || offer.is_some() {
                return Err("export request route mismatch".into());
            }
            (
                select(&candidates(self.ops.inventory()?)?, serial)?.device,
                3240,
            )
        } else {
            let offer = offer.ok_or("export offer required")?;
            offer.validate()?;
            if offer.policy.role != Role::Export
                || offer.request != request
                || route != request.host_route
                || offer.policy.remote.port() != 3240
            {
                return Err("import offer/route mismatch".into());
            }
            (offer.policy.device, offer.policy.remote.port())
        };
        let plan = Plan {
            request,
            route: route.clone(),
            policy: Policy {
                role,
                client_uid: self.authorization.client_uid,
                client_gid: self.authorization.client_gid,
                device,
                remote: route.usb_endpoint(role, port),
            },
            lease: self.ops.token()?,
            peer_lease,
        };
        plan.validate()?;
        self.lease = Some(Lease {
            plan: plan.clone(),
            boot_id: self.ops.boot_id()?,
            stage: Stage::Planned,
            firewall: None,
            daemon: None,
            commit: None,
        });
        self.save(Stage::Planned)?;
        self.ops.prerequisites(&self.authorization)?;
        self.save(Stage::Prerequisites)?;
        self.ops.module(role)?;
        self.save(Stage::Module)?;
        if role == Role::Export {
            // Re-read inventory immediately before preparation; no stale selection replacement.
            let current = candidates(self.ops.inventory()?)?;
            if select(&current, Some(&plan.policy.device.serial))?.device != plan.policy.device {
                return Err("Tracker identity changed before preparation".into());
            }
            self.ops.export_available(&plan)?;
            self.save(Stage::FirewallPending)?;
            let firewall = self.ops.firewall(&plan)?;
            self.lease.as_mut().unwrap().firewall = Some(firewall);
            self.save(Stage::Firewall)?;
            self.save(Stage::DaemonPending)?;
            let daemon = self.ops.daemon(&plan)?;
            self.lease.as_mut().unwrap().daemon = Some(daemon);
        } else {
            self.ops.probe(plan.policy.remote)?;
        }
        self.save(Stage::Prepared)?;
        Ok(plan)
    }
    pub fn commit(&mut self, c: &Commit) -> KResult<Plan> {
        let l = self.lease.as_ref().ok_or("no prepared lease")?;
        if self.recovered
            || !c.validates(&l.plan)
            || l.boot_id != self.ops.boot_id()?
            || !matches!(l.stage, Stage::Prepared | Stage::Committed)
            || l.commit.as_ref().is_some_and(|old| old != c)
        {
            return Err("commit denied: stale/unprepared lease".into());
        }
        let plan = l.plan.clone();
        if l.stage == Stage::Prepared
            && plan.policy.role == Role::Export
            && select(
                &candidates(self.ops.inventory()?)?,
                Some(&plan.policy.device.serial),
            )?
            .device
                != plan.policy.device
        {
            return Err("Tracker changed before bootstrap commit".into());
        }
        self.lease.as_mut().unwrap().commit = Some(c.clone());
        self.save(Stage::Committed)?;
        Ok(plan)
    }
    pub fn release(
        &mut self,
        stamp: &Stamp,
        runtime_released: bool,
        usb_empty: bool,
    ) -> KResult<()> {
        let l = self.lease.as_ref().ok_or("no environment lease")?;
        if self.recovered
            || &l.plan.request.stamp != stamp
            || !runtime_released
            || !usb_empty
            || l.boot_id != self.ops.boot_id()?
        {
            return Err("environment release denied: ownership/runtime/USB journal".into());
        }
        // Pending mutations never guessed clean; an administrator must inspect them.
        if matches!(
            l.stage,
            Stage::FirewallPending | Stage::DaemonPending | Stage::Releasing
        ) {
            return Err("uncertain environment operation; inspection required".into());
        }
        let l = l.clone();
        self.save(Stage::Releasing)?;
        if !matches!(l.stage, Stage::DaemonStopped | Stage::FirewallRemoved) {
            if let Some(d) = &l.daemon {
                self.ops.stop_daemon(&l.plan, d)?;
            }
            self.save(Stage::DaemonStopped)?;
        }
        if l.stage != Stage::FirewallRemoved {
            if let Some(f) = &l.firewall {
                self.ops.remove_firewall(&l.plan, f)?;
            }
            self.save(Stage::FirewallRemoved)?;
        }
        self.journal.store(None)?;
        self.last_stamp = Some(l.plan.request.stamp);
        self.lease = None;
        Ok(())
    }
}
