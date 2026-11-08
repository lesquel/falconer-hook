//! UnifiedPush / ntfy publisher: one HTTPS POST of the encrypted body.
//! No redirects are followed, environment proxies are ignored, 10 s timeout,
//! generic `User-Agent: openmosh` (no version); `404`/`410` mean the endpoint is stale.

use std::time::Duration;

use crate::features::push::domain::{validate_endpoint, PostOutcome, PushPublisher};

pub struct HttpPublisher {
    client: reqwest::blocking::Client,
}

impl HttpPublisher {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(10))
            .no_proxy()
            .user_agent("openmosh")
            .build()?;
        Ok(Self { client })
    }

    fn send(&self, endpoint: &str, body: &str) -> PostOutcome {
        let res = self
            .client
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send();
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
    fn invalid_endpoint_is_never_contacted() {
        let p = HttpPublisher::new().unwrap();
        assert_eq!(
            p.post("http://example.com/up", "{}"),
            PostOutcome::Failed("invalid_endpoint")
        );
    }
}
