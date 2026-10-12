//! Bounded LLBA v1 duplex frames and a single SCM_RIGHTS TCP route descriptor.
use crate::{
    bootstrap::{Route, MAX_PACKET},
    kernel::KResult,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
fn wait(end: Instant, cancel: &AtomicBool) -> KResult<()> {
    check(end, cancel)?;
    std::thread::sleep(Duration::from_millis(1));
    Ok(())
}
fn check(end: Instant, cancel: &AtomicBool) -> KResult<()> {
    if cancel.load(Ordering::Acquire) || Instant::now() >= end {
        return Err("bootstrap cancelled/timeout; close stream".into());
    }
    Ok(())
}
pub fn send<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    fd: Option<RawFd>,
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_PACKET {
        return Err("bootstrap packet too large".into());
    }
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut first = *b"L";
    let mut iov = libc::iovec {
        iov_base: first.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        if fd < 0 {
            return Err("invalid route fd".into());
        }
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<RawFd>(), fd);
        }
    }
    loop {
        check(end, cancel)?;
        let rc = unsafe {
            libc::sendmsg(
                stream.as_raw_fd(),
                &msg,
                libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
            )
        };
        if rc == 1 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if rc < 0
            && matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            )
        {
            wait(end, cancel)?;
        } else {
            return Err(format!("bootstrap sendmsg: {e}"));
        }
    }
    let mut tail = Vec::with_capacity(bytes.len() + 11);
    tail.extend_from_slice(b"LBA");
    tail.extend_from_slice(&1u32.to_be_bytes());
    tail.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    tail.extend_from_slice(&bytes);
    let mut n = 0;
    while n < tail.len() {
        check(end, cancel)?;
        match stream.write(&tail[n..]) {
            Ok(0) => return Err("bootstrap disconnected".into()),
            Ok(count) => n += count,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                wait(end, cancel)?
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
pub fn receive<T: DeserializeOwned>(
    stream: &mut UnixStream,
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<(T, Option<OwnedFd>)> {
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut first = [0u8];
    let mut iov = libc::iovec {
        iov_base: first.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 32];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    loop {
        check(end, cancel)?;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&control);
        msg.msg_flags = 0;
        let rc = unsafe {
            libc::recvmsg(
                stream.as_raw_fd(),
                &mut msg,
                libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC,
            )
        };
        if rc == 1 {
            break;
        }
        if rc == 0 {
            return Err("bootstrap disconnected".into());
        }
        let e = std::io::Error::last_os_error();
        if matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            wait(end, cancel)?;
        } else {
            return Err(e.to_string());
        }
    }
    let mut fds = Vec::new();
    let mut invalid = false;
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let header = libc::CMSG_LEN(0) as usize;
                if (*c).cmsg_len < header {
                    invalid = true;
                    break;
                }
                let n = ((*c).cmsg_len - header) / std::mem::size_of::<RawFd>();
                for i in 0..n {
                    let fd = std::ptr::read_unaligned(libc::CMSG_DATA(c).cast::<RawFd>().add(i));
                    fds.push(OwnedFd::from_raw_fd(fd));
                }
            } else {
                invalid = true;
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    if invalid
        || msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
        || fds.len() > 1
        || first[0] != b'L'
    {
        return Err("bootstrap ancillary/header invalid; close stream".into());
    }
    let mut header = [0; 11];
    read_exact(stream, &mut header, end, cancel)?;
    if &header[..3] != b"LBA" || u32::from_be_bytes(header[3..7].try_into().unwrap()) != 1 {
        return Err("bootstrap version invalid; close stream".into());
    }
    let n = u32::from_be_bytes(header[7..11].try_into().unwrap()) as usize;
    if n == 0 || n > MAX_PACKET {
        return Err("bootstrap packet size invalid; close stream".into());
    }
    let mut bytes = vec![0; n];
    read_exact(stream, &mut bytes, end, cancel)?;
    let value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok((value, fds.pop()))
}
fn read_exact(
    stream: &mut UnixStream,
    bytes: &mut [u8],
    end: Instant,
    cancel: &AtomicBool,
) -> KResult<()> {
    let mut n = 0;
    while n < bytes.len() {
        check(end, cancel)?;
        match stream.read(&mut bytes[n..]) {
            Ok(0) => return Err("bootstrap disconnected mid-frame; close stream".into()),
            Ok(count) => n += count,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                wait(end, cancel)?
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
fn socket_option(fd: RawFd, opt: i32) -> KResult<i32> {
    let mut value = 0i32;
    let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            opt,
            (&mut value as *mut i32).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of::<i32>()
    {
        return Err("TCP socket option unavailable".into());
    }
    Ok(value)
}
pub fn namespace_cookie(fd: RawFd) -> KResult<u64> {
    let mut cookie = 0u64;
    let mut len = 8 as libc::socklen_t;
    // SO_NETNS_COOKIE, Linux UAPI. Unsupported kernels fail closed.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            71,
            (&mut cookie as *mut u64).cast(),
            &mut len,
        )
    } != 0
        || len != 8
        || cookie == 0
    {
        return Err("network namespace cookie unavailable".into());
    }
    Ok(cookie)
}
pub fn local_namespace_cookie() -> KResult<u64> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    namespace_cookie(fd.as_raw_fd())
}
pub fn route_from_fd(fd: RawFd) -> KResult<Route> {
    if fd < 0
        || socket_option(fd, libc::SO_TYPE)? != libc::SOCK_STREAM
        || socket_option(fd, libc::SO_PROTOCOL)? != libc::IPPROTO_TCP
    {
        return Err("connected TCP descriptor required".into());
    }
    let mut info = [0u8; 256];
    let mut length = info.len() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            info.as_mut_ptr().cast(),
            &mut length,
        )
    } != 0
        || length == 0
        || info[0] != 1
    {
        return Err("TCP route is not established".into());
    }
    fn address(fd: RawFd, peer: bool) -> KResult<SocketAddr> {
        let mut a: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of_val(&a) as libc::socklen_t;
        let rc = unsafe {
            if peer {
                libc::getpeername(fd, (&mut a as *mut libc::sockaddr_storage).cast(), &mut len)
            } else {
                libc::getsockname(fd, (&mut a as *mut libc::sockaddr_storage).cast(), &mut len)
            }
        };
        if rc != 0 {
            return Err("socket is not connected".into());
        }
        match i32::from(a.ss_family) {
            libc::AF_INET => {
                let v =
                    unsafe { &*(&a as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
                Ok(SocketAddr::V4(SocketAddrV4::new(
                    v.sin_addr.s_addr.to_ne_bytes().into(),
                    u16::from_be(v.sin_port),
                )))
            }
            libc::AF_INET6 => {
                let v =
                    unsafe { &*(&a as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>() };
                let ip: std::net::Ipv6Addr = v.sin6_addr.s6_addr.into();
                if let Some(ip) = ip.to_ipv4_mapped() {
                    Ok(SocketAddr::new(IpAddr::V4(ip), u16::from_be(v.sin6_port)))
                } else {
                    Ok(SocketAddr::V6(SocketAddrV6::new(
                        ip,
                        u16::from_be(v.sin6_port),
                        v.sin6_flowinfo,
                        v.sin6_scope_id,
                    )))
                }
            }
            _ => Err("non-IP route descriptor".into()),
        }
    }
    let r = Route {
        local: address(fd, false)?,
        peer: address(fd, true)?,
        netns_cookie: namespace_cookie(fd)?,
    };
    r.validate(true)?;
    Ok(r)
}
