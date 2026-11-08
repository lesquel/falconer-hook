use super::*;
use crate::features::pairing::domain::fakes::{FakeTailnet, MemAuthorizedKeys, MemRegistry};
use crate::features::pairing::domain::{PairRequestPlain, TailnetStatus};
use crate::shared::domain::clock::fakes::FixedClock;
use crate::shared::domain::ids::fakes::SeqIds;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH";
const TOKEN: [u8; 32] = [42u8; 32];
const PHONE: &str = "100.101.1.2";

/// Fake crypto: a request line is `<b64u(token)>.<plaintext json>`.
struct FakeCrypto;
impl PairCrypto for FakeCrypto {
    type Session = ();
    fn open_request(&self, line: &str, token: &[u8; 32]) -> Option<(PairRequestPlain, ())> {
        let (tok, json) = line.split_once('.')?;
        (tok == b64u_encode(token)).then_some(())?;
        Some((serde_json::from_str(json).ok()?, ()))
    }
    fn seal_response(&self, _: &(), r: &PairResponsePlain) -> String {
        serde_json::to_string(r).unwrap()
    }
}

fn request(tok: &[u8; 32], f: impl FnOnce(&mut PairRequestPlain)) -> String {
    let mut r = PairRequestPlain {
        tok: b64u_encode(tok),
        name: "Pixel 8".into(),
        ssh_pub: format!("{KEY} phone"),
        ppk: b64u_encode(&[7u8; 32]),
        endpoint: "https://ntfy.sh/upX?up=1".into(),
    };
    f(&mut r);
    format!(
        "{}.{}",
        b64u_encode(tok),
        serde_json::to_string(&r).unwrap()
    )
}

struct World {
    tailnet: FakeTailnet,
    registry: MemRegistry,
    keys: MemAuthorizedKeys,
    clock: FixedClock,
    ids: SeqIds,
}

impl World {
    fn new() -> Self {
        Self {
            tailnet: FakeTailnet::running(
                "me@example.com",
                &[
                    (PHONE, "me@example.com"),
                    ("100.101.9.9", "other@example.com"),
                ],
            ),
            registry: MemRegistry::default(),
            keys: MemAuthorizedKeys::default(),
            clock: FixedClock::at(1_000),
            ids: SeqIds::default(),
        }
    }

    fn pairing(
        &self,
    ) -> QrPairing<'_, FakeTailnet, FakeCrypto, MemRegistry, MemAuthorizedKeys, FixedClock, SeqIds>
    {
        let pre = Preflight::check(&self.tailnet).unwrap();
        let facts = HostFacts {
            alias: "devbox".into(),
            ssh_user: "me".into(),
            ssh_port: 22,
            ssh_fp: "SHA256:abc".into(),
            hpk: [1u8; 32],
        };
        QrPairing::new(
            &self.tailnet,
            &FakeCrypto,
            &self.registry,
            &self.keys,
            &self.clock,
            &self.ids,
            pre,
            facts,
            TOKEN,
            300,
        )
    }
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn reason(a: Attempt) -> &'static str {
    match a {
        Attempt::Rejected { reason } => reason,
        Attempt::Paired { .. } => "paired",
    }
}

#[test]
fn preflight_refuses_when_tailscale_is_not_running() {
    let mut t = FakeTailnet::running("me@example.com", &[]);
    t.status = TailnetStatus {
        running: false,
        ..t.status.clone()
    };
    assert_eq!(Preflight::check(&t), Err(PairError::TailscaleNotRunning));
    t.missing = true;
    assert_eq!(Preflight::check(&t), Err(PairError::TailscaleMissing));
}

