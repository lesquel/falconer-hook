//! One-shot pairing listener. Bound by the caller to the Tailscale IPv4 only,
//! on an ephemeral port; each connection gets one request line (≤ 16 KiB,
//! 10 s read timeout) and one reply line. Stops on success, expiry,
//! cancellation (Ctrl+C) or after too many failures; the token is burned.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::pair_crypto::{MAX_LINE_BYTES, REJECTED_LINE};
use crate::features::pairing::application::{Attempt, PairSession};
use crate::features::pairing::domain::Device;
use crate::shared::infrastructure::log::{log, Field, Level};

#[derive(Debug, PartialEq, Eq)]
pub enum ListenEnd {
    Paired(Box<Device>),
    /// Expired, too many failures or cancelled.
    Closed {
        cancelled: bool,
    },
}

fn read_request_line(stream: &mut impl Read) -> Option<String> {
    let mut buf = Vec::new();
    let mut r = BufReader::new(stream.take(MAX_LINE_BYTES as u64 + 1));
    r.read_until(b'\n', &mut buf).ok()?;
    if buf.is_empty() || buf.len() > MAX_LINE_BYTES {
        return None;
    }
    String::from_utf8(buf).ok()
}

pub fn run_listener(
    listener: &TcpListener,
    session: &mut dyn PairSession,
    cancel: &AtomicBool,
) -> io::Result<ListenEnd> {
    listener.set_nonblocking(true)?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            session.burn();
            return Ok(ListenEnd::Closed { cancelled: true });
        }
        if session.is_closed() {
            session.burn();
            return Ok(ListenEnd::Closed { cancelled: false });
        }
        let (mut stream, peer) = match listener.accept() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e),
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut reader = stream.try_clone()?;
        let attempt = session.handle_connection(peer.ip(), &mut || read_request_line(&mut reader));
        match attempt {
            Attempt::Paired { device, reply } => {
                let _ = stream.write_all(format!("{reply}\n").as_bytes());
                let _ = stream.flush();
                session.burn();
                return Ok(ListenEnd::Paired(Box::new(device)));
            }
            Attempt::Rejected { reason } => {
                log(
                    Level::Warn,
                    "pairing attempt rejected",
                    &[("reason", Field::Code(reason))],
                );
                let _ = stream.write_all(format!("{REJECTED_LINE}\n").as_bytes());
                let _ = stream.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, TcpStream};

    /// Accepts only the line "good\n"; closes after 2 failures.
    struct Script {
        failures: u32,
        burned: bool,
        peers: Vec<IpAddr>,
    }
    impl PairSession for Script {
        fn handle_connection(
            &mut self,
            peer: IpAddr,
            read: &mut dyn FnMut() -> Option<String>,
        ) -> Attempt {
            self.peers.push(peer);
            match read().as_deref() {
                Some("good\n") => Attempt::Paired {
                    device: Device {
                        id: "d".into(),
                        name: "n".into(),
                        ppk: String::new(),
                        endpoint: String::new(),
                        paired_at: 0,
                        endpoint_stale: false,
                        ssh_key: None,
                    },
                    reply: "{\"v\":1}".into(),
                },
                _ => {
                    self.failures += 1;
                    Attempt::Rejected { reason: "bad" }
                }
            }
        }
        fn is_closed(&self) -> bool {
            self.failures >= 2
        }
        fn burn(&mut self) {
            self.burned = true;
        }
    }

    fn exchange(addr: std::net::SocketAddr, line: &[u8]) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(line).unwrap();
        let mut out = String::new();
        BufReader::new(s).read_line(&mut out).unwrap();
        out
    }

    #[test]
    fn replies_generically_then_pairs_and_burns() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let a = exchange(addr, b"bad\n");
            let b = exchange(addr, b"good\n");
            (a, b)
        });
        let mut s = Script {
            failures: 0,
            burned: false,
            peers: vec![],
        };
        let end = run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap();
        let (a, b) = client.join().unwrap();
        assert_eq!(a, "{\"v\":1,\"err\":\"pair_rejected\"}\n");
        assert_eq!(b, "{\"v\":1}\n");
        assert!(matches!(end, ListenEnd::Paired(_)));
        assert!(s.burned);
    }

    #[test]
    fn closes_after_failures_and_on_cancel() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            exchange(addr, b"x\n");
            exchange(addr, &vec![b'a'; MAX_LINE_BYTES + 10]);
        });
        let mut s = Script {
            failures: 0,
            burned: false,
            peers: vec![],
        };
        assert_eq!(
            run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap(),
            ListenEnd::Closed { cancelled: false }
        );
        client.join().unwrap();
        let mut s = Script {
            failures: 0,
            burned: false,
            peers: vec![],
        };
        assert_eq!(
            run_listener(&l, &mut s, &AtomicBool::new(true)).unwrap(),
            ListenEnd::Closed { cancelled: true }
        );
        assert!(s.burned);
    }
}
