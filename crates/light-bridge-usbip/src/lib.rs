//! USB inventory/planning, Mock transfer, and helper-mediated Linux kernel USB/IP.
#[cfg(target_os = "linux")]
pub mod auto_worker;
#[cfg(target_os = "linux")]
pub mod bootstrap;
#[cfg(target_os = "linux")]
pub mod bootstrap_rpc;
#[cfg(target_os = "linux")]
pub mod command;
#[cfg(target_os = "linux")]
pub mod environment;
#[cfg(target_os = "linux")]
pub mod fd_channel;
#[cfg(target_os = "linux")]
pub mod helper;
pub mod inventory;
#[cfg(target_os = "linux")]
pub mod kernel;
pub mod manager;
use light_bridge_core::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbDeviceIdentity {
    pub vid: u16,
    pub pid: u16,
    pub serial: String,
    pub busid: String,
    pub sysfs_path: String,
    pub topology: String,
}
impl UsbDeviceIdentity {
    pub fn mock() -> Self {
        Self {
            vid: 0xffff,
            pid: 1,
            serial: "MOCK-TRACKER".into(),
            busid: "mock-1".into(),
            sysfs_path: "/mock/usb/1".into(),
            topology: "mock-port-1".into(),
        }
    }
}
pub trait UsbInventory {
    fn devices(&self) -> Result<Vec<UsbDeviceIdentity>>;
}
#[derive(Debug, Clone)]
pub enum ExportTarget {
    WholeDevice(UsbDeviceIdentity),
    Interfaces {
        device: UsbDeviceIdentity,
        numbers: Vec<u8>,
    },
}
pub struct ExportPlanner {
    pub allowlist: Vec<UsbDeviceIdentity>,
}
impl ExportPlanner {
    pub fn plan(
        &self,
        inventory: &dyn UsbInventory,
        target: &ExportTarget,
    ) -> Result<UsbDeviceIdentity> {
        let d = match target {
            ExportTarget::WholeDevice(d) => d,
            _ => return Err(Error::UnsupportedExportGranularity),
        };
        if (d.vid, d.pid) == (0x2c30, 0x1050)
            || d.serial.is_empty()
            || d.busid.is_empty()
            || d.sysfs_path.is_empty()
            || d.topology.is_empty()
            || !self.allowlist.contains(d)
        {
            return Err(Error::DeviceDenied);
        }
        let matches = inventory.devices()?.into_iter().filter(|v| v == d).count();
        match matches {
            1 => Ok(d.clone()),
            0 => Err(Error::DeviceDenied),
            _ => Err(Error::AmbiguousDevice),
        }
    }
}
pub trait UsbTransferBackend: Send {
    fn export(&mut self, session: u64, device: &UsbDeviceIdentity) -> Result<()>;
    /// Must return only after enumeration. Port is an owned token, never guessed.
    fn attach(&mut self, session: u64, device: &UsbDeviceIdentity) -> Result<u32>;
    fn detach(&mut self, session: u64, port: u32) -> Result<()>;
    fn unexport(&mut self, session: u64) -> Result<()>;
}
#[derive(Debug, Default)]
pub struct MockUsbIp {
    pub fail_export: bool,
    pub fail_attach: bool,
    pub enumeration_ready: bool,
    pub inventory: Vec<UsbDeviceIdentity>,
    exports: BTreeMap<u64, UsbDeviceIdentity>,
    ports: BTreeMap<u32, u64>,
    next_port: u32,
    remote_mock: bool,
    remote_offers: BTreeSet<u64>,
}
impl MockUsbIp {
    pub fn ready() -> Self {
        Self {
            enumeration_ready: true,
            inventory: vec![UsbDeviceIdentity::mock()],
            ..Self::default()
        }
    }
    /// Independent Host-side mock ledger. Only the fixed synthetic tracker is accepted.
    pub fn remote_importer() -> Self {
        Self {
            remote_mock: true,
            ..Self::ready()
        }
    }
    /// Called only after the current session's UsbExportReady protocol message.
    /// This records mock metadata, never an OS export or authorization for real USB.
    pub fn observe_export_ready(&mut self, session: u64, device: &UsbDeviceIdentity) -> Result<()> {
        if !self.remote_mock {
            return Ok(());
        }
        if session == 0 || device != &UsbDeviceIdentity::mock() {
            return Err(Error::DeviceDenied);
        }
        if self.remote_offers.iter().any(|owner| *owner != session) {
            return Err(Error::Ownership);
        }
        self.remote_offers.insert(session);
        Ok(())
    }
    /// Clear a failed remote mock attach offer after runtime release; no HMD export operation.
    pub fn cancel_remote_offer(&mut self, session: u64) -> Result<()> {
        if self.ports.values().any(|owner| *owner == session) {
            return Err(Error::Ownership);
        }
        self.remote_offers.remove(&session);
        Ok(())
    }
    pub fn resource_count(&self) -> usize {
        self.exports.len() + self.ports.len()
    }
}
impl UsbInventory for MockUsbIp {
    fn devices(&self) -> Result<Vec<UsbDeviceIdentity>> {
        Ok(self.inventory.clone())
    }
}
impl UsbTransferBackend for MockUsbIp {
    fn export(&mut self, s: u64, d: &UsbDeviceIdentity) -> Result<()> {
        ExportPlanner {
            allowlist: self.inventory.clone(),
        }
        .plan(self, &ExportTarget::WholeDevice(d.clone()))?;
        if s == 0 {
            return Err(Error::InvalidArgument);
        }
        if self.fail_export {
            return Err(Error::UsbExport);
        }
        if self.exports.iter().any(|(owner, v)| v == d && *owner != s) {
            return Err(Error::Ownership);
        }
        if let Some(existing) = self.exports.get(&s) {
            return if existing == d {
                Ok(())
            } else {
                Err(Error::Ownership)
            };
        }
        self.exports.insert(s, d.clone());
        Ok(())
    }
    fn attach(&mut self, s: u64, d: &UsbDeviceIdentity) -> Result<u32> {
        if self.exports.get(&s) != Some(d)
            && !(self.remote_mock
                && self.remote_offers.contains(&s)
                && d == &UsbDeviceIdentity::mock())
        {
            return Err(Error::Ownership);
        }
        if self.fail_attach {
            return Err(Error::UsbAttach);
        }
        if !self.enumeration_ready {
            return Err(Error::Enumeration);
        }
        if let Some((&p, _)) = self.ports.iter().find(|(_, owner)| **owner == s) {
            return Ok(p);
        }
        self.next_port += 1;
        self.ports.insert(self.next_port, s);
        Ok(self.next_port)
    }
    fn detach(&mut self, s: u64, p: u32) -> Result<()> {
        match self.ports.get(&p) {
            Some(owner) if *owner == s => {
                self.ports.remove(&p);
                self.remote_offers.remove(&s);
                Ok(())
            }
            _ => Err(Error::Ownership),
        }
    }
    fn unexport(&mut self, s: u64) -> Result<()> {
        if self.ports.values().any(|owner| *owner == s) {
            return Err(Error::Ownership);
        }
        self.remote_offers.remove(&s);
        self.exports.remove(&s);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_granularity() {
        let mut b = MockUsbIp::ready();
        let mut d = UsbDeviceIdentity::mock();
        d.vid = 0x2c30;
        d.pid = 0x1050;
        b.inventory.push(d.clone());
        assert_eq!(b.export(1, &d), Err(Error::DeviceDenied));
        let d = UsbDeviceIdentity::mock();
        let planner = ExportPlanner {
            allowlist: vec![d.clone()],
        };
        assert_eq!(
            planner.plan(
                &b,
                &ExportTarget::Interfaces {
                    device: d.clone(),
                    numbers: vec![0]
                }
            ),
            Err(Error::UnsupportedExportGranularity)
        );
        b.inventory.push(d.clone());
        assert_eq!(
            planner.plan(&b, &ExportTarget::WholeDevice(d)),
            Err(Error::AmbiguousDevice)
        );
    }
    #[test]
    fn remote_mock_requires_export_ready_and_denies_real_devices() {
        let mut b = MockUsbIp::remote_importer();
        let d = UsbDeviceIdentity::mock();
        assert_eq!(b.attach(1, &d), Err(Error::Ownership));
        let mut real = d.clone();
        real.vid = 0x2c30;
        real.pid = 0x1050;
        assert_eq!(b.observe_export_ready(1, &real), Err(Error::DeviceDenied));
        assert_eq!(b.observe_export_ready(0, &d), Err(Error::DeviceDenied));
        b.observe_export_ready(1, &d).unwrap();
        assert_eq!(b.attach(2, &d), Err(Error::Ownership));
        let p = b.attach(1, &d).unwrap();
        assert_eq!(b.detach(2, p), Err(Error::Ownership));
        b.detach(1, p).unwrap();
        assert_eq!(b.attach(1, &d), Err(Error::Ownership));
        assert_eq!(b.resource_count(), 0);
    }
    #[test]
    fn ownership_failures_cleanup() {
        let mut b = MockUsbIp::ready();
        let d = UsbDeviceIdentity::mock();
        b.fail_export = true;
        assert_eq!(b.export(1, &d), Err(Error::UsbExport));
        b.fail_export = false;
        b.export(1, &d).unwrap();
        assert_eq!(b.export(2, &d), Err(Error::Ownership));
        b.fail_attach = true;
        assert_eq!(b.attach(1, &d), Err(Error::UsbAttach));
        b.fail_attach = false;
        b.enumeration_ready = false;
        assert_eq!(b.attach(1, &d), Err(Error::Enumeration));
        b.enumeration_ready = true;
        let p = b.attach(1, &d).unwrap();
        assert_eq!(b.attach(1, &d), Ok(p));
        assert_eq!(b.detach(2, p), Err(Error::Ownership));
        assert_eq!(b.unexport(1), Err(Error::Ownership));
        b.detach(1, p).unwrap();
        b.unexport(1).unwrap();
        assert_eq!(b.resource_count(), 0);
    }
}
