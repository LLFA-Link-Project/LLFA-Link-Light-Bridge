//! Read-only USB/IP prerequisite and device diagnostic. No privileged operations.
use light_bridge_usbip::inventory::SysfsInventory;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn parse_device(value: &str) -> Result<(u16, u16), String> {
    let (vid, pid) = value
        .split_once(':')
        .ok_or("expected VID:PID, e.g. 28de:2300")?;
    if vid.len() != 4 || pid.len() != 4 {
        return Err("VID and PID must each contain four hex digits".into());
    }
    Ok((
        u16::from_str_radix(vid, 16).map_err(|e| e.to_string())?,
        u16::from_str_radix(pid, 16).map_err(|e| e.to_string())?,
    ))
}
fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let mut selected = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("Usage: usbip-probe [--device VID:PID]\nRead-only sysfs inventory and USB/IP prerequisites. Never binds or attaches devices.");
                return ExitCode::SUCCESS;
            }
            "--device" => {
                if selected.is_some() {
                    eprintln!("--device specified twice");
                    return ExitCode::from(2);
                }
                let Some(value) = args.next() else {
                    eprintln!("--device needs VID:PID");
                    return ExitCode::from(2);
                };
                match parse_device(&value) {
                    Ok(id) => selected = Some(id),
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::from(2);
                    }
                }
            }
            _ => {
                eprintln!("Unknown argument {arg:?}; use --help");
                return ExitCode::from(2);
            }
        }
    }
    match run(selected) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Probe incomplete: {e}");
            ExitCode::from(2)
        }
    }
}
fn executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path).map(|p| p.join(name)).find(|p| {
        let Ok(metadata) = fs::metadata(p) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            metadata.is_file()
        }
    })
}
fn presence(path: impl AsRef<Path>) -> String {
    match path.as_ref().try_exists() {
        Ok(true) => "present".into(),
        Ok(false) => "absent".into(),
        Err(e) => format!("unknown ({e})"),
    }
}
fn run(selected: Option<(u16, u16)>) -> io::Result<()> {
    println!("READ-ONLY REAL INVENTORY: this is not a USB/IP transfer test.");
    println!("Bridge transfer backend: Mock only; no bind/attach/detach is performed.\n");
    println!("USB/IP prerequisites (presence does not prove permissions or working transfer):");
    for name in ["lsusb", "usbip", "usbipd"] {
        let tool = executable(name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found in PATH".into());
        println!("  {name}: {tool}");
    }
    for name in ["usbip_host", "vhci_hcd"] {
        println!(
            "  loaded module {name}: {}",
            presence(Path::new("/sys/module").join(name))
        );
    }
    println!(
        "  /dev/bus/usb: {} (visibility only; device open not tested)",
        presence("/dev/bus/usb")
    );
    match fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(release) => {
            let release = release.trim();
            println!("  kernel: {release}");
            for module in ["usbip-host", "vhci-hcd"] {
                let mut found = Vec::new();
                for root in ["/usr/lib/modules", "/lib/modules"] {
                    for suffix in ["ko", "ko.zst", "ko.xz", "ko.gz"] {
                        let p = Path::new(root)
                            .join(release)
                            .join("kernel/drivers/usb/usbip")
                            .join(format!("{module}.{suffix}"));
                        if p.try_exists()? {
                            let real = fs::canonicalize(p)?;
                            if !found.contains(&real) {
                                found.push(real);
                            }
                        }
                    }
                }
                let text = if found.is_empty() {
                    "not found at standard module paths".into()
                } else {
                    found
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!("  module file {module}: {text}");
            }
        }
        Err(e) => println!("  kernel/module file check: unavailable ({e})"),
    }
    println!("\nUSB devices (root hubs omitted):");
    let report = SysfsInventory::default().scan()?;
    let mut count = 0;
    for observed in &report.devices {
        let d = &observed.identity;
        if selected.is_some_and(|id| id != (d.vid, d.pid)) {
            continue;
        }
        count += 1;
        println!("  {:04x}:{:04x} {}", d.vid, d.pid, observed.product);
        println!("    busid={} serial={:?}", d.busid, d.serial);
        println!("    sysfs_path={}", d.sysfs_path);
        println!(
            "    topology={} interfaces={}",
            d.topology, observed.interface_count
        );
        println!(
            "    device_driver={}",
            observed.device_driver.as_deref().unwrap_or("unbound")
        );
        println!(
            "    transport evidence: {}",
            if observed.via_vhci {
                "vhci_hcd ancestor observed; Bridge ownership/URB transfer unverified"
            } else {
                "no vhci_hcd ancestor; locally enumerated USB"
            }
        );
        if (d.vid, d.pid) == (0x2c30, 0x1050) {
            println!("    export policy: DENIED (arpara HMD control/IMU)");
        } else {
            println!("    export policy: not authorized by this probe; explicit identity allowlist required");
        }
        for interface in &observed.interfaces {
            println!(
                "    {} number={} driver={}",
                interface.name,
                interface.number,
                interface.driver.as_deref().unwrap_or("unbound")
            );
        }
    }
    for issue in &report.issues {
        eprintln!("  inventory issue: {issue}");
    }
    println!("\nMatched devices: {count}");
    println!("Compare in your terminal: lsusb; lsusb -t; usb-devices");
    if let Some((vid, pid)) = selected {
        println!("Filter: lsusb -d {vid:04x}:{pid:04x}");
    }
    println!("With usbip installed: usbip port (read-only imported-port list).");
    println!("A successful mock-session leaves OS USB enumeration unchanged.");
    if !report.issues.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "inventory has missing/malformed attributes or incomplete enumeration",
        ));
    }
    if selected.is_some() && count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no USB device matches the requested VID:PID",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filter_rejects_partial_and_invalid_ids() {
        assert_eq!(parse_device("28de:2300"), Ok((0x28de, 0x2300)));
        for s in ["28de", "28:2300", "28de:xyz0", "28de:2300:1"] {
            assert!(parse_device(s).is_err());
        }
    }
}
