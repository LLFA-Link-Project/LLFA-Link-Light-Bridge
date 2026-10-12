#![cfg(target_os = "linux")]
use light_bridge_usbip::{
    bootstrap::*,
    bootstrap_rpc,
    environment::{firewall_script, FileEnvironmentJournal},
    fd_channel,
    inventory::{InventoryReport, ObservedDevice, UsbInterface},
    kernel::{KResult, Role},
    UsbDeviceIdentity,
};
use std::{
    net::{TcpListener, TcpStream},
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
};
fn device(serial: &str) -> ObservedDevice {
    ObservedDevice {
        identity: UsbDeviceIdentity {
            vid: 0x28de,
            pid: 0x2300,
            serial: serial.into(),
            busid: "1-2".into(),
            sysfs_path: "/sys/devices/mock/usb1/1-2".into(),
            topology: "bus1/port2".into(),
        },
        product: "synthetic Tracker".into(),
        interface_count: 3,
        interfaces: (0..3)
            .map(|number| UsbInterface {
                name: format!("1-2:1.{number}"),
                number,
                driver: Some("usbhid".into()),
            })
            .collect(),
        device_driver: Some("usb".into()),
        via_vhci: false,
    }
}
fn routes() -> (TcpStream, TcpStream, Request, Route) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let pi = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (host, _) = listener.accept().unwrap();
    let host_route = fd_channel::route_from_fd(host.as_raw_fd()).unwrap();
    let pi_route = fd_channel::route_from_fd(pi.as_raw_fd()).unwrap();
    let request = Request {
        stamp: Stamp {
            instance: 1,
            session: 2,
            host_id: 3,
            generation: 4,
        },
        host_route,
        timing: Timing::experimental(17, 22, 33),
    };
    (host, pi, request, pi_route)
}
#[derive(Clone, Default)]
struct Memory {
    data: Arc<Mutex<Option<Lease>>>,
    trace: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
impl Journal for Memory {
    fn load(&self) -> KResult<Option<Lease>> {
        Ok(self.data.lock().unwrap().clone())
    }
    fn store(&mut self, lease: Option<&Lease>) -> KResult<()> {
        self.trace
            .lock()
            .unwrap()
            .push(format!("save:{:?}", lease.map(|l| l.stage)));
        if self.fail {
            return Err("disk failure".into());
        }
        *self.data.lock().unwrap() = lease.cloned();
        Ok(())
    }
}
struct Fake {
    report: InventoryReport,
    token: &'static str,
    trace: Arc<Mutex<Vec<String>>>,
    fail: Option<&'static str>,
}
impl Fake {
    fn call(&self, name: &str) -> KResult<()> {
        self.trace.lock().unwrap().push(name.into());
        if self.fail == Some(name) {
            Err(format!("{name} failed"))
        } else {
            Ok(())
        }
    }
}
impl Ops for Fake {
    fn inventory(&mut self) -> KResult<InventoryReport> {
        Ok(self.report.clone())
    }
    fn token(&mut self) -> KResult<String> {
        Ok(self.token.repeat(32))
    }
    fn boot_id(&self) -> KResult<String> {
        Ok("boot-test".into())
    }
    fn prerequisites(&mut self, _: &Authorization) -> KResult<()> {
        self.call("prerequisites")
    }
    fn module(&mut self, _: Role) -> KResult<()> {
        self.call("module")
    }
    fn export_available(&mut self, _: &Plan) -> KResult<()> {
        self.call("available")
    }
    fn firewall(&mut self, _: &Plan) -> KResult<Firewall> {
        self.call("firewall")?;
        Ok(Firewall {
            handle: 19,
            snapshot: serde_json::json!({"owned":true}),
        })
    }
    fn daemon(&mut self, _: &Plan) -> KResult<Daemon> {
        self.call("daemon")?;
        Ok(Daemon {
            pid: 123,
            start_ticks: 456,
        })
    }
    fn probe(&mut self, _: std::net::SocketAddr) -> KResult<()> {
        self.call("probe")
    }
    fn stop_daemon(&mut self, _: &Plan, _: &Daemon) -> KResult<()> {
        self.call("stop")
    }
    fn remove_firewall(&mut self, _: &Plan, _: &Firewall) -> KResult<()> {
        self.call("remove_firewall")
    }
}
fn engine(role: Role) -> (Engine<Fake, Memory>, Memory) {
    let journal = Memory::default();
    let fake = Fake {
        report: InventoryReport {
            devices: vec![device("SYNTHETIC-ONE")],
            issues: vec![],
        },
        token: if role == Role::Export { "1" } else { "2" },
        trace: journal.trace.clone(),
        fail: None,
    };
    (
        Engine::new(
            Authorization {
                role,
                client_uid: 1000,
                client_gid: 1000,
                allow_loopback: true,
                allow_package_install: false,
            },
            fake,
            journal.clone(),
        )
        .unwrap(),
        journal,
    )
}
#[test]
fn loopback_route_autoselection_policy_timing_both_prepared_commit_and_cleanup() {
    let (host, pi, request, pi_route) = routes();
    assert!(request.host_route.reversed_ips(&pi_route));
    assert_eq!(
        bootstrap_rpc::peer_route(host.as_raw_fd()).unwrap(),
        request.host_route
    );
    assert_eq!(bootstrap_rpc::peer_route(pi.as_raw_fd()).unwrap(), pi_route);
    let (mut exporter, export_j) = engine(Role::Export);
    let (mut importer, _) = engine(Role::Import);
    let mut coordinator = Coordinator::new(request.clone()).unwrap();
    assert!(coordinator.commit().is_err());
    let offer = exporter
        .prepare(request.clone(), pi_route, None, None)
        .unwrap();
    let trace_before = export_j.trace.lock().unwrap().len();
    assert_eq!(
        exporter
            .prepare(request.clone(), offer.route.clone(), None, None)
            .unwrap(),
        offer
    );
    assert_eq!(export_j.trace.lock().unwrap().len(), trace_before);
    assert_eq!(offer.policy.device.serial, "SYNTHETIC-ONE");
    assert_eq!(offer.policy.remote.ip(), request.host_route.peer.ip());
    coordinator.offer(offer.clone()).unwrap();
    assert!(coordinator.commit().is_err());
    let prepared = importer
        .prepare(
            request.clone(),
            request.host_route.clone(),
            Some(offer.clone()),
            None,
        )
        .unwrap();
    assert_eq!(prepared.policy.device, offer.policy.device);
    assert_eq!(prepared.request.timing, offer.request.timing);
    coordinator.prepared(prepared.clone()).unwrap();
    let commit = coordinator.commit().unwrap();
    assert_eq!(exporter.commit(&commit).unwrap(), offer);
    assert_eq!(importer.commit(&commit).unwrap(), prepared);
    exporter.commit(&commit).unwrap();
    let mut changed = commit.clone();
    changed.import_lease = "3".repeat(32);
    assert!(exporter.commit(&changed).is_err());
    let before = export_j.trace.lock().unwrap().len();
    assert!(exporter.release(&request.stamp, true, false).is_err());
    assert_eq!(export_j.trace.lock().unwrap().len(), before);
    assert!(importer.release(&request.stamp, false, true).is_err());
    importer.release(&request.stamp, true, true).unwrap();
    exporter.release(&request.stamp, true, true).unwrap();
    assert!(exporter.lease.is_none());
    assert!(exporter
        .prepare(request.clone(), offer.route.clone(), None, None)
        .is_err());
    assert!(export_j.data.lock().unwrap().is_none());
    let trace = export_j.trace.lock().unwrap();
    assert!(
        trace
            .iter()
            .position(|s| s == "save:Some(FirewallPending)")
            .unwrap()
            < trace.iter().position(|s| s == "firewall").unwrap()
    );
    assert!(
        trace
            .iter()
            .position(|s| s == "save:Some(DaemonPending)")
            .unwrap()
            < trace.iter().position(|s| s == "daemon").unwrap()
    );
    assert!(
        trace.iter().position(|s| s == "stop").unwrap()
            < trace.iter().position(|s| s == "remove_firewall").unwrap()
    );
    assert!(!trace
        .iter()
        .any(|s| matches!(s.as_str(), "bind" | "attach" | "detach" | "unbind")));
}
#[test]
fn selector_refuses_hmd_vhci_ambiguous_duplicate_and_stale_inventory() {
    let mut hmd = device("HMD");
    hmd.identity.vid = 0x2c30;
    hmd.identity.pid = 0x1050;
    let mut imported = device("IMPORTED");
    imported.via_vhci = true;
    let c = candidates(InventoryReport {
        devices: vec![hmd, imported],
        issues: vec![],
    })
    .unwrap();
    assert!(c.is_empty());
    assert!(select(&c, None).is_err());
    let c = candidates(InventoryReport {
        devices: vec![device("A"), device("B")],
        issues: vec![],
    })
    .unwrap();
    assert!(select(&c, None).is_err());
    assert_eq!(select(&c, Some("B")).unwrap().device.serial, "B");
    assert!(select(&c, Some("C")).is_err());
    assert!(candidates(InventoryReport {
        devices: vec![device("A"), device("A")],
        issues: vec![]
    })
    .is_err());
    assert!(candidates(InventoryReport {
        devices: vec![],
        issues: vec!["unplugged".into()]
    })
    .is_err());
    let (_, _, request, pi_route) = routes();
    let (mut e, _) = engine(Role::Export);
    e.ops.report.devices = vec![device("A"), device("B")];
    assert!(e.prepare(request, pi_route, None, None).is_err());
    assert!(e.lease.is_none());
    assert!(e.ops.trace.lock().unwrap().is_empty());
}
#[test]
fn failures_retain_pending_journal_and_never_guess_resource_cleanup() {
    for failure in ["prerequisites", "module", "available", "firewall", "daemon"] {
        let (_, _, request, route) = routes();
        let (mut e, j) = engine(Role::Export);
        e.ops.fail = Some(failure);
        assert!(e
            .prepare(request.clone(), route.clone(), None, None)
            .is_err());
        assert!(e.lease.is_some());
        assert!(j.data.lock().unwrap().is_some());
        assert!(e.prepare(request.clone(), route, None, None).is_err());
        if matches!(failure, "firewall" | "daemon") {
            assert!(e.release(&request.stamp, true, true).is_err());
            assert!(!j.trace.lock().unwrap().contains(&"stop".into()));
        } else {
            // A prerequisite or occupied-port failure precedes environment mutation.
            assert!(!e.ops.trace.lock().unwrap().contains(&"firewall".into()));
            e.release(&request.stamp, true, true).unwrap();
            assert!(e.lease.is_none());
            assert!(j.data.lock().unwrap().is_none());
        }
    }
}
#[test]
fn recovered_and_wrong_session_plans_cannot_be_adopted() {
    let (_, _, request, route) = routes();
    let (mut e, j) = engine(Role::Export);
    let offer = e.prepare(request.clone(), route, None, None).unwrap();
    let commit = Commit {
        stamp: request.stamp.clone(),
        export_lease: offer.lease.clone(),
        import_lease: "2".repeat(32),
    };
    let (mut new, _) = engine(Role::Export);
    new = Engine::new(e.authorization.clone(), new.ops, j).unwrap();
    assert!(new.commit(&commit).is_err());
    assert!(new.release(&request.stamp, true, true).is_err());
    let mut stale = commit.clone();
    stale.stamp.generation += 1;
    assert!(e.commit(&stale).is_err());
    let mut coordinator = Coordinator::new(request.clone()).unwrap();
    assert!(coordinator.abort_for(&stale.stamp).is_err());
    coordinator.offer(offer).unwrap();
    coordinator.abort_for(&request.stamp).unwrap();
    assert!(coordinator.commit().is_err());
}
#[test]
fn route_offer_tampering_and_loopback_denial_precede_environment_mutation() {
    let (_, _, mut request, route) = routes();
    let (mut e, j) = engine(Role::Export);
    e.authorization.allow_loopback = false;
    assert!(e
        .prepare(request.clone(), route.clone(), None, None)
        .is_err());
    assert!(j.trace.lock().unwrap().is_empty());
    e.authorization.allow_loopback = true;
    request
        .host_route
        .peer
        .set_port(request.host_route.peer.port() + 1);
    assert!(e.prepare(request, route, None, None).is_err());
    assert!(j.trace.lock().unwrap().is_empty());
}
#[test]
fn disk_failure_precedes_commands_and_durable_journal_blocks_crash_temp() {
    use std::os::unix::fs::PermissionsExt;
    let (_, _, request, route) = routes();
    let (e, _) = engine(Role::Export);
    let j = Memory {
        trace: e.ops.trace.clone(),
        fail: true,
        ..Memory::default()
    };
    let mut e = Engine::new(e.authorization, e.ops, j.clone()).unwrap();
    assert!(e.prepare(request, route, None, None).is_err());
    assert_eq!(j.trace.lock().unwrap().as_slice(), ["save:Some(Planned)"]);
    let dir = std::env::temp_dir().join(format!(
        "llb-env-journal-{}",
        light_bridge_usbip::environment::random_token().unwrap()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("export-environment.json");
    let mut file = FileEnvironmentJournal { path: path.clone() };
    assert!(file.load().unwrap().is_none());
    file.store(e.lease.as_ref()).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(file.load().unwrap().is_some());
    let pending = path.with_extension("json.new");
    std::fs::write(&pending, b"pending").unwrap();
    assert!(file.load().is_err());
    assert!(file.store(e.lease.as_ref()).is_err());
    std::fs::remove_file(pending).unwrap();
    file.store(None).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn nft_script_is_scoped_and_contains_no_global_flush() {
    let (_, _, request, route) = routes();
    let (mut e, _) = engine(Role::Export);
    let p = e.prepare(request, route, None, None).unwrap();
    let script = firewall_script(&p).unwrap();
    assert!(script.contains("tcp dport 3240 drop"));
    assert!(script.contains("127.0.0.1"));
    assert!(!script.contains("flush"));
    assert!(!script.contains("include"));
    let mut wrong = p;
    wrong.lease = "$(id)".into();
    assert!(firewall_script(&wrong).is_err());
}
#[test]
fn private_lan_package_plan_and_foreign_exports_are_explicit_limits() {
    use light_bridge_usbip::environment::arch_package_plan;
    let mut installed = std::collections::BTreeMap::new();
    assert!(arch_package_plan("usbip 7.2.9-1\nnftables 1.1.5-1\n", &installed).is_ok());
    installed.insert("usbip".into(), "7.2.8-1".into());
    assert!(arch_package_plan("usbip 7.2.9-1\n", &installed).is_err());
    assert!(arch_package_plan("linux 7.2.9-1\n", &std::collections::BTreeMap::new()).is_err());
    let mut foreign = device("FOREIGN");
    foreign.device_driver = Some("usbip-host".into());
    assert!(candidates(InventoryReport {
        devices: vec![device("LOCAL"), foreign],
        issues: vec![]
    })
    .unwrap_err()
    .contains("existing USB/IP"));
    let mut route = Route {
        local: "192.168.1.2:1234".parse().unwrap(),
        peer: "192.168.1.3:5678".parse().unwrap(),
        netns_cookie: 1,
    };
    route.validate(false).unwrap();
    route.peer = "8.8.8.8:5678".parse().unwrap();
    assert!(route.validate(false).is_err());
    let route = Route {
        local: "[fd00::1]:1234".parse().unwrap(),
        peer: "[fd00::2]:5678".parse().unwrap(),
        netns_cookie: 1,
    };
    route.validate(false).unwrap();
}
#[test]
fn failed_owned_cleanup_and_changed_tracker_keep_environment_ownership() {
    let (_, _, request, route) = routes();
    let (mut e, j) = engine(Role::Export);
    e.prepare(request.clone(), route.clone(), None, None)
        .unwrap();
    e.ops.fail = Some("stop");
    assert!(e.release(&request.stamp, true, true).is_err());
    assert!(j.data.lock().unwrap().is_some());
    assert_eq!(e.lease.as_ref().unwrap().stage, Stage::Releasing);
    assert!(!j.trace.lock().unwrap().contains(&"remove_firewall".into()));
    assert!(e.release(&request.stamp, true, true).is_err());
    let (mut e, _) = engine(Role::Export);
    let p = e.prepare(request.clone(), route, None, None).unwrap();
    e.ops.report.devices[0].identity.busid = "1-3".into();
    let c = Commit {
        stamp: request.stamp,
        export_lease: p.lease,
        import_lease: "2".repeat(32),
    };
    assert!(e.commit(&c).is_err());
    assert_eq!(e.lease.as_ref().unwrap().stage, Stage::Prepared);
}
