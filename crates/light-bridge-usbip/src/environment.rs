//! Fixed privileged environment commands, ownership verification and durable intent.
use crate::{
    bootstrap::{self, Authorization, Daemon, Firewall, Journal, Lease, Ops, Plan},
    command,
    inventory::{InventoryReport, SysfsInventory},
    kernel::{KResult, Role},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::TcpListener,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
pub fn random_token() -> KResult<String> {
    let mut bytes = [0u8; 16];
    if unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) } != bytes.len() as isize
    {
        return Err("random identity unavailable".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn random_u64() -> KResult<u64> {
    loop {
        let token = random_token()?;
        let n = u64::from_str_radix(&token[..16], 16).unwrap();
        if n != 0 {
            return Ok(n);
        }
    }
}
fn fixed_program(paths: &[&str]) -> KResult<PathBuf> {
    for p in paths {
        let p = Path::new(p);
        if p.exists() {
            command::trusted(p).map_err(|e| e.to_string())?;
            return Ok(p.into());
        }
    }
    Err(format!("required tool missing: {}", paths.join(" or ")))
}
pub struct LinuxEnvironment {
    pub cancel: Arc<AtomicBool>,
}
/// Cached Arch transaction must contain only new approved tools/dependencies. No refresh,
/// kernel change, existing library upgrade or guessed distro transaction during connection.
pub fn arch_package_plan(
    preview: &str,
    installed: &std::collections::BTreeMap<String, String>,
) -> KResult<Vec<String>> {
    let allowed = [
        "usbip",
        "nftables",
        "kmod",
        "libnftnl",
        "libmnl",
        "libnfnetlink",
        "libpcap",
        "libusb",
        "libcap-ng",
        "nettle",
        "gmp",
        "readline",
        "ncurses",
        "libedit",
        "libnl",
    ];
    let mut names = Vec::new();
    for line in preview.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2 || !allowed.contains(&fields[0]) || fields[1].len() > 128 {
            return Err("Arch transaction includes unsupported dependency/system change; maintenance required".into());
        }
        if installed
            .get(fields[0])
            .is_some_and(|version| version != fields[1])
        {
            return Err("Arch transaction would upgrade installed packages; perform full system maintenance outside Bridge (no partial upgrade)".into());
        }
        if names.contains(&fields[0].to_string()) {
            return Err("duplicate transaction package".into());
        }
        names.push(fields[0].to_string());
    }
    if names.is_empty() {
        return Err(
            "tool files missing but no package transaction planned; inspect installation".into(),
        );
    }
    Ok(names)
}
impl LinuxEnvironment {
    fn run(&self, paths: &[&str], args: &[String], timeout: Duration) -> KResult<Vec<u8>> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(
                "environment commands require the explicitly authorized root helper".into(),
            );
        }
        command::capture(&fixed_program(paths)?, args, timeout, &self.cancel)
    }
    fn nft(&self, args: &[String]) -> KResult<Vec<u8>> {
        self.run(
            &["/usr/sbin/nft", "/usr/bin/nft"],
            args,
            Duration::from_secs(5),
        )
    }
    fn systemctl(&self, args: &[String]) -> KResult<Vec<u8>> {
        self.run(&["/usr/bin/systemctl"], args, Duration::from_secs(10))
    }
    fn table(plan: &Plan) -> KResult<String> {
        if !bootstrap::valid_token(&plan.lease) {
            return Err("invalid lease".into());
        }
        Ok(format!("llb_{}", plan.lease))
    }
    fn unit(plan: &Plan) -> KResult<String> {
        if !bootstrap::valid_token(&plan.lease) {
            return Err("invalid lease".into());
        }
        Ok(format!("llb-usbipd-{}.service", plan.lease))
    }
    fn firewall_snapshot(&self, plan: &Plan) -> KResult<Firewall> {
        let bytes = self.nft(&[
            "-j".into(),
            "-a".into(),
            "list".into(),
            "table".into(),
            "inet".into(),
            Self::table(plan)?,
        ])?;
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("nft JSON: {e}"))?;
        let objects = json["nftables"].as_array().ok_or("nft objects missing")?;
        let table = objects
            .iter()
            .find_map(|o| o.get("table"))
            .ok_or("nft table missing")?;
        if table["family"] != "inet"
            || table["name"] != Self::table(plan)?
            || table["comment"] != plan.lease
        {
            return Err("firewall ownership changed".into());
        }
        let handle = table["handle"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("nft handle missing")?;
        let snapshot = serde_json::Value::Array(
            objects
                .iter()
                .filter(|o| o.get("metainfo").is_none())
                .cloned()
                .collect(),
        );
        Ok(Firewall { handle, snapshot })
    }
    fn daemon_identity(&self, plan: &Plan) -> KResult<Daemon> {
        let pid = String::from_utf8(self.systemctl(&[
            "show".into(),
            "--property=MainPID".into(),
            "--value".into(),
            Self::unit(plan)?,
        ])?)
        .map_err(|e| e.to_string())?
        .trim()
        .parse::<u32>()
        .map_err(|_| "invalid managed daemon PID")?;
        if pid == 0 {
            return Err("managed daemon not running".into());
        }
        let executable = fixed_program(&["/usr/sbin/usbipd", "/usr/bin/usbipd"])?;
        if fs::canonicalize(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?
            != fs::canonicalize(executable).map_err(|e| e.to_string())?
        {
            return Err("managed daemon executable changed".into());
        }
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
        let tail = stat.rsplit_once(')').ok_or("bad daemon stat")?.1;
        let start_ticks = tail
            .split_whitespace()
            .nth(19)
            .ok_or("daemon start time absent")?
            .parse()
            .map_err(|_| "bad daemon start time")?;
        Ok(Daemon { pid, start_ticks })
    }
    fn daemon_listening(&self, plan: &Plan, daemon: &Daemon) -> KResult<bool> {
        let mut inodes = std::collections::BTreeSet::new();
        for f in fs::read_dir(format!("/proc/{}/fd", daemon.pid)).map_err(|e| e.to_string())? {
            let target =
                fs::read_link(f.map_err(|e| e.to_string())?.path()).map_err(|e| e.to_string())?;
            if let Some(inode) = target
                .to_string_lossy()
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
            {
                inodes.insert(inode.to_string());
            }
        }
        let table = if plan.route.local.is_ipv4() {
            "tcp"
        } else {
            "tcp6"
        };
        let text = fs::read_to_string(format!("/proc/{}/net/{table}", daemon.pid))
            .map_err(|e| e.to_string())?;
        Ok(text.lines().skip(1).any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.len() > 9
                && fields[3] == "0A"
                && fields[1]
                    .rsplit_once(':')
                    .and_then(|(_, p)| u16::from_str_radix(p, 16).ok())
                    == Some(plan.policy.remote.port())
                && inodes.contains(fields[9])
        }))
    }
}
pub fn firewall_script(plan: &Plan) -> KResult<String> {
    plan.validate()?;
    if plan.policy.role != Role::Export {
        return Err("firewall only on export".into());
    }
    let table = LinuxEnvironment::table(plan)?;
    let family = if plan.route.local.is_ipv4() {
        "ip"
    } else {
        "ip6"
    };
    let scope = match plan.route.local {
        std::net::SocketAddr::V6(v) => v.scope_id(),
        _ => 0,
    };
    let interface = if scope > 0 {
        format!("iif {scope} ")
    } else {
        String::new()
    };
    Ok(format!("create table inet {table} {{ comment \"{}\"; }}\nadd chain inet {table} input {{ type filter hook input priority -150; policy accept; }}\nadd rule inet {table} input {interface}{family} daddr {} {family} saddr {} tcp dport {} accept\nadd rule inet {table} input tcp dport {} drop\n",
        plan.lease,plan.route.local.ip(),plan.route.peer.ip(),plan.policy.remote.port(),plan.policy.remote.port()))
}
impl Ops for LinuxEnvironment {
    fn inventory(&mut self) -> KResult<InventoryReport> {
        SysfsInventory::default().scan().map_err(|e| e.to_string())
    }
    fn token(&mut self) -> KResult<String> {
        random_token()
    }
    fn boot_id(&self) -> KResult<String> {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|s| s.trim().into())
            .map_err(|e| e.to_string())
    }
    fn prerequisites(&mut self, auth: &Authorization) -> KResult<()> {
        let available = command::executable().is_ok()
            && fixed_program(&["/usr/sbin/modprobe", "/usr/bin/modprobe"]).is_ok()
            && (auth.role != Role::Export
                || (fixed_program(&["/usr/sbin/usbipd", "/usr/bin/usbipd"]).is_ok()
                    && fixed_program(&["/usr/sbin/nft", "/usr/bin/nft"]).is_ok()));
        if !available {
            if !auth.allow_package_install {
                return Err(
                    "missing USB/IP prerequisites: administrator package-install approval required"
                        .into(),
                );
            }
            let os = fs::read_to_string("/etc/os-release").map_err(|e| e.to_string())?;
            let id = os
                .lines()
                .find_map(|l| l.strip_prefix("ID="))
                .unwrap_or("")
                .trim_matches('"');
            let args:Vec<String>=match id {
                "debian"=>["install","-y","usbip","nftables","kmod"].into_iter().map(String::from).collect(),
                "arch"=>{
                    let preview=String::from_utf8(self.run(&["/usr/bin/pacman"],&["-Sp".into(),"--needed".into(),"--print-format".into(),"%n %v".into(),"usbip".into(),"nftables".into(),"kmod".into()],Duration::from_secs(10))?).map_err(|e|e.to_string())?;
                    let names=arch_package_plan(&preview,&std::collections::BTreeMap::new())?;
                    let mut installed=std::collections::BTreeMap::new();
                    for name in names {
                        match self.run(&["/usr/bin/pacman"],&["-Q".into(),name.clone()],Duration::from_secs(5)) {
                            Ok(b)=>{let text=String::from_utf8(b).map_err(|e|e.to_string())?;let fields:Vec<_>=text.split_whitespace().collect();if fields.len()!=2||fields[0]!=name{return Err("installed package metadata invalid".into());}installed.insert(name,fields[1].to_string());},
                            Err(e) if e.contains(&format!("error: package '{name}' was not found"))=>(),Err(e)=>return Err(format!("package state unknown: {e}"))
                        }
                    }
                    arch_package_plan(&preview,&installed)?;
                    ["-S","--needed","--noconfirm","usbip","nftables","kmod"].into_iter().map(String::from).collect()
                },
                "ubuntu"=>["install","-y","linux-tools-common","linux-tools-generic","nftables","kmod"].into_iter().map(String::from).collect(),
                _=>return Err("automatic package provisioning unsupported for this distro/kernel; inspect tools (no kernel replacement)".into())
            };
            self.run(
                if id == "arch" {
                    &["/usr/bin/pacman"]
                } else {
                    &["/usr/bin/apt-get"]
                },
                &args,
                Duration::from_secs(120),
            )?;
        }
        self.run(
            &["/usr/bin/usbip", "/usr/sbin/usbip"],
            &["version".into()],
            Duration::from_secs(5),
        )?;
        fixed_program(&["/usr/bin/systemd-run"])?;
        fixed_program(&["/usr/bin/systemctl"])?;
        Ok(())
    }
    fn module(&mut self, role: Role) -> KResult<()> {
        let (name, ready) = match role {
            Role::Export => ("usbip_host", Path::new("/sys/bus/usb/drivers/usbip-host")),
            Role::Import => ("vhci_hcd", Path::new("/sys/devices/platform/vhci_hcd.0")),
        };
        if !ready.exists() {
            self.run(
                &["/usr/sbin/modprobe", "/usr/bin/modprobe"],
                &[name.into()],
                Duration::from_secs(10),
            )?;
        }
        if !ready.exists() {
            return Err("USB/IP kernel feature unavailable; compatible kernel required (not replaced automatically)".into());
        }
        Ok(())
    }
    fn firewall(&mut self, plan: &Plan) -> KResult<Firewall> {
        // A foreign daemon/port must be rejected before changing network access.
        let reservation = TcpListener::bind(plan.policy.remote)
            .map_err(|e| format!("USB/IP listen port unavailable: {e}"))?;
        drop(reservation);
        let path = PathBuf::from(format!(
            "/run/llfa-link-light-bridge/nft-{}.rules",
            plan.lease
        ));
        command::trusted(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(firewall_script(plan)?.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        self.nft(&["-f".into(), path.to_string_lossy().into_owned()])?;
        let firewall = self.firewall_snapshot(plan)?;
        fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(firewall)
    }
    fn export_available(&mut self, plan: &Plan) -> KResult<()> {
        let reservation = TcpListener::bind(plan.policy.remote)
            .map_err(|e| format!("USB/IP listen port occupied; no foreign daemon adopted: {e}"))?;
        drop(reservation);
        Ok(())
    }
    fn daemon(&mut self, plan: &Plan) -> KResult<Daemon> {
        let program = fixed_program(&["/usr/sbin/usbipd", "/usr/bin/usbipd"])?;
        let args = vec![
            format!("--unit={}", Self::unit(plan)?),
            "--collect".into(),
            "--service-type=exec".into(),
            "--property=User=root".into(),
            "--property=Group=root".into(),
            "--property=Restart=no".into(),
            "--property=NoNewPrivileges=yes".into(),
            program.to_string_lossy().into_owned(),
            if plan.route.local.is_ipv4() {
                "-4".into()
            } else {
                "-6".into()
            },
            "--tcp-port".into(),
            plan.policy.remote.port().to_string(),
        ];
        self.run(&["/usr/bin/systemd-run"], &args, Duration::from_secs(10))?;
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(d) = self.daemon_identity(plan) {
                if self.daemon_listening(plan, &d)? {
                    return Ok(d);
                }
            }
            if Instant::now() >= end {
                return Err("managed usbipd listen not verified; retain pending lease".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn probe(&mut self, endpoint: std::net::SocketAddr) -> KResult<()> {
        std::net::TcpStream::connect_timeout(&endpoint, Duration::from_secs(3))
            .map(|_| ())
            .map_err(|e| format!("Pi USB/IP reverse route blocked: {e}"))
    }
    fn stop_daemon(&mut self, plan: &Plan, daemon: &Daemon) -> KResult<()> {
        let actual = self.daemon_identity(plan)?;
        if actual.pid != daemon.pid
            || actual.start_ticks != daemon.start_ticks
            || !self.daemon_listening(plan, &actual)?
        {
            return Err("managed daemon ownership changed; no stop".into());
        }
        self.systemctl(&["stop".into(), LinuxEnvironment::unit(plan)?])?;
        // Replaced/reused PIDs are not ours; do not signal them. Confirm the original instance vanished.
        match fs::read_to_string(format!("/proc/{}/stat", daemon.pid)) {
            Ok(stat) => {
                let tail = stat
                    .rsplit_once(')')
                    .ok_or("invalid remaining daemon stat")?
                    .1;
                let ticks = tail
                    .split_whitespace()
                    .nth(19)
                    .ok_or("remaining daemon start time missing")?
                    .parse::<u64>()
                    .map_err(|_| "bad remaining daemon start time")?;
                if ticks == daemon.start_ticks {
                    return Err("original daemon process still exists after stop".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("daemon disappearance unconfirmed: {e}")),
        }
        if TcpListener::bind(plan.policy.remote).is_err() {
            return Err("USB/IP port still busy after stop; inspect lease".into());
        }
        Ok(())
    }
    fn remove_firewall(&mut self, plan: &Plan, firewall: &Firewall) -> KResult<()> {
        let current = self.firewall_snapshot(plan)?;
        if current.handle != firewall.handle || current.snapshot != firewall.snapshot {
            return Err("firewall fingerprint changed; no deletion".into());
        }
        self.nft(&[
            "delete".into(),
            "table".into(),
            "inet".into(),
            "handle".into(),
            firewall.handle.to_string(),
        ])?;
        let json: serde_json::Value =
            serde_json::from_slice(&self.nft(&["-j".into(), "list".into(), "tables".into()])?)
                .map_err(|e| e.to_string())?;
        let objects = json["nftables"]
            .as_array()
            .ok_or("table-removal proof missing")?;
        if objects.iter().filter_map(|o| o.get("table")).any(|t| {
            t["family"] == "inet" && t["name"] == LinuxEnvironment::table(plan).unwrap_or_default()
        }) {
            return Err("firewall removal not confirmed".into());
        }
        Ok(())
    }
}
pub struct FileEnvironmentJournal {
    pub path: PathBuf,
}
impl Journal for FileEnvironmentJournal {
    fn load(&self) -> KResult<Option<Lease>> {
        if self.path.with_extension("json.new").exists() {
            return Err("uncertain environment journal write; inspect .new".into());
        }
        match fs::read(&self.path) {
            Ok(b) => {
                if b.len() > 65536 {
                    return Err("environment journal too large".into());
                }
                serde_json::from_slice(&b)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn store(&mut self, lease: Option<&Lease>) -> KResult<()> {
        let parent = self.path.parent().ok_or("journal parent missing")?;
        if let Some(l) = lease {
            let bytes = serde_json::to_vec(l).map_err(|e| e.to_string())?;
            if bytes.len() > 65536 {
                return Err("environment journal too large".into());
            }
            let tmp = self.path.with_extension("json.new");
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&tmp)
                .map_err(|e| e.to_string())?;
            f.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            f.write_all(&bytes).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
            fs::rename(tmp, &self.path).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(&self.path).map_err(|e| e.to_string())?;
        }
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
}
