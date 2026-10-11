//! TCP bridge for the bounded transport. Setup belongs on the adapter control thread.
//! Two sockets isolate pose from control. Correlation is association, not authentication.
use super::*;
use light_bridge_core::EndpointInfo;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub(crate) struct Workers {
    cancel: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
fn io_error(e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Error::Timeout,
        _ => Error::Disconnected,
    }
}
fn remaining(end: Instant) -> Result<Duration> {
    end.checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(Error::Timeout)
}
fn preface(endpoint: &EndpointInfo, lane: u8) -> Vec<u8> {
    let mut b = b"LLBN".to_vec();
    b.extend(PROTOCOL_VERSION.to_be_bytes());
    b.extend(endpoint.host_id.to_be_bytes());
    b.extend(endpoint.correlation_id.to_be_bytes());
    b.push(lane);
    b
}
fn setup(stream: &TcpStream, end: Instant) -> Result<()> {
    remaining(end)?;
    stream.set_nonblocking(true).map_err(io_error)?;
    stream.set_nodelay(true).map_err(io_error)
}
/// Try each advertised NIC under one overall deadline. No USB or runtime initialization.
pub fn connect(endpoint: &EndpointInfo, timeout: Duration) -> Result<Transport> {
    endpoint.validate()?;
    let end = Instant::now() + timeout;
    for address in &endpoint.addresses {
        let control = match TcpStream::connect_timeout(address, remaining(end)?) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut streams = vec![control];
        match TcpStream::connect_timeout(address, remaining(end)?) {
            Ok(s) => streams.push(s),
            Err(_) => continue,
        }
        for (lane, stream) in streams.iter_mut().enumerate() {
            setup(stream, end)?;
            transfer(stream, &mut preface(endpoint, lane as u8), true, end)?;
        }
        for stream in &mut streams {
            let mut ack = [0];
            transfer(stream, &mut ack, false, end)?;
            if ack != [1] {
                return Err(Error::Protocol);
            }
        }
        let pose = streams.pop().unwrap();
        return from_streams(streams.pop().unwrap(), pose);
    }
    Err(Error::Disconnected)
}
/// Accept exactly the expected host/correlation and lanes. Call on a control worker.
/// A malformed candidate fails this attempt; caller chooses retry policy.
pub fn accept(
    listener: &TcpListener,
    endpoint: &EndpointInfo,
    timeout: Duration,
) -> Result<Transport> {
    endpoint.validate()?;
    listener.set_nonblocking(true).map_err(io_error)?;
    let end = Instant::now() + timeout;
    let mut streams = Vec::new();
    for lane in 0..2 {
        let mut stream = loop {
            remaining(end)?;
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1))
                }
                Err(e) => return Err(io_error(e)),
            }
        };
        setup(&stream, end)?;
        let mut b = [0; 25];
        transfer(&mut stream, &mut b, false, end)?;
        if b.as_slice() != preface(endpoint, lane) {
            return Err(Error::WrongSession);
        }
        streams.push(stream);
    }
    for stream in &mut streams {
        transfer(stream, &mut [1], true, end)?;
    }
    let pose = streams.pop().unwrap();
    from_streams(streams.pop().unwrap(), pose)
}
fn from_streams(control: TcpStream, pose: TcpStream) -> Result<Transport> {
    control.set_nonblocking(true).map_err(io_error)?;
    pose.set_nonblocking(true).map_err(io_error)?;
    let (mut user, mut wire) = mock_pair();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    for (stream, rx, tx, is_pose) in [
        (control, wire.control_rx, wire.control_tx, false),
        (pose, wire.pose_rx.take().unwrap(), wire.pose_tx, true),
    ] {
        let cancel = cancel.clone();
        threads.push(thread::spawn(move || {
            let _ = pump(stream, rx, tx, is_pose, &cancel);
            cancel.store(true, Ordering::Release);
        }));
    }
    user.network = Some(Workers { cancel, threads });
    Ok(user)
}
fn pump(
    mut stream: TcpStream,
    rx: Receiver<Vec<u8>>,
    tx: SyncSender<Vec<u8>>,
    pose: bool,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut input = Vec::new();
    let mut pending: Option<(Vec<u8>, usize, Instant)> = None;
    let mut partial_since = None;
    while !cancel.load(Ordering::Acquire) {
        if pending.is_none() {
            match rx.try_recv() {
                Ok(b) => pending = Some((b, 0, Instant::now())),
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some((b, offset, since)) = pending.as_mut() {
            if since.elapsed() > Duration::from_secs(2) {
                return Err(Error::Timeout);
            }
            match stream.write(&b[*offset..]) {
                Ok(0) => return Err(Error::Disconnected),
                Ok(n) => *offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(io_error(e)),
            }
            if *offset == b.len() {
                pending = None;
            }
        }
        let mut b = [0; MAX_FRAME];
        match stream.read(&mut b) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => {
                if input.is_empty() {
                    partial_since = Some(Instant::now());
                }
                input.extend_from_slice(&b[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(io_error(e)),
        }
        while input.len() >= 4 {
            let length = u32::from_be_bytes(input[..4].try_into().unwrap()) as usize;
            if !(28..=MAX_FRAME - 4).contains(&length) {
                return Err(Error::Protocol);
            }
            if input.len() < length + 4 {
                break;
            }
            let frame: Vec<_> = input.drain(..length + 4).collect();
            let message = Message::decode(&frame)?;
            if (message.kind == Kind::Pose) != pose {
                return Err(Error::Protocol);
            }
            match tx.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) if pose => {} // bounded, drop newest pose
                Err(TrySendError::Full(_)) => return Err(Error::Backpressure),
                Err(TrySendError::Disconnected(_)) => return Err(Error::Disconnected),
            }
            partial_since = if input.is_empty() {
                None
            } else {
                Some(Instant::now())
            };
        }
        if partial_since.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
            return Err(Error::Timeout);
        }
        thread::sleep(Duration::from_millis(1));
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sockets() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        (a, l.accept().unwrap().0)
    }
    fn wait_disconnect(t: &Transport) {
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            if t.receive() == Err(Error::Disconnected) {
                break;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn fragmented_frames_and_pose_lane_separate_from_control() {
        let (a, mut b) = sockets();
        let (c, mut d) = sockets();
        let t = from_streams(a, c).unwrap();
        let frame = Message::control(Kind::Hello, 1, 1).encode().unwrap();
        for byte in frame {
            b.write_all(&[byte]).unwrap();
        }
        assert_eq!(
            t.receive_timeout(Duration::from_secs(1)).unwrap().kind,
            Kind::Hello
        );
        let frame = Message {
            kind: Kind::Pose,
            session_id: 1,
            operation_id: 1,
            pose: Some(ExternalPose::mock(1, 1, 42)),
            device: None,
        }
        .encode()
        .unwrap();
        d.write_all(&frame).unwrap();
        assert_eq!(
            Message::decode(
                &t.pose_rx
                    .as_ref()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
            )
            .unwrap()
            .pose
            .unwrap()
            .sample_timestamp_ns,
            42
        );
        // A pose frame on the control lane terminates both workers.
        b.write_all(&frame).unwrap();
        wait_disconnect(&t);
    }
    #[test]
    fn oversized_frame_and_wrong_correlation_fail_closed() {
        let (a, mut b) = sockets();
        let (c, _d) = sockets();
        let t = from_streams(a, c).unwrap();
        b.write_all(&u32::MAX.to_be_bytes()).unwrap();
        wait_disconnect(&t);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = EndpointInfo {
            addresses: vec![listener.local_addr().unwrap()],
            host_id: 17,
            correlation_id: 1,
        };
        let address = endpoint.addresses[0];
        let worker =
            thread::spawn(move || accept(&listener, &endpoint, Duration::from_secs(1)).err());
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(&preface(
                &EndpointInfo {
                    addresses: vec![address],
                    host_id: 17,
                    correlation_id: 2,
                },
                0,
            ))
            .unwrap();
        assert_eq!(worker.join().unwrap(), Some(Error::WrongSession));
    }
    #[test]
    fn ipv6_connection_and_missing_lane_deadline() {
        let listener = TcpListener::bind("[::1]:0").unwrap();
        let endpoint = EndpointInfo {
            addresses: vec![listener.local_addr().unwrap()],
            host_id: 17,
            correlation_id: 1,
        };
        let expected = endpoint.clone();
        let worker = thread::spawn(move || accept(&listener, &expected, Duration::from_secs(1)));
        let a = connect(&endpoint, Duration::from_secs(1)).unwrap();
        let b = worker.join().unwrap().unwrap();
        a.send(Kind::Hello, 1, 1).unwrap();
        assert_eq!(
            b.receive_timeout(Duration::from_secs(1)).unwrap().kind,
            Kind::Hello
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = EndpointInfo {
            addresses: vec![listener.local_addr().unwrap()],
            host_id: 17,
            correlation_id: 1,
        };
        let mut control = TcpStream::connect(endpoint.addresses[0]).unwrap();
        control.write_all(&preface(&endpoint, 0)).unwrap();
        let now = Instant::now();
        assert_eq!(
            accept(&listener, &endpoint, Duration::from_millis(15)).err(),
            Some(Error::Timeout)
        );
        assert!(now.elapsed() < Duration::from_secs(1));
    }
}
