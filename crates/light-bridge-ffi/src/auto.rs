//! Automatic preparation ABI; all I/O is on an explicitly created cancellable worker.
use crate::*;
use light_bridge_usbip::{
    auto_worker::Worker,
    bootstrap::{self, Coordinator, Packet, Plan, Request, Stamp, Timing},
    bootstrap_rpc::{self, Action, Envelope, Reply, Response},
    environment, fd_channel,
    kernel::Role,
};
use serde_json::Value;
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::net::UnixStream,
    sync::atomic::AtomicBool,
};
pub struct AutoHandle {
    worker: Worker,
    role: Role,
}
pub struct BootstrapHandle {
    coordinator: Coordinator,
}
/// Read-only launcher status. Only Missing permits offering temporary administrator authorization.
#[no_mangle]
pub unsafe extern "C" fn llb_auto_helper_status(
    helper_role: u32,
    bytes: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(bytes, capacity, written)?;
        write(
            bytes,
            capacity,
            written,
            &bootstrap_rpc::availability(role(helper_role)?),
        )
    })
}
/// Mark an unlinked handle as auto/kernel-only before negotiation; generic TCP cannot fall back to Mock.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_require(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if !matches!(h.library, Library::Unlinked) || h.listener.is_some() || h.kernel.is_some() {
            return Err(Error::InvalidState);
        }
        h.automatic_required = true;
        Ok(())
    })
}
#[repr(C)]
pub struct BootstrapInfo {
    pub size: u32,
    pub abi_version: u32,
    pub role: u32,
    pub reserved: u32,
    pub instance: u64,
    pub session: u64,
    pub host_id: u64,
    pub generation: u64,
    pub timing: TimingConfig,
    pub host_bind: Endpoint,
    pub serial: [u8; 129],
}
fn argument(e: impl std::fmt::Display) -> Error {
    eprintln!("LLFA bootstrap: {e}");
    Error::InvalidArgument
}
unsafe fn input<'a>(bytes: *const u8, n: u32) -> Result<&'a [u8]> {
    if bytes.is_null() || n == 0 || n as usize > bootstrap::MAX_PACKET {
        return Err(Error::InvalidArgument);
    }
    Ok(std::slice::from_raw_parts(bytes, n as usize))
}
unsafe fn output_ok(bytes: *mut u8, capacity: u32, written: *mut u32) -> Result<()> {
    if bytes.is_null() || written.is_null() || (capacity as usize) < bootstrap::MAX_PACKET {
        return Err(Error::InvalidArgument);
    }
    *written = 0;
    Ok(())
}
unsafe fn write(bytes: *mut u8, capacity: u32, written: *mut u32, value: &Value) -> Result<()> {
    output_ok(bytes, capacity, written)?;
    let encoded = serde_json::to_vec(value).map_err(argument)?;
    if encoded.len() > bootstrap::MAX_PACKET {
        return Err(Error::Backpressure);
    }
    std::ptr::copy_nonoverlapping(encoded.as_ptr(), bytes, encoded.len());
    *written = encoded.len() as u32;
    Ok(())
}
fn encode_packet(packet: Packet) -> Result<Value> {
    serde_json::from_slice(&light_bridge_protocol::bootstrap::encode(packet).map_err(argument)?)
        .map_err(argument)
}
fn decode_packet(bytes: &[u8]) -> Result<Packet> {
    light_bridge_protocol::bootstrap::decode(bytes).map_err(argument)
}
fn role(n: u32) -> Result<Role> {
    match n {
        LLB_HMD => Ok(Role::Export),
        LLB_HOST => Ok(Role::Import),
        _ => Err(Error::InvalidArgument),
    }
}
fn parse_response(bytes: &[u8]) -> Result<Response> {
    let response: Response = serde_json::from_slice(bytes).map_err(argument)?;
    response
        .validate(response.authorization.role)
        .map_err(argument)?;
    Ok(response)
}
fn committed(response: Response) -> Result<Plan> {
    let r = response.result.map_err(argument)?;
    let Reply::Committed(plan) = r else {
        return Err(Error::InvalidState);
    };
    plan.validate().map_err(argument)?;
    if plan.policy.role != response.authorization.role
        || plan.policy.client_uid != response.authorization.client_uid
        || plan.policy.client_gid != response.authorization.client_gid
    {
        return Err(Error::Ownership);
    }
    Ok(plan)
}
/// Label an Inventory reply with the active Prepare stamp for Client selection UI.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_candidates_packet(
    prepare: *const u8,
    prepare_len: u32,
    response: *const u8,
    response_len: u32,
    out: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(out, capacity, written)?;
        let Packet::Prepare(request) = decode_packet(input(prepare, prepare_len)?)? else {
            return Err(Error::InvalidState);
        };
        let response = parse_response(input(response, response_len)?)?;
        if response.authorization.role != Role::Export {
            return Err(Error::InvalidArgument);
        }
        let Reply::Candidates(candidates) = response.result.map_err(argument)? else {
            return Err(Error::InvalidState);
        };
        write(
            out,
            capacity,
            written,
            &encode_packet(Packet::Candidates {
                stamp: request.stamp,
                candidates,
            })?,
        )
    })
}

