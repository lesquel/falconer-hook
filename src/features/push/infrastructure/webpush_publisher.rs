//! Web Push publisher (RFC 8030): one POST of the `aes128gcm` message with a
//! VAPID `Authorization` header. Same client rules as the ntfy publisher:
//! no redirects, no environment proxies, 10 s timeout, generic `User-Agent`,
//! public addresses only; `404`/`410` mean the subscription is gone.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::features::pairing::domain::WebPush;
use crate::features::push::domain::{
    validate_webpush_endpoint, PostOutcome, WebPushPayload, WebPushSender, WEBPUSH_TTL_SECS,
};
use crate::features::push::infrastructure::aes128gcm_sealer;
use crate::features::push::infrastructure::ntfy_publisher::{build_client, outcome_for};
use crate::features::push::infrastructure::vapid;
use crate::shared::domain::encoding::b64u_decode_array;

pub struct WebPushPublisher {
    client: reqwest::blocking::Client,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl WebPushPublisher {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: build_client()?,
        })
    }

    fn deliver(&self, wp: &WebPush, kind: &str, now: u64) -> PostOutcome {
        if validate_webpush_endpoint(&wp.endpoint).is_err() {
            return PostOutcome::Failed("invalid_endpoint");
        }
        self.post_message(wp, kind, now)
    }

    /// Encrypts, signs and POSTs (the endpoint is already validated).
    fn post_message(&self, wp: &WebPush, kind: &str, now: u64) -> PostOutcome {
        let (Some(p256dh), Some(auth)) = (
            b64u_decode_array::<65>(&wp.p256dh),
            b64u_decode_array::<16>(&wp.auth),
        ) else {
            return PostOutcome::Failed("invalid_key");
        };
        let Ok(body) = aes128gcm_sealer::seal(
            &p256dh,
            &auth,
            WebPushPayload::for_kind(kind).to_json().as_bytes(),
        ) else {
            return PostOutcome::Failed("invalid_key");
        };
        let Some(authorization) = vapid::authorization(wp, now) else {
            return PostOutcome::Failed("invalid_key");
        };
        let res = self
            .client
            .post(&wp.endpoint)
            .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("TTL", WEBPUSH_TTL_SECS.to_string())
            .header("Urgency", "high")
            .header(reqwest::header::AUTHORIZATION, authorization)
            .body(body)
            .send();
        outcome_for(res)
    }
}

impl WebPushSender for WebPushPublisher {
    fn send(&self, wp: &WebPush, kind: &str) -> PostOutcome {
        self.deliver(wp, kind, unix_now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::WebPushInput;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// One-shot HTTP server on 127.0.0.1; reports the request head and body.
    fn serve_once(status_line: &'static str) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/wp", l.local_addr().unwrap());
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
            let mut s = s;
            write!(
                s,
                "HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            tx.send((head, body)).unwrap();
        });
        (url, rx)
    }

    fn wp() -> WebPush {
        let v = crate::features::pairing::domain::webpush_fixtures::input("mailto:me@example.com");
        WebPushInput::parse(&v.to_string())
            .unwrap()
            .validate()
            .unwrap()
    }

    #[test]
    fn invalid_endpoints_are_never_contacted() {
        let p = WebPushPublisher::new().unwrap();
        let mut w = wp();
        // Plain http to a non-tailnet host is refused before any connection.
        w.endpoint = "http://example.com/x".into();
        assert_eq!(p.send(&w, "done"), PostOutcome::Failed("invalid_endpoint"));
        // http is refused even to a tailnet host, which ntfy would accept.
        w.endpoint = "http://100.101.1.2:2586/x".into();
        assert_eq!(p.send(&w, "done"), PostOutcome::Failed("invalid_endpoint"));
    }

    #[test]
    fn statuses_map_like_the_ntfy_transport() {
        let p = WebPushPublisher::new().unwrap();
        let mut w = wp();
        for (status, want) in [
            ("201 Created", PostOutcome::Delivered),
            ("410 Gone", PostOutcome::Stale),
            ("404 Not Found", PostOutcome::Stale),
            ("413 Payload Too Large", PostOutcome::Failed("http_status")),
        ] {
            let (url, _rx) = serve_once(status);
            // `validate_webpush_endpoint` refuses loopback; test the POST half.
            w.endpoint = url;
            assert_eq!(p.post_message(&w, "done", 1_000), want, "{status}");
        }
    }

    #[test]
    fn the_request_carries_the_web_push_headers_and_a_generic_payload() {
        let p = WebPushPublisher::new().unwrap();
        let mut w = wp();
        let (url, rx) = serve_once("201 Created");
        w.endpoint = url;
        assert_eq!(p.post_message(&w, "done", 1_000), PostOutcome::Delivered);
        let (head, got) = rx.recv().unwrap();
        let head = head.to_ascii_lowercase();
        assert!(
            head.contains("\r\ncontent-encoding: aes128gcm\r\n"),
            "{head}"
        );
        assert!(head.contains("\r\ncontent-type: application/octet-stream\r\n"));
        assert!(head.contains("\r\nttl: 900\r\n"));
        assert!(head.contains("\r\nurgency: high\r\n"));
        assert!(head.contains("\r\nauthorization: vapid t="));
        assert!(head.contains("\r\nuser-agent: openmosh\r\n"));
        // header (86) + 256-byte bucket + tag (16)
        assert_eq!(got.len(), 86 + 256 + 16);
    }
}
