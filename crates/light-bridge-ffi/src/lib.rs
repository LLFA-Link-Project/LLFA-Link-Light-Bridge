//! C ABI v1, mock-only connection. Callers serialize access and must not reenter callbacks.
//! # Safety
//! All non-null pointers must be valid, aligned and point to the documented live objects.
//! Userdata outlives the handle; callbacks borrow pose for the duration of the call only.
#![allow(clippy::missing_safety_doc)]
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
        if h.role != 0
            || s.role != 1
            || !matches!(h.library, Library::Unlinked)
            || !matches!(s.library, Library::Unlinked)
        {
            return Err(Error::InvalidState);
        }
        let (a, b) = mock_pair();
        let usb = Arc::new(Mutex::new(MockUsbIp::ready()));
        h.library = Library::Hmd(HmdClient::new(a, usb.clone(), h.timeout));
        s.library = Library::Host(HostServer::new(b, usb, s.timeout));
        Ok(())
    })
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
                if endpoint.is_null() {
                    return Err(Error::InvalidArgument);
                }
                if endpoint.cast::<u32>().read() != std::mem::size_of::<Endpoint>() as u32
                    || endpoint.cast::<u32>().add(1).read() != VERSION
                {
                    return Err(Error::InvalidArgument);
                }
                let e = &*endpoint;
                if e.port == 0 || e.port > 65535 || e.reserved != 0 {
                    return Err(Error::InvalidArgument);
                }
                let address = match e.family {
                    4 => std::net::SocketAddr::from((
                        std::net::Ipv4Addr::new(
                            e.address[0],
                            e.address[1],
                            e.address[2],
                            e.address[3],
                        ),
                        e.port as u16,
                    )),
                    6 => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                        std::net::Ipv6Addr::from(e.address),
                        e.port as u16,
                        0,
                        e.scope_id,
                    )),
                    _ => return Err(Error::InvalidArgument),
                };
                v.start(
                    session,
                    &EndpointInfo {
                        addresses: vec![address],
                        host_id: e.host_id,
                        correlation_id: e.correlation_id,
                    },
                )?;
            }
            _ => return Err(Error::InvalidState),
        }
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
}
