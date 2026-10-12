#![cfg(target_os = "linux")]
use light_bridge_usbip::fd_channel;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
fn tcp() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (server, client)
}
#[test]
fn duplex_descriptor_transfer_preserves_tcp_flags_and_data() {
    let (mut host, mut pi) = tcp();
    let original = unsafe { libc::fcntl(host.as_raw_fd(), libc::F_GETFL) };
    let (mut a, mut b) = UnixStream::pair().unwrap();
    let cancel = AtomicBool::new(false);
    let end = Instant::now() + Duration::from_secs(1);
    fd_channel::send(
        &mut a,
        &serde_json::json!({"prepare":true}),
        Some(host.as_raw_fd()),
        end,
        &cancel,
    )
    .unwrap();
    let (value, fd): (serde_json::Value, _) = fd_channel::receive(&mut b, end, &cancel).unwrap();
    assert!(value["prepare"].as_bool().unwrap());
    let fd = fd.unwrap();
    assert_eq!(
        fd_channel::route_from_fd(fd.as_raw_fd()).unwrap(),
        fd_channel::route_from_fd(host.as_raw_fd()).unwrap()
    );
    assert_ne!(
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    fd_channel::send(
        &mut b,
        &serde_json::json!({"ready":true}),
        None,
        end,
        &cancel,
    )
    .unwrap();
    let (reply, none): (serde_json::Value, _) = fd_channel::receive(&mut a, end, &cancel).unwrap();
    assert_eq!(reply["ready"], true);
    assert!(none.is_none());
    assert_eq!(
        unsafe { libc::fcntl(host.as_raw_fd(), libc::F_GETFL) },
        original
    );
    drop(fd);
    pi.write_all(b"pose").unwrap();
    let mut bytes = [0; 4];
    host.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"pose");
}
#[test]
fn cancellation_and_bad_frames_close_without_retrying_consumed_framing() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    let cancel = AtomicBool::new(true);
    assert!(fd_channel::send(
        &mut a,
        &0,
        None,
        Instant::now() + Duration::from_secs(1),
        &cancel
    )
    .is_err());
    cancel.store(false, std::sync::atomic::Ordering::Release);
    a.write_all(b"LLBA\0\0\0\x02\0\0\0\x01x").unwrap();
    assert!(fd_channel::receive::<serde_json::Value>(
        &mut b,
        Instant::now() + Duration::from_secs(1),
        &cancel
    )
    .is_err());
    drop(a);
    drop(b);
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(b"LLBA\0\0\0\x01\xff\xff\xff\xff").unwrap();
    assert!(fd_channel::receive::<serde_json::Value>(
        &mut b,
        Instant::now() + Duration::from_secs(1),
        &cancel
    )
    .unwrap_err()
    .contains("size"));
    let (a, mut b) = UnixStream::pair().unwrap();
    drop(a);
    assert!(fd_channel::receive::<serde_json::Value>(
        &mut b,
        Instant::now() + Duration::from_secs(1),
        &cancel
    )
    .is_err());
    assert!(fd_channel::route_from_fd(b.as_raw_fd()).is_err());
}
#[test]
fn extra_ancillary_fds_are_rejected_and_received_duplicates_are_closed() {
    let (host, pi) = tcp();
    let (a, mut b) = UnixStream::pair().unwrap();
    let mut data = *b"L";
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 8];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(8) } as usize;
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(8) as usize;
        let fds = [host.as_raw_fd(), pi.as_raw_fd()];
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(c), 8);
        assert_eq!(libc::sendmsg(a.as_raw_fd(), &msg, libc::MSG_NOSIGNAL), 1);
    }
    assert!(fd_channel::receive::<serde_json::Value>(
        &mut b,
        Instant::now() + Duration::from_secs(1),
        &AtomicBool::new(false)
    )
    .unwrap_err()
    .contains("ancillary"));
    assert!(fd_channel::route_from_fd(host.as_raw_fd()).is_ok());
    assert!(fd_channel::route_from_fd(pi.as_raw_fd()).is_ok());
}
