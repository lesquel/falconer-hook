//! UnifiedPush / ntfy publisher: one HTTPS POST of the encrypted body.
//! No redirects are followed, environment proxies are ignored, 10 s timeout,
//! generic `User-Agent: openmosh` (no version); `404`/`410` mean the endpoint is stale.
//! Names are resolved by [`PublicResolver`], which drops every forbidden
//! address, so the connection goes to an address that passed the endpoint
//! rules (no DNS rebinding to the LAN or the host itself).

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use crate::features::push::domain::{forbidden_ip, validate_endpoint, PostOutcome, PushPublisher};

/// The allowed addresses `host` resolves to; an error when there are none.
pub fn resolve_allowed(host: &str) -> std::io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, 0)
        .to_socket_addrs()?
        .filter(|a| !forbidden_ip(a.ip().to_canonical()))
        .collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "push endpoint resolves only to forbidden addresses",
        ));
    }
    Ok(addrs)
}

/// DNS resolver for the push client (see the module docs).
pub struct PublicResolver;

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // The blocking client has its own runtime thread; one lookup at a time.
            let addrs = resolve_allowed(&host)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

pub struct HttpPublisher {
    client: reqwest::blocking::Client,
}

/// The HTTP client every push transport uses (see the module docs).
pub fn build_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(10))
        .no_proxy()
        .dns_resolver(Arc::new(PublicResolver))
        .user_agent("openmosh")
        .build()
}

/// Maps a push service answer to an outcome (shared by both transports).
pub fn outcome_for(res: Result<reqwest::blocking::Response, reqwest::Error>) -> PostOutcome {
    match res {
        Ok(r) if r.status().is_success() => PostOutcome::Delivered,
        Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 410 => PostOutcome::Stale,
        Ok(r) if r.status().is_redirection() => PostOutcome::Failed("redirect_refused"),
        // ntfy refuses UnifiedPush topics whose phone app is not connected.
        Ok(r) if r.status().as_u16() == 507 => PostOutcome::Failed("no_active_subscriber"),
        Ok(_) => PostOutcome::Failed("http_status"),
        Err(e) if e.is_timeout() => PostOutcome::Failed("timeout"),
        Err(_) => PostOutcome::Failed("network"),
    }
}

impl HttpPublisher {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: build_client()?,
        })
    }

    fn send(&self, endpoint: &str, body: &str) -> PostOutcome {
        let res = self
            .client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send();
        outcome_for(res)
    }
}

impl PushPublisher for HttpPublisher {
    fn post(&self, endpoint: &str, body: &str) -> PostOutcome {
        if validate_endpoint(endpoint).is_err() {
            return PostOutcome::Failed("invalid_endpoint");
        }
        self.send(endpoint, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// One-shot HTTP server on 127.0.0.1 answering with `status_line` and
    /// reporting the request it saw.
    fn serve_once(
        status_line: &'static str,
        extra: &'static str,
    ) -> (String, mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/up", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            head.push_str(&String::from_utf8(body).unwrap());
            let mut s = s;
            write!(
                s,
                "HTTP/1.1 {status_line}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            tx.send(head).unwrap();
        });
        (url, rx)
    }

    #[test]
    fn posts_json_and_maps_statuses() {
        let p = HttpPublisher::new().unwrap();
        let (url, rx) = serve_once("200 OK", "");
        assert_eq!(p.send(&url, r#"{"v":1}"#), PostOutcome::Delivered);
        let req = rx.recv().unwrap().to_ascii_lowercase();
        assert!(req.starts_with("post /up"));
        assert!(req.contains("content-type: application/json"));
        assert!(req.ends_with(r#"{"v":1}"#));

        let (url, _rx) = serve_once("410 Gone", "");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Stale);
        let (url, _rx) = serve_once("404 Not Found", "");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Stale);
        let (url, _rx) = serve_once("500 Internal Server Error", "");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Failed("http_status"));
        // ntfy.sh: "cannot publish to UnifiedPush topic without previously active subscriber".
        let (url, _rx) = serve_once("507 Insufficient Storage", "");
        assert_eq!(
            p.send(&url, "{}"),
            PostOutcome::Failed("no_active_subscriber")
        );
    }

    #[test]
    fn user_agent_is_generic() {
        let p = HttpPublisher::new().unwrap();
        let (url, rx) = serve_once("200 OK", "");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Delivered);
        let req = rx.recv().unwrap().to_ascii_lowercase();
        assert!(req.contains("\r\nuser-agent: openmosh\r\n"), "{req}");
    }

    #[test]
    fn environment_proxies_are_ignored() {
        // A dead proxy: if it were used, delivery would fail.
        for k in ["NO_PROXY", "no_proxy"] {
            std::env::remove_var(k);
        }
        for k in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            std::env::set_var(k, "http://127.0.0.1:9");
        }
        let p = HttpPublisher::new().unwrap();
        let (url, _rx) = serve_once("200 OK", "");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Delivered);
    }

    #[test]
    fn redirects_are_not_followed() {
        let p = HttpPublisher::new().unwrap();
        let (url, _rx) = serve_once("302 Found", "Location: http://127.0.0.1:1/elsewhere\r\n");
        assert_eq!(p.send(&url, "{}"), PostOutcome::Failed("redirect_refused"));
    }

    #[test]
    fn names_resolving_to_forbidden_addresses_are_never_contacted() {
        assert!(resolve_allowed("localhost").is_err());
        let p = HttpPublisher::new().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(
            p.send(&format!("http://localhost:{port}/up"), "{}"),
            PostOutcome::Failed("network")
        );
        assert!(l.accept().is_err(), "no connection was made");
    }

    #[test]
    fn invalid_endpoint_is_never_contacted() {
        let p = HttpPublisher::new().unwrap();
        assert_eq!(
            p.post("http://example.com/up", "{}"),
            PostOutcome::Failed("invalid_endpoint")
        );
    }
}
