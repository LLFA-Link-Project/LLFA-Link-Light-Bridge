//! Versioned, bounded local lifecycle notifications. Never carries pose or IMU.
//! Caller owns the UDS listener/path and must restrict its containing directory.
use super::*;
use light_bridge_core::EndpointInfo;
#[cfg(test)]
use std::io::Write;
use std::os::unix::net::UnixStream;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    Start(EndpointInfo),
    Stop { host_id: u64, correlation_id: u64 },
}
impl Notification {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let (kind, host, correlation, addresses) = match self {
            Self::Start(e) => {
                e.validate()?;
                (1u32, e.host_id, e.correlation_id, e.addresses.as_slice())
            }
            Self::Stop {
                host_id,
                correlation_id,
            } => (2, *host_id, *correlation_id, &[][..]),
        };
        if host == 0 || correlation == 0 || addresses.len() > 8 {
            return Err(Error::InvalidArgument);
        }
        let mut b = b"LLBI".to_vec();
        for n in [VERSION, kind] {
            b.extend(n.to_be_bytes());
        }
        for n in [host, correlation] {
            b.extend(n.to_be_bytes());
        }
        b.extend((addresses.len() as u32).to_be_bytes());
        for address in addresses {
            let address = address.to_string();
            b.extend((address.len() as u32).to_be_bytes());
            b.extend(address.as_bytes());
        }
        if b.len() + 4 > MAX_FRAME {
            return Err(Error::InvalidArgument);
        }
        let mut frame = (b.len() as u32).to_be_bytes().to_vec();
        frame.extend(b);
        Ok(frame)
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        if !(36..=MAX_FRAME).contains(&b.len()) {
            return Err(Error::Protocol);
        }
        let mut r = Reader { bytes: b, pos: 0 };
        if r.u32()? as usize != b.len() - 4 || r.take(4)? != b"LLBI" || r.u32()? != VERSION {
            return Err(Error::Protocol);
        }
        let kind = r.u32()?;
        let host_id = r.u64()?;
        let correlation_id = r.u64()?;
        let count = r.u32()?;
        if host_id == 0 || correlation_id == 0 || count > 8 {
            return Err(Error::Protocol);
        }
        let mut addresses = Vec::new();
        for _ in 0..count {
            let n = r.u32()? as usize;
            let text = std::str::from_utf8(r.take(n)?).map_err(|_| Error::Protocol)?;
            addresses.push(text.parse().map_err(|_| Error::Protocol)?);
        }
        if r.pos != b.len() {
            return Err(Error::Protocol);
        }
        match kind {
            1 => {
                let e = EndpointInfo {
                    addresses,
                    host_id,
                    correlation_id,
                };
                e.validate()?;
                Ok(Self::Start(e))
            }
            2 if addresses.is_empty() => Ok(Self::Stop {
                host_id,
                correlation_id,
            }),
            _ => Err(Error::Protocol),
        }
    }
}
/// Leaves the supplied stream nonblocking; serial control-thread calls only.
pub fn send(stream: &mut UnixStream, notification: &Notification, timeout: Duration) -> Result<()> {
    stream
        .set_nonblocking(true)
        .map_err(|_| Error::Disconnected)?;
    transfer(
        stream,
        &mut notification.encode()?,
        true,
        std::time::Instant::now() + timeout,
    )
}
pub fn receive(stream: &mut UnixStream, timeout: Duration) -> Result<Notification> {
    stream
        .set_nonblocking(true)
        .map_err(|_| Error::Disconnected)?;
    let end = std::time::Instant::now() + timeout;
    let mut length = [0; 4];
    transfer(stream, &mut length, false, end)?;
    let n = u32::from_be_bytes(length) as usize;
    if !(32..=MAX_FRAME - 4).contains(&n) {
        return Err(Error::Protocol);
    }
    let mut b = length.to_vec();
    b.resize(n + 4, 0);
    transfer(stream, &mut b[4..], false, end)?;
    Notification::decode(&b)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_nics_ipv6_and_stop_strict_codec() {
        let n = Notification::Start(EndpointInfo {
            addresses: vec![
                "127.0.0.1:42".parse().unwrap(),
                "[fe80::1%7]:42".parse().unwrap(),
            ],
            host_id: 17,
            correlation_id: 19,
        });
        for n in [
            n,
            Notification::Stop {
                host_id: 17,
                correlation_id: 19,
            },
        ] {
            let b = n.encode().unwrap();
            assert_eq!(Notification::decode(&b).unwrap(), n);
            for len in 0..b.len() {
                assert!(Notification::decode(&b[..len]).is_err());
            }
            let mut b = b;
            b[11] = 99;
            assert_eq!(Notification::decode(&b), Err(Error::Protocol));
        }
    }
    #[test]
    fn uds_timeout_and_oversized_before_allocation() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        assert_eq!(
            receive(&mut a, Duration::from_millis(5)),
            Err(Error::Timeout)
        );
        b.write_all(&u32::MAX.to_be_bytes()).unwrap();
        assert_eq!(
            receive(&mut a, Duration::from_millis(5)),
            Err(Error::Protocol)
        );
    }
}
