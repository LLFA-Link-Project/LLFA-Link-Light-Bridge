//! C ABI v1, bounded transports with explicit Mock or helper-mediated kernel USB/IP. Callers serialize access and must not reenter callbacks.
//! # Safety
//! All non-null pointers must be valid, aligned and point to the documented live objects.
//! Userdata outlives the handle; callbacks borrow pose for the duration of the call only.
#![allow(clippy::missing_safety_doc)]
use light_bridge_core::clock::{monotonic_ns, ClockStatus, ReferenceTiming, TimingConfig};
use light_bridge_core::*;
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::mock_pair;
use light_bridge_usbip::MockUsbIp;
use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex},
    time::Duration,
};
pub const LLB_ABI_VERSION: u32 = 1;
pub const LLB_HMD: u32 = 0;
pub const LLB_HOST: u32 = 1;

pub type Callback = Option<unsafe extern "C" fn(*mut c_void, u32, u32, *const ExternalPose)>;
#[repr(C)]
pub struct Config {
    pub size: u32,
    pub abi_version: u32,
    pub role: u32,
    pub timeout_ms: u32,
    pub callback: Callback,
    pub userdata: *mut c_void,
}
#[repr(C)]
pub struct Endpoint {
    pub size: u32,
    pub abi_version: u32,
    pub family: u32,
    pub port: u32,
    pub address: [u8; 16],
    pub scope_id: u32,
    pub reserved: u32,
    pub host_id: u64,
    pub correlation_id: u64,
}
enum Library {
    Unlinked,
    Hmd(HmdClient),
    Host(HostServer),
}
pub struct Handle {
    role: u32,
    timeout: Duration,
    callback: Callback,
    userdata: *mut c_void,
    library: Library,
    listener: Option<(std::net::TcpListener, EndpointInfo)>,
    peer_host: Option<u64>,
    timing: Option<TimingConfig>,
    #[cfg(target_os = "linux")]
    kernel: Option<(String, light_bridge_usbip::kernel::Policy, u64)>,
}
fn kernel_configured(h: &Handle) -> bool {
    #[cfg(target_os = "linux")]
    {
        h.kernel.is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = h;
        false
    }
}
fn apply_timing(library: &mut Library, config: Option<TimingConfig>) -> Result<()> {
    if let Some(c) = config {
        match library {
            Library::Hmd(v) => v.configure_timing(c)?,
            Library::Host(v) => v.configure_timing(c)?,
            _ => return Err(Error::InvalidState),
        }
    }
    Ok(())
}
fn connected_library(h: &Handle, transport: light_bridge_protocol::Transport) -> Result<Library> {
    let mut library = make_connected_library(h, transport)?;
    apply_timing(&mut library, h.timing)?;
    Ok(library)
}
fn make_connected_library(
    h: &Handle,
    transport: light_bridge_protocol::Transport,
) -> Result<Library> {
    #[cfg(target_os = "linux")]
    if let Some((socket, policy, instance)) = &h.kernel {
        let backend = Box::new(light_bridge_usbip::helper::KernelUsbIp::new(
            socket,
            policy.clone(),
            *instance,
        )?);
        return if h.role == LLB_HOST {
            HostServer::with_usb_backend(transport, backend, policy.device.clone(), h.timeout)
                .map(Library::Host)
        } else {
            HmdClient::with_usb_backend(transport, backend, policy.device.clone(), h.timeout)
                .map(Library::Hmd)
        };
    }
    Ok(if h.role == LLB_HOST {
        Library::Host(HostServer::new(
            transport,
            Arc::new(Mutex::new(MockUsbIp::remote_importer())),
            h.timeout,
        ))
    } else {
        Library::Hmd(HmdClient::new(
            transport,
            Arc::new(Mutex::new(MockUsbIp::ready())),
            h.timeout,
        ))
    })
}
/// Configure CLOCK_MONOTONIC and explicit pose policy before start. Does not enable Bridge or USB.
#[no_mangle]
pub unsafe extern "C" fn llb_configure_timing(
    handle: *mut Handle,
    config: *const TimingConfig,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if config.is_null()
            || config.cast::<u32>().read() != std::mem::size_of::<TimingConfig>() as u32
            || config.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        let c = *config;
        c.validate()?;
        monotonic_ns()?;
        if h.state() != State::Idle {
            return Err(Error::InvalidState);
        }
        if !matches!(h.library, Library::Unlinked) {
            apply_timing(&mut h.library, Some(c))?;
        }
        h.timing = Some(c);
        Ok(())
    })
}
/// Linux CLOCK_MONOTONIC nanoseconds, same domain used by live ClockSync. No wall clock.
#[no_mangle]
pub unsafe extern "C" fn llb_monotonic_time_ns(out: *mut i64) -> i32 {
    guard(|| {
        if out.is_null() {
            return Err(Error::InvalidArgument);
        }
        *out = monotonic_ns()?;
        Ok(())
    })
}
/// HMD only. Caller initializes output size/abi_version. Unsynchronized is a successful status result.
#[no_mangle]
pub unsafe extern "C" fn llb_get_clock_status(handle: *mut Handle, out: *mut ClockStatus) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if out.is_null()
            || out.cast::<u32>().read() != std::mem::size_of::<ClockStatus>() as u32
            || out.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        match &h.library {
            Library::Hmd(v) => {
                *out = v.clock_status()?;
                Ok(())
            }
            _ => Err(Error::InvalidState),
        }
    })
}
/// Timing of the last callback-delivered reference. Query AFTER poll, never reenter a callback.
/// Match session/sequence to the copied raw pose. Does not alter its original timestamp or semantics.
#[no_mangle]
pub unsafe extern "C" fn llb_get_reference_timing(
    handle: *mut Handle,
    out: *mut ReferenceTiming,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if out.is_null()
            || out.cast::<u32>().read() != std::mem::size_of::<ReferenceTiming>() as u32
            || out.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        match &h.library {
            Library::Hmd(v) => {
                *out = v.reference_timing()?;
                Ok(())
            }
            _ => Err(Error::InvalidState),
        }
    })
}
/// HMD rejection diagnostics: 0 last accepted, 1 clock/inactive, 2 pose policy, 3 malformed.
#[no_mangle]
pub unsafe extern "C" fn llb_get_pose_diagnostics(
    handle: *mut Handle,
    rejected: *mut u64,
    reason: *mut u32,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if rejected.is_null() || reason.is_null() {
            return Err(Error::InvalidArgument);
        }
        match &h.library {
            Library::Hmd(v) => {
                (*rejected, *reason) = v.pose_diagnostics();
                Ok(())
            }
            _ => Err(Error::InvalidState),
        }
    })
}
/// Configure exact runtime policy before TCP setup. No privileged action at this point.
/// UTF-8 socket and JSON policy buffers are borrowed only during this call, each <=4096 bytes.
#[no_mangle]
pub unsafe extern "C" fn llb_configure_kernel_usbip(
    handle: *mut Handle,
    socket: *const u8,
    socket_len: u32,
    policy_json: *const u8,
    policy_len: u32,
    instance: u64,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if !matches!(h.library, Library::Unlinked) || h.listener.is_some() || kernel_configured(h) {
            return Err(Error::InvalidState);
        }
        if socket.is_null()
            || policy_json.is_null()
            || socket_len == 0
            || socket_len > 107
            || policy_len == 0
            || policy_len > 4096
            || instance == 0
        {
            return Err(Error::InvalidArgument);
        }
        #[cfg(target_os = "linux")]
        {
            let socket =
                std::str::from_utf8(std::slice::from_raw_parts(socket, socket_len as usize))
                    .map_err(|_| Error::InvalidArgument)?;
            if !socket.starts_with('/') || socket.contains('\0') {
                return Err(Error::InvalidArgument);
            }
            let policy: light_bridge_usbip::kernel::Policy = serde_json::from_slice(
                std::slice::from_raw_parts(policy_json, policy_len as usize),
            )
            .map_err(|_| Error::InvalidArgument)?;
            policy.validate().map_err(|_| Error::DeviceDenied)?;
            let expected = if h.role == LLB_HOST {
                light_bridge_usbip::kernel::Role::Import
            } else {
                light_bridge_usbip::kernel::Role::Export
            };
            if policy.role != expected {
                return Err(Error::InvalidArgument);
            }
            h.kernel = Some((socket.into(), policy, instance));
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (socket, policy_json);
            Err(Error::Unsupported)
        }
    })
}
fn guard(f: impl FnOnce() -> Result<()>) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => code(e),
        Err(_) => 255,
    }
}
fn code(e: Error) -> i32 {
    match e {
        Error::InvalidArgument => 1,
        Error::InvalidState => 2,
        Error::WrongSession => 3,
        Error::Timeout => 4,
        Error::Disconnected => 5,
        Error::Backpressure => 6,
        Error::Unsupported | Error::UnsupportedExportGranularity => 7,
        Error::Protocol => 8,
        Error::Ownership => 9,
        _ => 10,
    }
}
impl Handle {
    fn state(&self) -> State {
        match &self.library {
            Library::Hmd(v) => v.state(),
            Library::Host(v) => v.state(),
            _ => State::Idle,
        }
    }
    unsafe fn notify(&mut self) {
        if let Some(cb) = self.callback {
            cb(self.userdata, 1, self.state() as u32, std::ptr::null());
            if let Library::Hmd(v) = &mut self.library {
                if let Some(p) = v.take_reference_pose() {
                    cb(self.userdata, 2, State::Active as u32, &p);
                }
            }
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn llb_create(config: *const Config, out: *mut *mut Handle) -> i32 {
    guard(|| {
        if config.is_null() || out.is_null() {
            return Err(Error::InvalidArgument);
        }
        // Inspect prefix before touching the full versioned structure.
        if config.cast::<u32>().read() != std::mem::size_of::<Config>() as u32
            || config.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        let c = &*config;
        if c.role > 1 || c.timeout_ms == 0 {
            return Err(Error::InvalidArgument);
        }
        if !(*out).is_null() {
            return Err(Error::InvalidArgument);
        }
        *out = Box::into_raw(Box::new(Handle {
            role: c.role,
            timeout: Duration::from_millis(c.timeout_ms.into()),
            callback: c.callback,
            userdata: c.userdata,
            library: Library::Unlinked,
            listener: None,
            peer_host: None,
            timing: None,
            #[cfg(target_os = "linux")]
            kernel: None,
        }));
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_mock_link(hmd: *mut Handle, host: *mut Handle) -> i32 {
    guard(|| {
        if hmd.is_null() || host.is_null() || hmd == host {
            return Err(Error::InvalidArgument);
        }
        let h = &mut *hmd;
        let s = &mut *host;
        if kernel_configured(h)
            || kernel_configured(s)
            || h.role != 0
            || s.role != 1
            || !matches!(h.library, Library::Unlinked)
            || !matches!(s.library, Library::Unlinked)
            || h.listener.is_some()
            || s.listener.is_some()
        {
            return Err(Error::InvalidState);
        }
        let (a, b) = mock_pair();
        let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
        h.library = Library::Hmd(HmdClient::new(a, usb.clone(), h.timeout));
        s.library = Library::Host(HostServer::new(b, usb, s.timeout));
        apply_timing(&mut h.library, h.timing)?;
        apply_timing(&mut s.library, s.timing)?;
        Ok(())
    })
}
unsafe fn endpoint_info(endpoint: *const Endpoint, allow_zero_port: bool) -> Result<EndpointInfo> {
    if endpoint.is_null()
        || endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
        || endpoint.cast::<u32>().add(1).read() != VERSION
    {
        return Err(Error::InvalidArgument);
    }
    let e = &*endpoint;
    if (!allow_zero_port && e.port == 0)
        || e.port > 65535
        || e.reserved != 0
        || e.host_id == 0
        || e.correlation_id == 0
    {
        return Err(Error::InvalidArgument);
    }
    let address = match e.family {
        4 if e.scope_id == 0 && e.address[4..].iter().all(|b| *b == 0) => {
            std::net::SocketAddr::from((
                std::net::Ipv4Addr::new(e.address[0], e.address[1], e.address[2], e.address[3]),
                e.port as u16,
            ))
        }
        6 => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            std::net::Ipv6Addr::from(e.address),
            e.port as u16,
            0,
            e.scope_id,
        )),
        _ => return Err(Error::InvalidArgument),
    };
    Ok(EndpointInfo {
        addresses: vec![address],
        host_id: e.host_id,
        correlation_id: e.correlation_id,
    })
}
/// Bind Host before notifying Client. Port 0 is permitted and updated in endpoint.
/// Uses explicitly configured kernel backend, otherwise Mock. Bridge TCP is not authenticated.
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_listen(handle: *mut Handle, endpoint: *mut Endpoint) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if h.role != LLB_HOST || !matches!(h.library, Library::Unlinked) || h.listener.is_some() {
            return Err(Error::InvalidState);
        }
        let mut info = endpoint_info(endpoint, true)?;
        let listener =
            std::net::TcpListener::bind(info.addresses[0]).map_err(|_| Error::Disconnected)?;
        info.addresses[0] = listener.local_addr().map_err(|_| Error::Disconnected)?;
        (*endpoint).port = info.addresses[0].port() as u32;
        h.listener = Some((listener, info));
        Ok(())
    })
}
/// Blocking deadline-bounded setup on the caller's control thread, never render thread.
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_accept(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if h.role != LLB_HOST || !matches!(h.library, Library::Unlinked) {
            return Err(Error::InvalidState);
        }
        let (listener, info) = h.listener.as_ref().ok_or(Error::InvalidState)?;
        let transport = light_bridge_protocol::network::accept(listener, info, h.timeout)?;
        let peer_host = info.host_id;
        h.library = connected_library(h, transport)?;
        h.peer_host = Some(peer_host);
        h.listener = None;
        Ok(())
    })
}
/// Connect HMD using a previously notified endpoint; start is a separate lifecycle event.
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_connect(handle: *mut Handle, endpoint: *const Endpoint) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if h.role != LLB_HMD || !matches!(h.library, Library::Unlinked) {
            return Err(Error::InvalidState);
        }
        let info = endpoint_info(endpoint, false)?;
        let transport = light_bridge_protocol::network::connect(&info, h.timeout)?;
        h.library = connected_library(h, transport)?;
        h.peer_host = Some(info.host_id);
        Ok(())
    })
}
/// Compatibility test-only API: reject a configured kernel backend.
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_listen_mock(handle: *mut Handle, endpoint: *mut Endpoint) -> i32 {
    if handle.is_null() {
        return 1;
    }
    if kernel_configured(&*handle) {
        return 2;
    }
    llb_tcp_listen(handle, endpoint)
}
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_accept_mock(handle: *mut Handle) -> i32 {
    if handle.is_null() {
        return 1;
    }
    if kernel_configured(&*handle) {
        return 2;
    }
    llb_tcp_accept(handle)
}
#[no_mangle]
pub unsafe extern "C" fn llb_tcp_connect_mock(
    handle: *mut Handle,
    endpoint: *const Endpoint,
) -> i32 {
    if handle.is_null() {
        return 1;
    }
    if kernel_configured(&*handle) {
        return 2;
    }
    llb_tcp_connect(handle, endpoint)
}
#[no_mangle]
pub unsafe extern "C" fn llb_request_start(
    handle: *mut Handle,
    session: u64,
    endpoint: *const Endpoint,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Host(v) => v.start(session)?,
            Library::Hmd(v) => {
                let info = endpoint_info(endpoint, false)?;
                if h.peer_host.is_some_and(|host| host != info.host_id) {
                    return Err(Error::WrongSession);
                }
                v.start(session, &info)?;
                h.peer_host = Some(info.host_id);
            }
            _ => return Err(Error::InvalidState),
        }
        h.notify();
        Ok(())
    })
}
#[cfg(target_os = "linux")]
unsafe fn borrowed_notification_stream(fd: i32) -> Result<std::os::unix::net::UnixStream> {
    use std::os::fd::FromRawFd;
    let duplicate = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0);
    if duplicate < 0 {
        return Err(Error::InvalidArgument);
    }
    Ok(std::os::unix::net::UnixStream::from_raw_fd(duplicate))
}
/// Send Start(kind=1) or Stop(kind=2) on an already connected, borrowed UDS fd.
/// Leaves the shared socket nonblocking. No pose/IMU payload. Linux only.
#[no_mangle]
pub unsafe extern "C" fn llb_ipc_send(
    fd: i32,
    kind: u32,
    endpoint: *const Endpoint,
    timeout_ms: u32,
) -> i32 {
    guard(|| {
        if timeout_ms == 0 {
            return Err(Error::InvalidArgument);
        }
        #[cfg(target_os = "linux")]
        {
            let mut stream = borrowed_notification_stream(fd)?;
            let notification = match kind {
                1 => {
                    light_bridge_protocol::ipc::Notification::Start(endpoint_info(endpoint, false)?)
                }
                2 => {
                    if endpoint.is_null()
                        || endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
                        || endpoint.cast::<u32>().add(1).read() != VERSION
                    {
                        return Err(Error::InvalidArgument);
                    }
                    let e = &*endpoint;
                    if e.host_id == 0 || e.correlation_id == 0 || e.reserved != 0 {
                        return Err(Error::InvalidArgument);
                    }
                    light_bridge_protocol::ipc::Notification::Stop {
                        host_id: e.host_id,
                        correlation_id: e.correlation_id,
                    }
                }
                _ => return Err(Error::InvalidArgument),
            };
            light_bridge_protocol::ipc::send(
                &mut stream,
                &notification,
                Duration::from_millis(timeout_ms.into()),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (fd, kind, endpoint);
            Err(Error::Unsupported)
        }
    })
}
/// Receive notification into an initialized ABI-v1 Endpoint. Stop has family/port=0.
/// Rejects multi-address Start with Unsupported; ABI v1 accepts one selected NIC only.
#[no_mangle]
pub unsafe extern "C" fn llb_ipc_receive(
    fd: i32,
    timeout_ms: u32,
    kind: *mut u32,
    endpoint: *mut Endpoint,
) -> i32 {
    guard(|| {
        if kind.is_null()
            || endpoint.is_null()
            || timeout_ms == 0
            || endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
            || endpoint.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        #[cfg(target_os = "linux")]
        {
            let mut stream = borrowed_notification_stream(fd)?;
            if light_bridge_usbip::helper::peer_uid(&stream).map_err(|_| Error::Ownership)?
                != libc::geteuid()
            {
                return Err(Error::Ownership);
            }
            let notification = light_bridge_protocol::ipc::receive(
                &mut stream,
                Duration::from_millis(timeout_ms.into()),
            )?;
            let mut out = Endpoint {
                size: std::mem::size_of::<Endpoint>() as u32,
                abi_version: 1,
                family: 0,
                port: 0,
                address: [0; 16],
                scope_id: 0,
                reserved: 0,
                host_id: 0,
                correlation_id: 0,
            };
            let event = match notification {
                light_bridge_protocol::ipc::Notification::Start(info) => {
                    if info.addresses.len() != 1 {
                        return Err(Error::Unsupported);
                    }
                    out.host_id = info.host_id;
                    out.correlation_id = info.correlation_id;
                    match info.addresses[0] {
                        std::net::SocketAddr::V4(a) => {
                            out.family = 4;
                            out.port = a.port() as u32;
                            out.address[..4].copy_from_slice(&a.ip().octets());
                        }
                        std::net::SocketAddr::V6(a) => {
                            out.family = 6;
                            out.port = a.port() as u32;
                            out.scope_id = a.scope_id();
                            out.address = a.ip().octets();
                        }
                    };
                    1
                }
                light_bridge_protocol::ipc::Notification::Stop {
                    host_id,
                    correlation_id,
                } => {
                    out.host_id = host_id;
                    out.correlation_id = correlation_id;
                    2
                }
            };
            endpoint.write(out);
            kind.write(event);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = fd;
            Err(Error::Unsupported)
        }
    })
}
/// Apply Start to an already connected HMD, or request Stop after exact host/session checks.
#[no_mangle]
pub unsafe extern "C" fn llb_apply_notification(
    handle: *mut Handle,
    kind: u32,
    endpoint: *const Endpoint,
) -> i32 {
    if kind == 1 {
        if handle.is_null() {
            return 1;
        }
        if (*handle).role != LLB_HMD {
            return 2;
        }
        if endpoint.is_null() {
            return 1;
        }
        // Read the common prefix before accessing correlation. request_start repeats validation.
        if endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
            || endpoint.cast::<u32>().add(1).read() != VERSION
        {
            return 1;
        }
        return llb_request_start(handle, (*endpoint).correlation_id, endpoint);
    }
    guard(|| {
        if kind != 2
            || endpoint.is_null()
            || endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
            || endpoint.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        let e = &*endpoint;
        if h.peer_host != Some(e.host_id) {
            return Err(Error::WrongSession);
        }
        match &mut h.library {
            Library::Hmd(v) if v.session.id == e.correlation_id => v.request_stop()?,
            Library::Hmd(_) => return Err(Error::WrongSession),
            _ => return Err(Error::InvalidState),
        };
        h.notify();
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_request_prepare(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Host(v) => v.prepare_tracking(),
            _ => Err(Error::InvalidState),
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_tracking_ready(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Host(v) => v.tracking_ready(),
            _ => Err(Error::InvalidState),
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_mock_clock(
    handle: *mut Handle,
    now_ns: i64,
    offset_ns: i64,
    uncertainty_ns: u64,
) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Hmd(v) => v.set_mock_clock(
                now_ns,
                ClockMapping {
                    source_clock_id: 1,
                    offset_ns,
                    uncertainty_ns,
                },
            ),
            _ => Err(Error::InvalidState),
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_publish_pose(handle: *mut Handle, pose: *const ExternalPose) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        if pose.is_null()
            || pose.cast::<u32>().read() != std::mem::size_of::<ExternalPose>() as u32
            || pose.cast::<u32>().add(1).read() != VERSION
        {
            return Err(Error::InvalidArgument);
        }
        match &mut h.library {
            Library::Host(v) => v.publish_pose(*pose),
            _ => Err(Error::InvalidState),
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_poll(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Hmd(v) => v.poll()?,
            Library::Host(v) => v.poll()?,
            _ => return Err(Error::InvalidState),
        }
        h.notify();
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_request_stop(handle: *mut Handle, runtime_released: u32) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &mut h.library {
            Library::Hmd(v) => v.request_stop()?,
            Library::Host(v) => v.stop(runtime_released == 1)?,
            _ => return Err(Error::InvalidState),
        }
        h.notify();
        Ok(())
    })
}
#[no_mangle]
pub unsafe extern "C" fn llb_destroy(handle: *mut *mut Handle) -> i32 {
    guard(|| {
        if handle.is_null() {
            return Err(Error::InvalidArgument);
        }
        let p = *handle;
        if p.is_null() {
            return Ok(());
        }
        *handle = std::ptr::null_mut();
        let mut h = Box::from_raw(p);
        h.callback = None;
        drop(h);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn llb_heartbeat(handle: *mut Handle) -> i32 {
    guard(|| {
        let h = handle.as_mut().ok_or(Error::InvalidArgument)?;
        match &h.library {
            Library::Hmd(v) => v.heartbeat(),
            Library::Host(v) => v.heartbeat(),
            _ => Err(Error::InvalidState),
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn llb_get_state(
    handle: *const Handle,
    state: *mut u32,
    needs_runtime_release: *mut u32,
) -> i32 {
    guard(|| {
        let h = handle.as_ref().ok_or(Error::InvalidArgument)?;
        if state.is_null() || needs_runtime_release.is_null() {
            return Err(Error::InvalidArgument);
        }
        *state = h.state() as u32;
        *needs_runtime_release = match &h.library {
            Library::Host(v) => u32::from(v.needs_runtime_release()),
            _ => 0,
        };
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_layout_and_panic_barrier() {
        assert_eq!(LLB_ABI_VERSION, VERSION);
        assert_eq!(std::mem::size_of::<ExternalPose>(), 136);
        assert_eq!(std::mem::offset_of!(ExternalPose, position_m), 80);
        assert_eq!(std::mem::size_of::<Endpoint>(), 56);
        assert_eq!(guard(|| panic!("injected")), 255);
        assert_eq!(guard(|| Err(Error::Backpressure)), 6);
    }
    #[test]
    fn invalid_version_and_owning_slot() {
        unsafe {
            let mut h = std::ptr::null_mut();
            let mut c = Config {
                size: std::mem::size_of::<Config>() as u32,
                abi_version: 2,
                role: 0,
                timeout_ms: 1,
                callback: None,
                userdata: std::ptr::null_mut(),
            };
            assert_eq!(llb_create(&c, &mut h), 1);
            assert!(h.is_null());
            c.abi_version = VERSION;
            assert_eq!(llb_create(&c, &mut h), 0);
            assert_eq!(llb_create(&c, &mut h), 1);
            assert_eq!(llb_request_start(h, 1, std::ptr::null()), 2);
            assert_eq!(llb_destroy(&mut h), 0);
            assert!(h.is_null());
            assert_eq!(llb_destroy(&mut h), 0);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_configuration_is_explicit_and_mock_api_cannot_activate_it() {
        unsafe {
            let config = Config {
                size: std::mem::size_of::<Config>() as u32,
                abi_version: 1,
                role: LLB_HMD,
                timeout_ms: 1000,
                callback: None,
                userdata: std::ptr::null_mut(),
            };
            let mut h = std::ptr::null_mut();
            assert_eq!(llb_create(&config, &mut h), 0);
            let mut device = light_bridge_usbip::UsbDeviceIdentity::mock();
            device.busid = "1-3.3".into();
            device.sysfs_path = "/sys/devices/mock/1-3.3".into();
            let policy = light_bridge_usbip::kernel::Policy {
                role: light_bridge_usbip::kernel::Role::Export,
                client_uid: 1000,
                client_gid: 1000,
                device,
                remote: "192.0.2.1:3240".parse().unwrap(),
            };
            let policy = serde_json::to_vec(&policy).unwrap();
            let socket = b"/run/llfa-link-light-bridge/export.sock";
            assert_eq!(
                llb_configure_kernel_usbip(
                    h,
                    socket.as_ptr(),
                    socket.len() as u32,
                    policy.as_ptr(),
                    policy.len() as u32,
                    9
                ),
                0
            );
            assert_eq!(llb_tcp_connect_mock(h, std::ptr::null()), 2);
            assert_eq!(
                llb_configure_kernel_usbip(
                    h,
                    socket.as_ptr(),
                    socket.len() as u32,
                    policy.as_ptr(),
                    policy.len() as u32,
                    9
                ),
                2
            );
            assert_eq!(llb_destroy(&mut h), 0);
        }
    }
}