/// Create a host request from a connected WiVRn TCP FD. No IP or timing IDs are required.
/// Optional quality template uses the TimingConfig size/ABI prefix; IDs are generated.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_new_request(
    fd: i32,
    quality: *const TimingConfig,
    bytes: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(bytes, capacity, written)?;
        let host_route = fd_channel::route_from_fd(fd).map_err(argument)?;
        let stamp = Stamp {
            instance: environment::random_u64().map_err(argument)?,
            session: environment::random_u64().map_err(argument)?,
            host_id: environment::random_u64().map_err(argument)?,
            generation: environment::random_u64().map_err(argument)?,
        };
        let mut c = TimingConfig::experimental(
            environment::random_u64().map_err(argument)?,
            environment::random_u64().map_err(argument)?,
            environment::random_u64().map_err(argument)?,
        );
        if !quality.is_null() {
            if quality.cast::<u32>().read() != std::mem::size_of::<TimingConfig>() as u32
                || quality.cast::<u32>().add(1).read() != VERSION
            {
                return Err(Error::InvalidArgument);
            }
            let (d, s, l) = (c.tracked_device_id, c.source_clock_id, c.local_clock_id);
            c = *quality;
            c.tracked_device_id = d;
            c.source_clock_id = s;
            c.local_clock_id = l;
        }
        c.validate()?;
        let request = Request {
            stamp,
            host_route,
            timing: Timing::from_core(c),
        };
        write(
            bytes,
            capacity,
            written,
            &encode_packet(Packet::Prepare(request))?,
        )
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_create(
    bytes: *const u8,
    n: u32,
    out: *mut *mut BootstrapHandle,
) -> i32 {
    guard(|| {
        if out.is_null() || !(*out).is_null() {
            return Err(Error::InvalidArgument);
        }
        let Packet::Prepare(r) = decode_packet(input(bytes, n)?)? else {
            return Err(Error::InvalidState);
        };
        *out = Box::into_raw(Box::new(BootstrapHandle {
            coordinator: Coordinator::new(r).map_err(argument)?,
        }));
        Ok(())
    })
}
/// Apply an Offer or Prepared packet. State is bounded; changed or stale offers are rejected.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_receive(
    handle: *mut BootstrapHandle,
    bytes: *const u8,
    n: u32,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match decode_packet(input(bytes, n)?)? {
            Packet::Offer(p) => h.coordinator.offer(p).map_err(argument)?,
            Packet::Prepared(p) => h.coordinator.prepared(p).map_err(argument)?,
            Packet::Abort { stamp, .. } => h.coordinator.abort_for(&stamp).map_err(argument)?,
            _ => return Err(Error::InvalidState),
        }
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_commit(
    handle: *mut BootstrapHandle,
    bytes: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(bytes, capacity, written)?;
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        let c = h.coordinator.commit().map_err(|e| {
            eprintln!("LLFA bootstrap: {e}");
            Error::InvalidState
        })?;
        write(bytes, capacity, written, &encode_packet(Packet::Commit(c))?)
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_destroy(slot: *mut *mut BootstrapHandle) -> i32 {
    guard(|| {
        let slot = slot.as_mut().ok_or(Error::InvalidArgument)?;
        let raw = std::mem::replace(slot, std::ptr::null_mut());
        if !raw.is_null() {
            drop(Box::from_raw(raw));
        }
        Ok(())
    })
}
/// Convert bootstrap packet to fixed helper command: 1 export, 2 import, 3 commit, 4 release.
/// Release requires runtime_stopped=1 after normal USB cleanup. serial may be NULL/0 for auto selection.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_helper_command(
    bytes: *const u8,
    n: u32,
    operation: u32,
    serial: *const u8,
    serial_len: u32,
    runtime_stopped: u32,
    helper_role: u32,
    out: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(out, capacity, written)?;
        let p = decode_packet(input(bytes, n)?)?;
        let role = role(helper_role)?;
        let selected = if serial_len == 0 {
            None
        } else {
            if serial.is_null() || serial_len > 128 {
                return Err(Error::InvalidArgument);
            }
            Some(
                std::str::from_utf8(std::slice::from_raw_parts(serial, serial_len as usize))
                    .map_err(argument)?
                    .to_string(),
            )
        };
        let action = match (operation, p) {
            (1, Packet::Prepare(request)) if role == Role::Export => Action::PrepareExport {
                request,
                serial: selected,
            },
            (2, Packet::Offer(offer)) if role == Role::Import => Action::PrepareImport {
                offer: Box::new(offer),
            },
            (3, Packet::Commit(c)) => Action::Commit(c),
            (4, Packet::Commit(c)) if runtime_stopped == 1 => Action::Release {
                stamp: c.stamp,
                runtime_released: true,
            },
            (4, Packet::Prepare(r)) if runtime_stopped == 1 => Action::Release {
                stamp: r.stamp,
                runtime_released: true,
            },
            (4, Packet::Offer(p) | Packet::Prepared(p)) if runtime_stopped == 1 => {
                Action::Release {
                    stamp: p.request.stamp,
                    runtime_released: true,
                }
            }
            _ => return Err(Error::InvalidState),
        };
        write(
            out,
            capacity,
            written,
            &serde_json::to_value(Envelope::new(role, action)).map_err(argument)?,
        )
    })
}
/// Convert a successful helper Prepared reply to a WiVRn/UDS Offer or Prepared packet.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_response_packet(
    bytes: *const u8,
    n: u32,
    out: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(out, capacity, written)?;
        let response = parse_response(input(bytes, n)?)?;
        let Reply::Prepared(plan) = response.result.map_err(argument)? else {
            return Err(Error::InvalidState);
        };
        let p = if plan.policy.role == Role::Export {
            Packet::Offer(plan)
        } else {
            Packet::Prepared(plan)
        };
        write(out, capacity, written, &encode_packet(p)?)
    })
}
/// Committed reply metadata: IDs, timing, Host bind address and selected serial.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_get_info(
    bytes: *const u8,
    n: u32,
    out: *mut BootstrapInfo,
) -> i32 {
    guard(|| {
        if out.is_null()
            || out.cast::<u32>().read() != std::mem::size_of::<BootstrapInfo>() as u32
            || out.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        let p = committed(parse_response(input(bytes, n)?)?)?;
        let host = if p.policy.role == Role::Import {
            p.route.local
        } else {
            p.request.host_route.local
        };
        let mut address = [0u8; 16];
        let (family, scope_id) = match host {
            std::net::SocketAddr::V4(v) => {
                address[..4].copy_from_slice(&v.ip().octets());
                (4, 0)
            }
            std::net::SocketAddr::V6(v) => {
                address.copy_from_slice(&v.ip().octets());
                (6, v.scope_id())
            }
        };
        let mut serial = [0u8; 129];
        serial[..p.policy.device.serial.len()].copy_from_slice(p.policy.device.serial.as_bytes());
        *out = BootstrapInfo {
            size: std::mem::size_of::<BootstrapInfo>() as u32,
            abi_version: VERSION,
            role: if p.policy.role == Role::Export {
                LLB_HMD
            } else {
                LLB_HOST
            },
            reserved: 0,
            instance: p.request.stamp.instance,
            session: p.request.stamp.session,
            host_id: p.request.stamp.host_id,
            generation: p.request.stamp.generation,
            timing: p.request.timing.core(),
            host_bind: Endpoint {
                size: std::mem::size_of::<Endpoint>() as u32,
                abi_version: VERSION,
                family,
                port: 0,
                address,
                scope_id,
                reserved: 0,
                host_id: p.request.stamp.host_id,
                correlation_id: p.request.stamp.session,
            },
            serial,
        };
        Ok(())
    })
}
/// Configure an unlinked runtime handle from a committed root helper reply. No Mock fallback.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_apply(handle: *mut Handle, bytes: *const u8, n: u32) -> i32 {
    guard(|| {
        let p = committed(parse_response(input(bytes, n)?)?)?;
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if !matches!(h.library, Library::Unlinked)
            || h.listener.is_some()
            || h.kernel.is_some()
            || h.timing.is_some()
        {
            return Err(Error::InvalidState);
        }
        if role(h.role)? != p.policy.role {
            return Err(Error::InvalidArgument);
        }
        h.timing = Some(p.request.timing.core());
        h.automatic_required = true;
        h.kernel = Some((
            format!(
                "/run/llfa-link-light-bridge/{}.sock",
                if p.policy.role == Role::Export {
                    "export"
                } else {
                    "import"
                }
            ),
            p.policy,
            p.request.stamp.instance,
        ));
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_auto_worker_create(
    helper_role: u32,
    out: *mut *mut AutoHandle,
) -> i32 {
    guard(|| {
        if out.is_null() || !(*out).is_null() {
            return Err(Error::InvalidArgument);
        }
        let role = role(helper_role)?;
        let worker = Worker::new(bootstrap_rpc::socket(role)).map_err(argument)?;
        *out = Box::into_raw(Box::new(AutoHandle { worker, role }));
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_auto_worker_submit(
    handle: *mut AutoHandle,
    operation_id: u64,
    bytes: *const u8,
    n: u32,
    route_fd: i32,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        let request: Envelope = serde_json::from_slice(input(bytes, n)?).map_err(argument)?;
        request.validate().map_err(argument)?;
        if request.role != h.role
            || operation_id == 0
            || matches!(
                request.action,
                Action::PrepareExport { .. } | Action::PrepareImport { .. }
            ) != (route_fd >= 0)
        {
            return Err(Error::InvalidArgument);
        }
        h.worker
            .submit(
                operation_id,
                request,
                if route_fd < 0 { None } else { Some(route_fd) },
            )
            .map_err(|e| {
                eprintln!("LLFA auto worker: {e}");
                if e.contains("queue full") {
                    Error::Backpressure
                } else {
                    Error::InvalidArgument
                }
            })
    })
}
/// Return 6 when no completion is available. Outputs require 8192 bytes to avoid lost replies.
#[no_mangle]
pub unsafe extern "C" fn llb_auto_worker_poll(
    handle: *mut AutoHandle,
    id: *mut u64,
    bytes: *mut u8,
    capacity: u32,
    written: *mut u32,
) -> i32 {
    guard(|| {
        output_ok(bytes, capacity, written)?;
        if id.is_null() {
            return Err(Error::InvalidArgument);
        }
        *id = 0;
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        let Some((operation, result)) = h.worker.poll().map_err(argument)? else {
            return Err(Error::Backpressure);
        };
        *id = operation;
        match result {
            Ok(response) => write(
                bytes,
                capacity,
                written,
                &serde_json::to_value(response).map_err(argument)?,
            ),
            Err(e) => {
                write(
                    bytes,
                    capacity,
                    written,
                    &serde_json::json!({"transport_error":e}),
                )?;
                Err(Error::Protocol)
            }
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_auto_worker_destroy(slot: *mut *mut AutoHandle) -> i32 {
    guard(|| {
        let slot = slot.as_mut().ok_or(Error::InvalidArgument)?;
        let raw = std::mem::replace(slot, std::ptr::null_mut());
        if !raw.is_null() {
            drop(Box::from_raw(raw));
        }
        Ok(())
    })
}
/// Duplex local bootstrap I/O, same UID, one optional TCP fd; use an adapter control worker.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_ipc_send(
    fd: i32,
    bytes: *const u8,
    n: u32,
    route_fd: i32,
    timeout_ms: u32,
) -> i32 {
    guard(|| {
        let p = decode_packet(input(bytes, n)?)?;
        let raw = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0);
        if raw < 0 || timeout_ms == 0 {
            if raw >= 0 {
                libc::close(raw);
            }
            return Err(Error::InvalidArgument);
        }
        let dup = OwnedFd::from_raw_fd(raw);
        let mut stream = UnixStream::from(dup);
        if light_bridge_usbip::helper::peer_uid(&stream).map_err(argument)? != libc::geteuid() {
            return Err(Error::Ownership);
        }
        fd_channel::send(
            &mut stream,
            &encode_packet(p)?,
            if route_fd < 0 { None } else { Some(route_fd) },
            std::time::Instant::now() + std::time::Duration::from_millis(u64::from(timeout_ms)),
            &AtomicBool::new(false),
        )
        .map_err(argument)
    })
}
/// On any framing failure close the original stream. Returned TCP descriptor is owned by caller.
#[no_mangle]
pub unsafe extern "C" fn llb_bootstrap_ipc_receive(
    fd: i32,
    route_fd: *mut i32,
    bytes: *mut u8,
    capacity: u32,
    written: *mut u32,
    timeout_ms: u32,
) -> i32 {
    guard(|| {
        output_ok(bytes, capacity, written)?;
        if route_fd.is_null() || timeout_ms == 0 {
            return Err(Error::InvalidArgument);
        }
        *route_fd = -1;
        let raw = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0);
        if raw < 0 {
            return Err(Error::InvalidArgument);
        }
        let mut stream = UnixStream::from(OwnedFd::from_raw_fd(raw));
        if light_bridge_usbip::helper::peer_uid(&stream).map_err(argument)? != libc::geteuid() {
            return Err(Error::Ownership);
        }
        let (wire, route): (light_bridge_protocol::bootstrap::Wire, _) = fd_channel::receive(
            &mut stream,
            std::time::Instant::now() + std::time::Duration::from_millis(u64::from(timeout_ms)),
            &AtomicBool::new(false),
        )
        .map_err(argument)?;
        let packet = wire.validate().map_err(argument)?;
        if let Some(fd) = &route {
            if !matches!(packet, Packet::Prepare(_)) {
                return Err(Error::InvalidArgument);
            }
            fd_channel::route_from_fd(fd.as_raw_fd()).map_err(argument)?;
        }
        write(bytes, capacity, written, &encode_packet(packet)?)?;
        if let Some(fd) = route {
            use std::os::fd::IntoRawFd;
            *route_fd = fd.into_raw_fd();
        }
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use light_bridge_usbip::{
        bootstrap::{Authorization, PROFILE},
        kernel::Policy,
        UsbDeviceIdentity,
    };
    #[test]
    fn committed_auto_metadata_configures_both_roles_without_manual_policy_or_ids() {
        unsafe {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let pi = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (host, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 8192];
            let mut n = 0;
            assert_eq!(
                llb_bootstrap_new_request(
                    host.as_raw_fd(),
                    std::ptr::null(),
                    bytes.as_mut_ptr(),
                    8192,
                    &mut n
                ),
                0
            );
            let prepare = bytes[..n as usize].to_vec();
            let Packet::Prepare(request) = decode_packet(&prepare).unwrap() else {
                panic!()
            };
            let route = fd_channel::route_from_fd(pi.as_raw_fd()).unwrap();
            let device = UsbDeviceIdentity {
                vid: PROFILE.0,
                pid: PROFILE.1,
                serial: "SYNTHETIC-ONE".into(),
                busid: "1-2".into(),
                sysfs_path: "/sys/devices/mock/usb1/1-2".into(),
                topology: "bus1/port2".into(),
            };
            let export = Plan {
                request: request.clone(),
                route: route.clone(),
                policy: Policy {
                    role: Role::Export,
                    client_uid: 1000,
                    client_gid: 1000,
                    device: device.clone(),
                    remote: route.usb_endpoint(Role::Export, 3240),
                },
                lease: "1".repeat(32),
                peer_lease: None,
            };
            let import = Plan {
                request: request.clone(),
                route: request.host_route.clone(),
                policy: Policy {
                    role: Role::Import,
                    client_uid: 1000,
                    client_gid: 1000,
                    device,
                    remote: request.host_route.usb_endpoint(Role::Import, 3240),
                },
                lease: "2".repeat(32),
                peer_lease: Some(export.lease.clone()),
            };
            let mut coordinator = std::ptr::null_mut();
            assert_eq!(
                llb_bootstrap_create(prepare.as_ptr(), prepare.len() as u32, &mut coordinator),
                0
            );
            for plan in [&export, &import] {
                let response = Response::new(
                    Authorization {
                        role: plan.policy.role,
                        client_uid: 1000,
                        client_gid: 1000,
                        allow_loopback: true,
                        allow_package_install: false,
                    },
                    true,
                    Ok(Reply::Prepared(plan.clone())),
                );
                let data = serde_json::to_vec(&response).unwrap();
                assert_eq!(
                    llb_bootstrap_response_packet(
                        data.as_ptr(),
                        data.len() as u32,
                        bytes.as_mut_ptr(),
                        8192,
                        &mut n
                    ),
                    0
                );
                assert_eq!(llb_bootstrap_receive(coordinator, bytes.as_ptr(), n), 0);
            }
            assert_eq!(
                llb_bootstrap_commit(coordinator, bytes.as_mut_ptr(), 8192, &mut n),
                0
            );
            let Packet::Commit(commit) = decode_packet(&bytes[..n as usize]).unwrap() else {
                panic!()
            };
            assert!(commit.validates(&export));
            assert!(commit.validates(&import));
            for plan in [export, import] {
                let response = Response::new(
                    Authorization {
                        role: plan.policy.role,
                        client_uid: 1000,
                        client_gid: 1000,
                        allow_loopback: true,
                        allow_package_install: false,
                    },
                    true,
                    Ok(Reply::Committed(plan.clone())),
                );
                let data = serde_json::to_vec(&response).unwrap();
                let mut info = std::mem::MaybeUninit::<BootstrapInfo>::zeroed().assume_init();
                info.size = std::mem::size_of::<BootstrapInfo>() as u32;
                info.abi_version = 1;
                assert_eq!(
                    llb_bootstrap_get_info(data.as_ptr(), data.len() as u32, &mut info),
                    0
                );
                assert_eq!(info.session, request.stamp.session);
                assert_eq!(info.host_bind.correlation_id, info.session);
                assert_eq!(info.timing.tracked_device_id, request.timing.device);
                let mut handle = std::ptr::null_mut();
                let config = Config {
                    size: std::mem::size_of::<Config>() as u32,
                    abi_version: 1,
                    role: info.role,
                    timeout_ms: 1000,
                    callback: None,
                    userdata: std::ptr::null_mut(),
                };
                assert_eq!(llb_create(&config, &mut handle), 0);
                assert_eq!(llb_bootstrap_require(handle), 0);
                assert_eq!(
                    llb_bootstrap_apply(handle, data.as_ptr(), data.len() as u32),
                    0
                );
                assert_eq!((*handle).kernel.as_ref().unwrap().1, plan.policy);
                assert_eq!((*handle).kernel.as_ref().unwrap().2, request.stamp.instance);
                assert_eq!(
                    llb_bootstrap_apply(handle, data.as_ptr(), data.len() as u32),
                    2
                );
                assert_eq!(llb_destroy(&mut handle), 0);
            }
            assert_eq!(llb_bootstrap_destroy(&mut coordinator), 0);
        }
    }
}