#[test]
fn offer_has_wire_fields() {
    let w = World::new();
    let p = w.pairing();
    let o = p.offer(41234);
    assert_eq!(o.ip, "100.88.77.66");
    assert_eq!(o.addr, "host.tail.ts.net");
    assert_eq!(o.exp, 1_300);
    assert_eq!(o.tok, b64u_encode(&TOKEN));
    let json = serde_json::to_string(&o).unwrap();
    assert!(json.starts_with(r#"{"v":1,"alias":"devbox","addr":"host.tail.ts.net","ip":"100.88.77.66","ssh_port":22,"ssh_user":"me","ssh_fp":"SHA256:abc","hpk":""#));
    assert!(json.ends_with(&format!(
        r#","port":41234,"tok":"{}","exp":1300}}"#,
        b64u_encode(&TOKEN)
    )));
    assert!(o.uri().starts_with("openmosh://pair?d="));
}

#[test]
fn successful_pairing_authorizes_key_stores_device_and_burns_token() {
    let w = World::new();
    let mut p = w.pairing();
    let Attempt::Paired { device, reply } = p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))
    else {
        panic!("expected success")
    };
    assert_eq!(device.name, "Pixel 8");
    assert_eq!(device.ssh_key.as_deref(), Some(KEY));
    assert_eq!(
        reply,
        format!(r#"{{"device_id":"{}","alias":"devbox"}}"#, device.id)
    );
    assert_eq!(
        w.keys.lines.borrow().as_slice(),
        [format!("from=\"100.64.0.0/10,fd7a:115c:a1e0::/48\",no-agent-forwarding,no-X11-forwarding {KEY} openmosh:{}", device.id)]
    );
    assert_eq!(w.registry.list().unwrap(), vec![device]);
    assert!(p.is_closed());
    // Reuse of the same token is refused.
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "already_used"
    );
    p.burn();
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "decrypt_failed"
    );
}

#[test]
fn every_rejection_path_in_order() {
    let w = World::new();
    let mut p = w.pairing();
    let never_read = || -> Option<String> { panic!("must not read from an unverified peer") };
    assert_eq!(
        reason(p.handle(ip("192.168.1.5"), never_read)),
        "not_tailnet"
    );
    assert_eq!(
        reason(p.handle(ip("100.101.9.9"), never_read)),
        "whois_mismatch"
    );
    assert_eq!(
        reason(p.handle(ip("100.101.7.7"), never_read)),
        "whois_mismatch"
    );
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&[0u8; 32], |_| {})))),
        "decrypt_failed"
    );
    assert_eq!(reason(p.handle(ip(PHONE), || None)), "read_failed");
    // Five failures: closed, even a perfect request is now refused.
    assert!(p.is_closed());
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "too_many_failures"
    );
    assert!(w.keys.lines.borrow().is_empty());
    assert!(w.registry.list().unwrap().is_empty());
}

#[test]
fn token_inside_ciphertext_must_match() {
    let w = World::new();
    let mut p = w.pairing();
    let line = request(&TOKEN, |r| r.tok = b64u_encode(&[9u8; 32]));
    assert_eq!(reason(p.handle(ip(PHONE), || Some(line))), "token_mismatch");
}

#[test]
fn expired_token_is_refused() {
    let w = World::new();
    let mut p = w.pairing();
    w.clock.advance(301);
    assert!(p.is_closed());
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "expired"
    );
}

#[test]
fn bad_ssh_key_endpoint_name_and_ppk_are_refused() {
    let w = World::new();
    let mut p = w.pairing();
    type Case = (fn(&mut PairRequestPlain), &'static str);
    let cases: [Case; 4] = [
        (
            |r| r.ssh_pub = "ssh-rsa AAAAB3NzaC1yc2E".into(),
            "bad_ssh_key",
        ),
        (|r| r.endpoint = "http://ntfy.sh/up".into(), "bad_endpoint"),
        (|r| r.name = "\u{1b}\n".into(), "bad_name"),
        (|r| r.ppk = "short".into(), "bad_ppk"),
    ];
    for (f, want) in cases {
        assert_eq!(
            reason(p.handle(ip(PHONE), || Some(request(&TOKEN, f)))),
            want
        );
    }
    assert!(w.keys.lines.borrow().is_empty());
}

#[test]
fn storage_failure_rolls_back_and_rejects() {
    let mut w = World::new();
    w.keys.fail_append = true;
    let mut p = w.pairing();
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "storage"
    );
    assert!(w.registry.list().unwrap().is_empty());
    assert!(!p.is_closed());
}

#[test]
fn whois_error_is_a_rejection() {
    let mut w = World::new();
    w.tailnet.whois_fails = true;
    let mut p = w.pairing();
    assert_eq!(
        reason(p.handle(ip(PHONE), || Some(request(&TOKEN, |_| {})))),
        "whois_failed"
    );
}
