//! Read-only Linux sysfs inventory. This does not export, attach or open USB devices.
use crate::{UsbDeviceIdentity, UsbInventory};
use light_bridge_core::{Error, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct UsbInterface {
    pub name: String,
    pub number: u8,
    pub driver: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ObservedDevice {
    pub identity: UsbDeviceIdentity,
    pub product: String,
    pub interface_count: u8,
    pub interfaces: Vec<UsbInterface>,
    pub device_driver: Option<String>,
    pub via_vhci: bool,
}
#[derive(Debug, Default, Clone)]
pub struct InventoryReport {
    pub devices: Vec<ObservedDevice>,
    /// Missing attributes, unplug races and permission errors are visible, never silently ignored.
    pub issues: Vec<String>,
}
pub struct SysfsInventory {
    root: PathBuf,
}
impl Default for SysfsInventory {
    fn default() -> Self {
        Self::new("/sys/bus/usb/devices")
    }
}
impl SysfsInventory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn scan(&self) -> io::Result<InventoryReport> {
        let mut entries = fs::read_dir(&self.root)?
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        let mut report = InventoryReport::default();
        for path in &entries {
            let Some(busid) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Interface nodes and root hubs are omitted; external device busids start with digits.
            if busid.contains(':') || !busid.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                continue;
            }
            match self.observe(path, busid, &entries) {
                Ok(device) => report.devices.push(device),
                Err(e) => report.issues.push(format!("{}: {e}", path.display())),
            }
        }
        Ok(report)
    }
    fn observe(&self, path: &Path, busid: &str, entries: &[PathBuf]) -> io::Result<ObservedDevice> {
        let vid = hex_attribute::<u16>(path, "idVendor")?;
        let pid = hex_attribute::<u16>(path, "idProduct")?;
        let realpath = fs::canonicalize(path)?;
        let busnum = attribute(path, "busnum")?;
        let devpath = attribute(path, "devpath")?;
        let identity = UsbDeviceIdentity {
            vid,
            pid,
            serial: optional_attribute(path, "serial")?.unwrap_or_default(),
            busid: busid.into(),
            sysfs_path: realpath.to_string_lossy().into_owned(),
            topology: format!("bus{busnum}/port{devpath}"),
        };
        let interface_count = attribute(path, "bNumInterfaces")?
            .parse::<u8>()
            .map_err(invalid_data)?;
        let prefix = format!("{busid}:");
        let mut interfaces = Vec::new();
        for interface in entries.iter().filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        }) {
            interfaces.push(UsbInterface {
                name: interface
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                number: hex_attribute::<u8>(interface, "bInterfaceNumber")?,
                driver: driver(interface)?,
            });
        }
        if interfaces.len() != usize::from(interface_count) {
            return Err(io::Error::new(io::ErrorKind::InvalidData,format!("declared {interface_count} interfaces, observed {}; enumeration may be incomplete",interfaces.len())));
        }
        let via_vhci = realpath
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with("vhci_hcd"));
        Ok(ObservedDevice {
            identity,
            product: optional_attribute(path, "product")?.unwrap_or_default(),
            interface_count,
            interfaces,
            device_driver: driver(path)?,
            via_vhci,
        })
    }
}
impl UsbInventory for SysfsInventory {
    fn devices(&self) -> Result<Vec<UsbDeviceIdentity>> {
        let report = self.scan().map_err(|_| Error::Inventory)?;
        if !report.issues.is_empty() {
            return Err(Error::Inventory);
        }
        Ok(report.devices.into_iter().map(|d| d.identity).collect())
    }
}
fn invalid_data(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}
fn attribute(path: &Path, name: &str) -> io::Result<String> {
    fs::read_to_string(path.join(name))
        .map(|s| s.trim().to_owned())
        .map_err(|e| io::Error::new(e.kind(), format!("{name}: {e}")))
}
fn optional_attribute(path: &Path, name: &str) -> io::Result<Option<String>> {
    match fs::read_to_string(path.join(name)) {
        Ok(s) => Ok(Some(s.trim().to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn hex_attribute<T: HexAttribute>(path: &Path, name: &str) -> io::Result<T> {
    T::parse(&attribute(path, name)?).map_err(|e| invalid_data(format!("{name}: {e}")))
}
trait HexAttribute: Sized {
    fn parse(s: &str) -> std::result::Result<Self, std::num::ParseIntError>;
}
impl HexAttribute for u16 {
    fn parse(s: &str) -> std::result::Result<Self, std::num::ParseIntError> {
        u16::from_str_radix(s, 16)
    }
}
impl HexAttribute for u8 {
    fn parse(s: &str) -> std::result::Result<Self, std::num::ParseIntError> {
        u8::from_str_radix(s, 16)
    }
}
fn driver(path: &Path) -> io::Result<Option<String>> {
    match fs::read_link(path.join("driver")) {
        Ok(p) => Ok(p.file_name().map(|n| n.to_string_lossy().into_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{ExportPlanner, ExportTarget};
    use std::{
        os::unix::fs::symlink,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "llb-inventory-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn device(&self, id: &str, vid: &str, pid: &str) {
            let p = self.0.join(id);
            fs::create_dir(&p).unwrap();
            for (k, v) in [
                ("idVendor", vid),
                ("idProduct", pid),
                ("busnum", "1"),
                ("devpath", "3.3"),
                ("bNumInterfaces", "1"),
                ("product", "Test Tracker"),
            ] {
                fs::write(p.join(k), format!("{v}\n")).unwrap();
            }
            let i = self.0.join(format!("{id}:1.0"));
            fs::create_dir(&i).unwrap();
            fs::write(i.join("bInterfaceNumber"), "00\n").unwrap();
            symlink("/sys/bus/usb/drivers/usbhid", i.join("driver")).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn missing_serial_and_hmd_never_become_exportable() {
        let f = Fixture::new();
        f.device("1-3.3", "28de", "2300");
        f.device("1-4", "2c30", "1050");
        let inventory = SysfsInventory::new(&f.0);
        let report = inventory.scan().unwrap();
        assert!(report.issues.is_empty());
        assert_eq!(report.devices.len(), 2);
        assert_eq!(
            report.devices[0].interfaces[0].driver.as_deref(),
            Some("usbhid")
        );
        let d = report.devices[0].identity.clone();
        let planner = ExportPlanner {
            allowlist: vec![d.clone()],
        };
        assert_eq!(
            planner.plan(&inventory, &ExportTarget::WholeDevice(d)),
            Err(Error::DeviceDenied)
        );
        fs::write(f.0.join("1-4/serial"), "HMD").unwrap();
        let d = inventory
            .devices()
            .unwrap()
            .into_iter()
            .find(|d| d.vid == 0x2c30)
            .unwrap();
        let planner = ExportPlanner {
            allowlist: vec![d.clone()],
        };
        assert_eq!(
            planner.plan(&inventory, &ExportTarget::WholeDevice(d)),
            Err(Error::DeviceDenied)
        );
    }
    #[test]
    fn incomplete_and_malformed_inventory_fail_closed() {
        let f = Fixture::new();
        f.device("1-3.3", "28de", "2300");
        let inventory = SysfsInventory::new(&f.0);
        fs::write(f.0.join("1-3.3/idProduct"), "not-hex").unwrap();
        let r = inventory.scan().unwrap();
        assert!(r.devices.is_empty());
        assert!(r.issues[0].contains("idProduct"));
        assert_eq!(inventory.devices(), Err(Error::Inventory));
        fs::write(f.0.join("1-3.3/idProduct"), "2300").unwrap();
        fs::write(f.0.join("1-3.3/bNumInterfaces"), "3").unwrap();
        assert!(inventory.scan().unwrap().issues[0].contains("enumeration may be incomplete"));
        assert!(SysfsInventory::new(f.0.join("absent")).scan().is_err());
    }
    #[test]
    fn vhci_ancestry_and_exact_allowlist() {
        let f = Fixture::new();
        f.device("1-3.3", "28de", "2300");
        fs::write(f.0.join("1-3.3/serial"), "TRACKER").unwrap();
        let real = f.0.join("vhci_hcd.0");
        fs::create_dir(&real).unwrap();
        fs::rename(f.0.join("1-3.3"), real.join("1-3.3")).unwrap();
        symlink(real.join("1-3.3"), f.0.join("1-3.3")).unwrap();
        let inventory = SysfsInventory::new(&f.0);
        let report = inventory.scan().unwrap();
        assert!(report.devices[0].via_vhci);
        let d = report.devices[0].identity.clone();
        let planner = ExportPlanner {
            allowlist: vec![d.clone()],
        };
        assert_eq!(
            planner.plan(&inventory, &ExportTarget::WholeDevice(d.clone())),
            Ok(d)
        );
    }
}
