//! One-shot pairing listener. Bound by the caller to the Tailscale IPv4 only,
//! on an ephemeral port; each connection gets one request line (≤ 16 KiB)
//! and one reply line. The whole request must arrive within
//! `min(10 s, time left before expiry)`; cancellation (Ctrl+C) and expiry are
//! checked at least every 100 ms, even while a slow peer is sending. Stops on
//! success, expiry, cancellation, a "no" on the host or after too many
//! failures; the token is burned. A pairing whose phone hung up before the
//! reply could be sent (e.g. while the host user was still deciding) is
//! taken back, so no key stays authorized for a phone that never got it.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::pair_crypto::{MAX_LINE_BYTES, REJECTED_LINE};
use crate::features::pairing::application::{Attempt, PairSession};
use crate::features::pairing::domain::Device;
use crate::shared::infrastructure::log::{log, Field, Level};

/// Upper bound for reading one request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// How often cancellation is checked while waiting.
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Eq)]
pub enum ListenEnd {
    Paired(Box<Device>),
    /// Expired, too many failures or cancelled.
    Closed {
        cancelled: bool,
    },
}

/// A reader that fails once `deadline` passes or `cancel` is set, however
/// slowly the peer sends.
struct DeadlineReader<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
    cancel: &'a AtomicBool,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                // Not `Interrupted`: `read_until` retries on that kind.
                return Err(io::Error::other("cancelled"));
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left.min(POLL)))?;
            match (&*self.stream).read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                r => return r,
            }
        }
    }
}

/// True when the peer has already closed its side (EOF or reset).
fn peer_hung_up(stream: &TcpStream) -> bool {
    let mut b = [0u8; 1];
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let gone = match stream.peek(&mut b) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != io::ErrorKind::WouldBlock,
    };
    let _ = stream.set_nonblocking(false);
    gone
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
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => return Err(e),
        };
        stream.set_nonblocking(false)?;
        stream.set_write_timeout(Some(REQUEST_DEADLINE))?;
        let deadline = Instant::now() + REQUEST_DEADLINE.min(session.remaining());
        let attempt = session.handle_connection(peer.ip(), &mut || {
            read_request_line(&mut DeadlineReader {
                stream: &stream,
                deadline,
                cancel,
            })
        });
        match attempt {
            Attempt::Paired { device, reply } => {
                let sent = !peer_hung_up(&stream)
                    && stream.write_all(format!("{reply}\n").as_bytes()).is_ok()
                    && stream.flush().is_ok();
                session.burn();
                if !sent {
                    session.undo(&device);
                    log(
                        Level::Warn,
                        "pairing taken back",
                        &[("reason", Field::Code("phone_hung_up"))],
                    );
                    return Ok(ListenEnd::Closed { cancelled: false });
                }
                return Ok(ListenEnd::Paired(Box::new(device)));
            }
            Attempt::Declined => {
                let _ = stream.write_all(format!("{REJECTED_LINE}\n").as_bytes());
                let _ = stream.flush();
                session.burn();
                return Ok(ListenEnd::Closed { cancelled: true });
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
    use std::sync::Arc;
    use std::time::Instant;

    /// Accepts only the line "good\n"; closes after `max_failures` failures.
    struct Script {
        failures: u32,
        burned: bool,
        undone: bool,
        peers: Vec<IpAddr>,
        max_failures: u32,
        remaining: Duration,
    }

    fn script(max_failures: u32, remaining: Duration) -> Script {
        Script {
            failures: 0,
            burned: false,
            undone: false,
            peers: vec![],
            max_failures,
            remaining,
        }
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
                        peer_ips: vec![],
                        webpush: None,
                    },
                    reply: "{\"v\":1}".into(),
                },
                Some("no\n") => Attempt::Declined,
                _ => {
                    self.failures += 1;
                    Attempt::Rejected { reason: "bad" }
                }
            }
        }
        fn is_closed(&self) -> bool {
            self.failures >= self.max_failures
        }
        fn remaining(&self) -> Duration {
            self.remaining
        }
        fn burn(&mut self) {
            self.burned = true;
        }
        fn undo(&mut self, _: &Device) {
            self.undone = true;
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
        let mut s = script(2, Duration::from_secs(300));
        let end = run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap();
        let (a, b) = client.join().unwrap();
        assert_eq!(a, "{\"v\":1,\"err\":\"pair_rejected\"}\n");
        assert_eq!(b, "{\"v\":1}\n");
        assert!(matches!(end, ListenEnd::Paired(_)));
        assert!(s.burned);
    }

    #[test]
    fn a_no_on_the_host_ends_the_session() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || exchange(addr, b"no\n"));
        let mut s = script(5, Duration::from_secs(300));
        let end = run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            client.join().unwrap(),
            "{\"v\":1,\"err\":\"pair_rejected\"}\n"
        );
        assert_eq!(end, ListenEnd::Closed { cancelled: true });
        assert!(s.burned);
    }

    #[test]
    fn a_phone_that_hung_up_is_not_left_paired() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"good\n").unwrap();
            s.shutdown(std::net::Shutdown::Both).unwrap();
        });
        client.join().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let mut s = script(5, Duration::from_secs(300));
        let end = run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap();
        assert_eq!(end, ListenEnd::Closed { cancelled: false });
        assert!(s.undone && s.burned);
    }

    #[test]
    fn closes_after_failures_and_on_cancel() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            exchange(addr, b"x\n");
            exchange(addr, &vec![b'a'; MAX_LINE_BYTES + 10]);
        });
        let mut s = script(2, Duration::from_secs(300));
        assert_eq!(
            run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap(),
            ListenEnd::Closed { cancelled: false }
        );
        client.join().unwrap();
        let mut s = script(2, Duration::from_secs(300));
        assert_eq!(
            run_listener(&l, &mut s, &AtomicBool::new(true)).unwrap(),
            ListenEnd::Closed { cancelled: true }
        );
        assert!(s.burned);
    }

    /// Connects and writes one byte every 100 ms for up to 6 s.
    fn dribble(addr: std::net::SocketAddr) {
        std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(6) {
                if s.write_all(b"a").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    #[test]
    fn slow_client_is_cut_at_the_session_deadline() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        dribble(l.local_addr().unwrap());
        let mut s = script(1, Duration::from_secs(1));
        let start = Instant::now();
        let end = run_listener(&l, &mut s, &AtomicBool::new(false)).unwrap();
        assert_eq!(end, ListenEnd::Closed { cancelled: false });
        assert!(
            start.elapsed() < Duration::from_millis(2500),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cancel_is_honored_during_a_slow_connection() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        dribble(l.local_addr().unwrap());
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            c.store(true, Ordering::SeqCst);
        });
        let mut s = script(5, Duration::from_secs(300));
        let start = Instant::now();
        let end = run_listener(&l, &mut s, &cancel).unwrap();
        assert_eq!(end, ListenEnd::Closed { cancelled: true });
        assert!(s.burned);
        assert!(
            start.elapsed() < Duration::from_millis(2000),
            "took {:?}",
            start.elapsed()
        );
    }
}
