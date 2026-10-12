//! Privileged helper's kernel orchestration. No automatic crash detach/adoption.
use crate::{
    command::Operation, inventory::SysfsInventory, ExportPlanner, ExportTarget, UsbDeviceIdentity,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    net::SocketAddr,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    thread,
    time::{Duration, Instant},
};
pub type KResult<T> = Result<T, String>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Export,
    Import,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub role: Role,
    pub client_uid: u32,
    pub client_gid: u32,
    pub device: UsbDeviceIdentity,
    pub remote: SocketAddr,
}
impl Policy {
    pub fn validate(&self) -> KResult<()> {
        let d = &self.device;
        if (d.vid, d.pid) == (0x2c30, 0x1050)
            || d.serial.is_empty()
            || d.busid.len() > 31
            || !valid_busid(&d.busid)
            || !d.sysfs_path.starts_with("/sys/devices/")
            || d.topology.is_empty()
            || self.remote.port() == 0
            || d.serial.len() > 128
            || d.sysfs_path.len() > 512
            || d.topology.len() > 128
            || [
                d.serial.as_str(),
                d.sysfs_path.as_str(),
                d.topology.as_str(),
            ]
            .iter()
            .any(|s| s.contains('\0') || s.contains('\n'))
        {
            return Err("invalid policy or forbidden HMD".into());
        }
        Ok(())
    }
}
pub fn valid_busid(s: &str) -> bool {
    let Some((bus, ports)) = s.split_once('-') else {
        return false;
    };
    !bus.is_empty()
        && bus.bytes().all(|b| b.is_ascii_digit())
        && !ports.is_empty()
        && ports
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub number: u32,
    pub status: u32,
    pub devid: u32,
    pub sockfd: u32,
    pub local_busid: String,
}
pub fn parse_status(s: &str) -> KResult<BTreeMap<u32, Port>> {
    let mut ports = BTreeMap::new();
    let mut lines = s.lines();
    if !lines.next().is_some_and(|s| {
        s.split_whitespace()
            .eq(["hub", "port", "sta", "spd", "dev", "sockfd", "local_busid"])
    }) {
        return Err("unsupported vhci status header".into());
    }
    for line in lines {
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() != 7 || !["hs", "ss"].contains(&columns[0]) {
            return Err("malformed vhci status".into());
        }
        let decimal = |n: usize| {
            columns[n]
                .parse::<u32>()
                .map_err(|_| "invalid vhci integer".to_owned())
        };
        let p = Port {
            number: decimal(1)?,
            status: decimal(2)?,
            devid: u32::from_str_radix(columns[4], 16).map_err(|_| "invalid devid")?,
            sockfd: decimal(5)?,
            local_busid: columns[6].into(),
        };
        decimal(3)?;
        if ![4, 5, 6, 7].contains(&p.status)
            || (p.status == 6 && !valid_busid(&p.local_busid))
            || ports.insert(p.number, p).is_some()
        {
            return Err("ambiguous/unsupported vhci port".into());
        }
    }
    if ports.is_empty() {
        return Err("empty vhci status".into());
    }
    Ok(ports)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stage {
    Binding,
    Bound,
    Attaching,
    Attached,
    Detaching,
    Unbinding,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    pub role: Role,
    pub instance: u64,
    pub session: u64,
    pub device: UsbDeviceIdentity,
    pub remote: SocketAddr,
    pub boot_id: String,
    pub stage: Stage,
    pub port: Option<Port>,
    pub previous_driver: Option<String>,
    pub previous_interfaces: Vec<(u8, Option<String>)>,
}
pub trait Journal: Send {
    fn load(&self) -> KResult<Option<Record>>;
    fn save(&mut self, record: Option<&Record>) -> KResult<()>;
}
pub trait KernelOps: Send {
    fn boot_id(&self) -> KResult<String>;
    fn validate_export(&self, policy: &Policy) -> KResult<Option<String>>;
    fn export_status(&self, device: &UsbDeviceIdentity) -> KResult<u32>;
    fn ports(&self) -> KResult<BTreeMap<u32, Port>>;
    fn imported_matches(&self, port: &Port, device: &UsbDeviceIdentity) -> KResult<bool>;
    fn run(&mut self, op: Operation) -> KResult<()>;
    fn interface_drivers(&self, _device: &UsbDeviceIdentity) -> KResult<Vec<(u8, Option<String>)>> {
        Ok(Vec::new())
    }
    fn restored(
        &self,
        device: &UsbDeviceIdentity,
        driver: &Option<String>,
        interfaces: &[(u8, Option<String>)],
    ) -> KResult<bool>;
}
pub struct Engine<O: KernelOps, J: Journal> {
    pub policy: Policy,
    pub ops: O,
    pub journal: J,
    record: Option<Record>,
    live_owner: bool,
}
impl<O: KernelOps, J: Journal> Engine<O, J> {
    pub fn new(policy: Policy, ops: O, journal: J) -> KResult<Self> {
        policy.validate()?;
        let record = journal.load()?;
        if record
            .as_ref()
            .is_some_and(|r| r.version != 1 || r.role != policy.role)
        {
            return Err("journal version/role mismatch".into());
        }
        Ok(Self {
            policy,
            ops,
            journal,
            record,
            live_owner: false,
        })
    }
    pub fn record(&self) -> Option<&Record> {
        self.record.as_ref()
    }
    fn persist(&mut self, record: Record) -> KResult<()> {
        self.journal.save(Some(&record))?;
        self.record = Some(record);
        Ok(())
    }
    fn owner(&self, instance: u64, session: u64) -> KResult<()> {
        let r = self.record.as_ref().ok_or("no owned resource")?;
        if !self.live_owner
            || r.instance != instance
            || r.session != session
            || r.device != self.policy.device
            || r.remote != self.policy.remote
            || r.boot_id != self.ops.boot_id()?
        {
            return Err(
                "ownership/recovery mismatch; inspect journal, no automatic adoption".into(),
            );
        }
        Ok(())
    }
    fn execute(&mut self, operation: Operation) -> KResult<()> {
        let result = self.ops.run(operation);
        if result.is_err() {
            self.live_owner = false;
        } // uncertain mutation freezes the lease until audit
        result
    }
    pub fn acquire(&mut self, instance: u64, session: u64) -> KResult<Option<u32>> {
        if instance == 0 || session == 0 {
            return Err("zero owner/session".into());
        }
        if let Some(r) = &self.record {
            self.owner(instance, session)?;
            return match r.stage {
                Stage::Bound if [1, 2].contains(&self.ops.export_status(&self.policy.device)?) => {
                    Ok(None)
                }
                Stage::Attached => {
                    let port = r.port.as_ref().ok_or("missing owned port")?;
                    if self.ops.ports()?.get(&port.number) != Some(port)
                        || !self.ops.imported_matches(port, &self.policy.device)?
                    {
                        return Err("owned import no longer matches".into());
                    }
                    Ok(Some(port.number))
                }
                _ => Err("pending operation requires inspection".into()),
            };
        }
        let previous_driver = if self.policy.role == Role::Export {
            self.ops.validate_export(&self.policy)?
        } else {
            None
        };
        let previous_interfaces = if self.policy.role == Role::Export {
            self.ops.interface_drivers(&self.policy.device)?
        } else {
            Vec::new()
        };
        let before = if self.policy.role == Role::Import {
            self.ops.ports()?
        } else {
            BTreeMap::new()
        };
        let record = Record {
            version: 1,
            role: self.policy.role,
            instance,
            session,
            device: self.policy.device.clone(),
            remote: self.policy.remote,
            boot_id: self.ops.boot_id()?,
            stage: if self.policy.role == Role::Export {
                Stage::Binding
            } else {
                Stage::Attaching
            },
            port: None,
            previous_driver,
            previous_interfaces,
        };
        self.persist(record)?;
        self.live_owner = true;
        match self.policy.role {
            Role::Export => {
                self.execute(Operation::Bind(self.policy.device.busid.clone()))?;
                if self.ops.export_status(&self.policy.device)? != 1 {
                    return Err("export did not become available".into());
                }
                let mut r = self.record.clone().unwrap();
                r.stage = Stage::Bound;
                self.persist(r)?;
                Ok(None)
            }
            Role::Import => {
                self.execute(Operation::Attach(
                    self.policy.remote,
                    self.policy.device.busid.clone(),
                ))?;
                let end = Instant::now() + Duration::from_secs(3);
                loop {
                    let after = self.ops.ports()?;
                    let changed: Vec<_> = after
                        .values()
                        .filter(|p| before.get(&p.number) != Some(*p))
                        .collect();
                    if changed.len() > 1 {
                        return Err("concurrent vhci changes; refuse to guess owned port".into());
                    }
                    if let Some(port) = changed.first() {
                        if before.get(&port.number).is_some_and(|p| p.status != 4) {
                            return Err("pre-existing port changed".into());
                        }
                        if port.status == 6
                            && self.ops.imported_matches(port, &self.policy.device)?
                        {
                            let mut r = self.record.clone().unwrap();
                            r.stage = Stage::Attached;
                            r.port = Some((*port).clone());
                            self.persist(r)?;
                            return Ok(Some(port.number));
                        }
                    }
                    if Instant::now() >= end {
                        return Err(
                            "USB enumeration timeout; keep pending journal for inspection".into(),
                        );
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    /// Runtime release is mandatory for import. Export requires detach-confirmed stop AND kernel available.
    pub fn release(&mut self, instance: u64, session: u64, released: bool) -> KResult<()> {
        if instance == 0 || session == 0 {
            return Err("zero owner/session".into());
        }
        if self.record.is_none() {
            return Ok(());
        }
        self.owner(instance, session)?;
        if !released {
            return Err("runtime/remote detach acknowledgement required".into());
        }
        let mut r = self.record.clone().unwrap();
        match self.policy.role {
            Role::Import => {
                let port = r
                    .port
                    .clone()
                    .ok_or("pending attach: owned port unknown, manual inspection required")?;
                if !matches!(r.stage, Stage::Attached | Stage::Detaching) {
                    return Err("invalid import release stage".into());
                }
                if self.ops.ports()?.get(&port.number) != Some(&port)
                    || !self.ops.imported_matches(&port, &self.policy.device)?
                {
                    return Err("port fingerprint changed; refuse detach".into());
                }
                r.stage = Stage::Detaching;
                self.persist(r)?;
                self.execute(Operation::Detach(port.number))?;
                let end = Instant::now() + Duration::from_secs(3);
                loop {
                    let current = self.ops.ports()?;
                    if current.get(&port.number).is_some_and(|p| p.status == 4) {
                        break;
                    }
                    if Instant::now() >= end {
                        return Err("detach enumeration timeout".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            Role::Export => {
                if !matches!(r.stage, Stage::Bound | Stage::Unbinding) {
                    return Err("pending bind needs manual inspection".into());
                }
                // STUB_ST_AVAILABLE=1; imported/busy=2 must never be unbound.
                if self.ops.export_status(&self.policy.device)? != 1 {
                    return Err("export busy/identity changed; wait for Host detach".into());
                }
                r.stage = Stage::Unbinding;
                self.persist(r.clone())?;
                self.execute(Operation::Unbind(self.policy.device.busid.clone()))?;
                let end = Instant::now() + Duration::from_secs(3);
                while !self.ops.restored(
                    &self.policy.device,
                    &r.previous_driver,
                    &r.previous_interfaces,
                )? {
                    if Instant::now() >= end {
                        return Err("local driver restoration unverified".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        self.journal.save(None)?;
        self.record = None;
        self.live_owner = false;
        Ok(())
    }
}
pub struct LinuxOps {
    pub cancel: Arc<AtomicBool>,
    pub remote: SocketAddr,
}
impl LinuxOps {
    fn device(&self, d: &UsbDeviceIdentity) -> KResult<crate::inventory::ObservedDevice> {
        let report = SysfsInventory::default()
            .scan()
            .map_err(|e| e.to_string())?;
        if !report.issues.is_empty() {
            return Err(format!("inventory incomplete: {:?}", report.issues));
        }
        report
            .devices
            .into_iter()
            .find(|v| &v.identity == d)
            .ok_or("exact export identity missing".into())
    }
}
impl KernelOps for LinuxOps {
    fn boot_id(&self) -> KResult<String> {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|s| s.trim().into())
            .map_err(|e| e.to_string())
    }
    fn validate_export(&self, policy: &Policy) -> KResult<Option<String>> {
        crate::command::executable().map_err(|e| format!("usbip prerequisite: {e}"))?;
        std::net::TcpStream::connect_timeout(&policy.remote, Duration::from_millis(500))
            .map_err(|e| format!("usbipd prerequisite at {}: {e}", policy.remote))?;
        let d = self.device(&policy.device)?;
        ExportPlanner {
            allowlist: vec![policy.device.clone()],
        }
        .plan(
            &SysfsInventory::default(),
            &ExportTarget::WholeDevice(policy.device.clone()),
        )
        .map_err(|e| format!("planner: {e:?}"))?;
        if d.via_vhci
            || d.device_driver.as_deref() != Some("usb")
            || fs::read_to_string(PathBuf::from(&policy.device.sysfs_path).join("bDeviceClass"))
                .map_err(|e| e.to_string())?
                .trim()
                == "09"
        {
            return Err("not an unowned local whole USB device".into());
        }
        if !std::path::Path::new("/sys/module/usbip_host").exists() {
            return Err("usbip_host not loaded".into());
        }
        Ok(d.device_driver)
    }
    fn export_status(&self, d: &UsbDeviceIdentity) -> KResult<u32> {
        if self.device(d)?.device_driver.as_deref() != Some("usbip-host") {
            return Err("export driver/identity mismatch".into());
        }
        fs::read_to_string(PathBuf::from(&d.sysfs_path).join("usbip_status"))
            .map_err(|e| e.to_string())?
            .trim()
            .parse()
            .map_err(|_| "bad usbip_status".into())
    }
    fn ports(&self) -> KResult<BTreeMap<u32, Port>> {
        crate::command::executable().map_err(|e| format!("usbip prerequisite: {e}"))?;
        if !std::path::Path::new("/sys/module/vhci_hcd").exists() {
            return Err("vhci_hcd not loaded".into());
        }
        let base = PathBuf::from("/sys/devices/platform/vhci_hcd.0");
        let mut ports = BTreeMap::new();
        for entry in fs::read_dir(&base).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "status"
                || name
                    .strip_prefix("status.")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            {
                for (n, p) in
                    parse_status(&fs::read_to_string(entry.path()).map_err(|e| e.to_string())?)?
                {
                    if ports.insert(n, p).is_some() {
                        return Err("duplicate controller ports".into());
                    }
                }
            }
        }
        if ports.is_empty() {
            return Err("no vhci status attributes".into());
        }
        Ok(ports)
    }
    fn imported_matches(&self, p: &Port, d: &UsbDeviceIdentity) -> KResult<bool> {
        let report = SysfsInventory::default()
            .scan()
            .map_err(|e| e.to_string())?;
        if !report.issues.is_empty() {
            return Ok(false);
        }
        let matches = report
            .devices
            .iter()
            .filter(|v| {
                v.via_vhci
                    && v.identity.busid == p.local_busid
                    && v.identity.vid == d.vid
                    && v.identity.pid == d.pid
                    && v.identity.serial == d.serial
            })
            .count();
        // Standard usbip CLI writes the remote address/port/busid record. Validate too.
        let record = fs::read_to_string(format!("/run/vhci_hcd/port{}", p.number))
            .map_err(|e| e.to_string())?;
        let columns: Vec<_> = record.split_whitespace().collect();
        let address = if columns.len() == 3 {
            if columns[0].contains(':') {
                format!("[{}]:{}", columns[0], columns[1])
            } else {
                format!("{}:{}", columns[0], columns[1])
            }
        } else {
            return Ok(false);
        };
        Ok(matches == 1
            && address.parse::<SocketAddr>().ok() == Some(self.remote)
            && columns[2] == d.busid)
    }
    fn run(&mut self, op: Operation) -> KResult<()> {
        crate::command::run(&op, Duration::from_secs(5), &self.cancel)
    }
    fn interface_drivers(&self, device: &UsbDeviceIdentity) -> KResult<Vec<(u8, Option<String>)>> {
        let mut drivers: Vec<_> = self
            .device(device)?
            .interfaces
            .into_iter()
            .map(|i| (i.number, i.driver))
            .collect();
        drivers.sort_by_key(|i| i.0);
        Ok(drivers)
    }
    fn restored(
        &self,
        d: &UsbDeviceIdentity,
        previous: &Option<String>,
        interfaces: &[(u8, Option<String>)],
    ) -> KResult<bool> {
        let observed = self.device(d)?;
        let mut actual: Vec<_> = observed
            .interfaces
            .into_iter()
            .map(|i| (i.number, i.driver))
            .collect();
        actual.sort_by_key(|i| i.0);
        Ok(&observed.device_driver == previous && actual == interfaces)
    }
}
pub struct FileJournal {
    pub path: PathBuf,
}
impl Journal for FileJournal {
    fn load(&self) -> KResult<Option<Record>> {
        match fs::read(&self.path) {
            Ok(b) if b.len() <= 8192 => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| e.to_string()),
            Ok(_) => Err("journal too large".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn save(&mut self, record: Option<&Record>) -> KResult<()> {
        use std::io::Write;
        let parent = self.path.parent().ok_or("journal requires parent")?;
        if let Some(r) = record {
            let temp = self.path.with_extension("new");
            // Crash-left temporary file is a blocker, never silently overwritten.
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = fs::OpenOptions::new()
                .mode(0o600)
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            f.write_all(&serde_json::to_vec(r).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temp, &self.path).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(&self.path).map_err(|e| e.to_string())?;
        }
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct MemoryJournal(Arc<std::sync::Mutex<Option<Record>>>, bool);
    impl Journal for MemoryJournal {
        fn load(&self) -> KResult<Option<Record>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&mut self, r: Option<&Record>) -> KResult<()> {
            if self.1 {
                return Err("journal fault".into());
            }
            *self.0.lock().unwrap() = r.cloned();
            Ok(())
        }
    }
    struct FakeOps {
        ports: BTreeMap<u32, Port>,
        calls: usize,
        busy: bool,
        boot: String,
        driver: Option<String>,
        concurrent: bool,
        fail: bool,
    }
    impl FakeOps {
        fn new() -> Self {
            Self {
                ports: [(
                    7,
                    Port {
                        number: 7,
                        status: 4,
                        devid: 0,
                        sockfd: 0,
                        local_busid: "0-0".into(),
                    },
                )]
                .into(),
                calls: 0,
                busy: false,
                boot: "boot-a".into(),
                driver: Some("usb".into()),
                concurrent: false,
                fail: false,
            }
        }
    }
    impl KernelOps for FakeOps {
        fn boot_id(&self) -> KResult<String> {
            Ok(self.boot.clone())
        }
        fn validate_export(&self, _: &Policy) -> KResult<Option<String>> {
            Ok(self.driver.clone())
        }
        fn export_status(&self, _: &UsbDeviceIdentity) -> KResult<u32> {
            Ok(if self.busy { 2 } else { 1 })
        }
        fn ports(&self) -> KResult<BTreeMap<u32, Port>> {
            Ok(self.ports.clone())
        }
        fn imported_matches(&self, _: &Port, _: &UsbDeviceIdentity) -> KResult<bool> {
            Ok(true)
        }
        fn restored(
            &self,
            _: &UsbDeviceIdentity,
            driver: &Option<String>,
            _interfaces: &[(u8, Option<String>)],
        ) -> KResult<bool> {
            Ok(&self.driver == driver)
        }
        fn run(&mut self, op: Operation) -> KResult<()> {
            self.calls += 1;
            if self.fail {
                return Err("command fault".into());
            }
            match op {
                Operation::Bind(_) => self.driver = Some("usbip-host".into()),
                Operation::Unbind(_) => self.driver = Some("usb".into()),
                Operation::Attach(..) => {
                    self.ports.insert(
                        7,
                        Port {
                            number: 7,
                            status: 6,
                            devid: 0x10003,
                            sockfd: 12,
                            local_busid: "3-1".into(),
                        },
                    );
                    if self.concurrent {
                        self.ports.insert(
                            8,
                            Port {
                                number: 8,
                                status: 6,
                                devid: 0x20003,
                                sockfd: 13,
                                local_busid: "4-1".into(),
                            },
                        );
                    }
                }
                Operation::Detach(p) => {
                    self.ports.insert(
                        p,
                        Port {
                            number: p,
                            status: 4,
                            devid: 0,
                            sockfd: 0,
                            local_busid: "0-0".into(),
                        },
                    );
                }
            };
            Ok(())
        }
    }
    fn policy(role: Role) -> Policy {
        let mut device = UsbDeviceIdentity::mock();
        device.busid = "1-3.3".into();
        device.sysfs_path = "/sys/devices/mock/1-3.3".into();
        Policy {
            role,
            client_uid: 1000,
            client_gid: 1000,
            device,
            remote: "192.0.2.1:3240".parse().unwrap(),
        }
    }
    #[test]
    fn bind_unbind_busy_and_owner_barriers() {
        let mut e = Engine::new(
            policy(Role::Export),
            FakeOps::new(),
            MemoryJournal::default(),
        )
        .unwrap();
        e.acquire(9, 1).unwrap();
        assert_eq!(e.acquire(9, 1).unwrap(), None);
        assert_eq!(e.ops.calls, 1);
        assert!(e.release(8, 1, true).is_err());
        assert!(e.release(9, 1, false).is_err());
        e.ops.busy = true;
        assert!(e.release(9, 1, true).is_err());
        assert_eq!(e.ops.calls, 1);
        e.ops.busy = false;
        e.release(9, 1, true).unwrap();
        assert!(e.record().is_none());
        assert_eq!(e.ops.driver.as_deref(), Some("usb"));
    }
    #[test]
    fn measured_port_fingerprint_and_runtime_release() {
        let mut e = Engine::new(
            policy(Role::Import),
            FakeOps::new(),
            MemoryJournal::default(),
        )
        .unwrap();
        assert_eq!(e.acquire(9, 1).unwrap(), Some(7));
        assert!(e.release(9, 1, false).is_err());
        let saved = e.ops.ports[&7].clone();
        e.ops.ports.get_mut(&7).unwrap().sockfd = 99;
        assert!(e.release(9, 1, true).is_err());
        assert_eq!(e.ops.calls, 1);
        e.ops.ports.insert(7, saved);
        e.release(9, 1, true).unwrap();
        assert_eq!(e.ops.ports[&7].status, 4);
        assert!(e.record().is_none());
    }
    #[test]
    fn journal_precedes_mutation_and_restart_never_adopts() {
        let journal = MemoryJournal(Default::default(), true);
        let mut e = Engine::new(policy(Role::Export), FakeOps::new(), journal).unwrap();
        assert!(e.acquire(9, 1).is_err());
        assert_eq!(e.ops.calls, 0);
        let j = MemoryJournal::default();
        let mut e = Engine::new(policy(Role::Export), FakeOps::new(), j.clone()).unwrap();
        e.acquire(9, 1).unwrap();
        let mut restarted = Engine::new(policy(Role::Export), FakeOps::new(), j).unwrap();
        assert!(restarted.release(9, 1, true).is_err());
        assert!(restarted.acquire(9, 1).is_err());
        assert_eq!(restarted.ops.calls, 0);
        e.ops.boot = "boot-b".into();
        assert!(e.release(9, 1, true).is_err());
    }
    #[test]
    fn ambiguous_attach_and_command_failure_keep_pending_record() {
        let mut ops = FakeOps::new();
        ops.concurrent = true;
        let mut e = Engine::new(policy(Role::Import), ops, MemoryJournal::default()).unwrap();
        assert!(e.acquire(9, 1).unwrap_err().contains("concurrent"));
        assert!(e.release(9, 1, true).is_err());
        assert!(e.record().unwrap().port.is_none());
        assert_eq!(e.ops.calls, 1);
        let mut ops = FakeOps::new();
        ops.fail = true;
        let mut e = Engine::new(policy(Role::Export), ops, MemoryJournal::default()).unwrap();
        assert!(e.acquire(9, 1).is_err());
        assert!(matches!(e.record().unwrap().stage, Stage::Binding));
    }
    #[test]
    fn strict_status_and_hmd_policy_denial() {
        let status =
            "hub port sta spd dev sockfd local_busid\nhs 0007 006 002 00010003 000012 3-1\n";
        assert_eq!(parse_status(status).unwrap()[&7].devid, 0x10003);
        assert!(parse_status(&(status.to_owned() + "hs 0007 004 000 0 0 0-0\n")).is_err());
        assert!(parse_status("unexpected header\n").is_err());
        let mut p = policy(Role::Export);
        p.device.vid = 0x2c30;
        p.device.pid = 0x1050;
        assert!(p.validate().is_err());
        assert!(!valid_busid("1-3;reboot"));
        assert!(!valid_busid("1-3:1.0"));
    }
    #[test]
    fn unauthorized_helper_request_cannot_mutate() {
        use crate::helper::{dispatch, Action, Request};
        let mut e = Engine::new(
            policy(Role::Export),
            FakeOps::new(),
            MemoryJournal::default(),
        )
        .unwrap();
        let r = Request {
            version: crate::helper::RPC_VERSION,
            compatibility_id: crate::helper::COMPATIBILITY_ID.into(),
            instance: 9,
            session: 1,
            action: Action::Acquire,
            device: e.policy.device.clone(),
            runtime_released: false,
        };
        assert!(dispatch(&mut e, 2000, r).result.is_err());
        assert_eq!(e.ops.calls, 0);
    }
    #[test]
    fn helper_compatibility_precedes_mutation_and_shutdown_preserves_ownership() {
        use crate::helper::{self, Action};
        let mut e = Engine::new(
            policy(Role::Export),
            FakeOps::new(),
            MemoryJournal::default(),
        )
        .unwrap();
        let uid = e.policy.client_uid;
        let describe =
            helper::dispatch(&mut e, uid, helper::describe_request(&policy(Role::Export)));
        assert!(describe.result.is_ok());
        helper::validate_response(&describe, &e.policy).unwrap();
        assert_eq!(e.ops.calls, 0);
        let mut incompatible = helper::describe_request(&e.policy);
        incompatible.action = Action::Acquire;
        incompatible.session = 1;
        incompatible.compatibility_id = "same-package-different-contract".into();
        assert!(helper::dispatch(&mut e, uid, incompatible).result.is_err());
        assert_eq!(e.ops.calls, 0);
        let mut old = helper::describe_request(&e.policy);
        old.version = 1;
        assert!(helper::dispatch(&mut e, uid, old).result.is_err());
        assert_eq!(e.ops.calls, 0);
        let mut shutdown = helper::describe_request(&e.policy);
        shutdown.action = Action::Shutdown;
        assert!(helper::dispatch(&mut e, uid, shutdown).result.is_err());
        let mut shutdown = helper::describe_request(&e.policy);
        shutdown.action = Action::Shutdown;
        shutdown.runtime_released = true;
        assert!(helper::dispatch(&mut e, uid, shutdown).result.is_ok());
        assert_eq!(e.ops.calls, 0);
        let mut acquire = helper::describe_request(&e.policy);
        acquire.action = Action::Acquire;
        acquire.session = 1;
        assert!(helper::dispatch(&mut e, uid, acquire).result.is_ok());
        let calls = e.ops.calls;
        let mut shutdown = helper::describe_request(&e.policy);
        shutdown.action = Action::Shutdown;
        shutdown.runtime_released = true;
        let denied = helper::dispatch(&mut e, uid, shutdown);
        assert!(denied.result.is_err());
        assert!(denied.pending);
        assert_eq!(e.ops.calls, calls);
        assert!(e.record().is_some());
    }
    #[test]
    fn file_journal_is_durable_and_crash_temp_blocks_overwrite() {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!(
            "llb-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let file = path.join("export.json");
        let mut e = Engine::new(
            policy(Role::Export),
            FakeOps::new(),
            FileJournal { path: file.clone() },
        )
        .unwrap();
        e.acquire(9, 1).unwrap();
        let loaded = e.journal.load().unwrap().unwrap();
        assert_eq!(loaded.instance, 9);
        assert!(matches!(loaded.stage, Stage::Bound));
        fs::write(file.with_extension("new"), b"crash-left").unwrap();
        assert!(e.release(9, 1, true).is_err());
        assert_eq!(e.ops.calls, 1); // fails before unbind
        fs::remove_file(file.with_extension("new")).unwrap();
        e.release(9, 1, true).unwrap();
        assert!(e.journal.load().unwrap().is_none());
        fs::remove_dir(path).unwrap();
    }
}
