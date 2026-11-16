//! Shared cross-implementation test vectors (`tests/vectors/*.json`).
//!
//! The app's crypto tests consume these files. They are generated from fixed
//! keys and nonces by `regenerate_vectors` (run with
//! `FALCONER_WRITE_VECTORS=1 cargo test regenerate_vectors -- --ignored`) and
//! verified on every `cargo test` by `vectors_match_implementation`.

use serde::{Deserialize, Serialize};

use crate::features::pairing::domain::PairResponsePlain;
use crate::features::pairing::infrastructure::pair_crypto::{
    derive_pair_keys, open_response, seal_request, seal_response_with_nonce,
};
use crate::features::push::domain::PushPlaintext;
use crate::features::push::infrastructure::xchacha_sealer::{push_key, seal_push_with_nonce};
use crate::shared::domain::encoding::{b64u_decode_array, b64u_encode};
use crate::shared::infrastructure::crypto::{diffie_hellman, public_key};

const PUSH_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/push_v1.json");
const PAIR_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/pair_v1.json");

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct PushVector {
    host_priv: String,
    host_pub: String,
    device_priv: String,
    device_pub: String,
    device_id: String,
    nonce: String,
    key: String,
    plaintext: String,
    body: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct PairVector {
    host_priv: String,
    host_pub: String,
    eph_priv: String,
    eph_pub: String,
    token: String,
    k_req: String,
    k_resp: String,
    nonce_req: String,
    req_plaintext: String,
    req_line: String,
    nonce_resp: String,
    resp_plaintext: String,
    resp_line: String,
}

fn seq<const N: usize>(start: u8) -> [u8; N] {
    let mut a = [0u8; N];
    for (i, b) in a.iter_mut().enumerate() {
        *b = start.wrapping_add(i as u8);
    }
    a
}

const HOST_PRIV: u8 = 0x01;
const DEVICE_PRIV: u8 = 0x21;
const EPH_PRIV: u8 = 0x41;

fn build_push() -> PushVector {
    let host: [u8; 32] = seq(HOST_PRIV);
    let device: [u8; 32] = seq(DEVICE_PRIV);
    let device_id = b64u_encode(&seq::<16>(0xA0));
    let nonce: [u8; 24] = seq(0x00);
    let pt = PushPlaintext::new(
        &b64u_encode(&seq::<16>(0x10)),
        42,
        "devbox",
        "claude",
        "approval_required",
        "claude needs approval",
        "Allow Bash: cargo test — ✓ ünïcödé \"quoted\"",
        "w7:pA",
        1_762_500_000,
    );
    let plaintext = serde_json::to_string(&pt).unwrap();
    let key = push_key(&host, &public_key(&device), &device_id).unwrap();
    let body =
        seal_push_with_nonce(&host, &device_id, &public_key(&device), &nonce, &plaintext).unwrap();
    PushVector {
        host_priv: b64u_encode(&host),
        host_pub: b64u_encode(&public_key(&host)),
        device_priv: b64u_encode(&device),
        device_pub: b64u_encode(&public_key(&device)),
        device_id,
        nonce: b64u_encode(&nonce),
        key: b64u_encode(&*key),
        plaintext,
        body,
    }
}

fn build_pair() -> PairVector {
    let host: [u8; 32] = seq(HOST_PRIV);
    let eph: [u8; 32] = seq(EPH_PRIV);
    let token: [u8; 16] = seq(0x80);
    let nonce_req: [u8; 24] = seq(0x30);
    let nonce_resp: [u8; 24] = seq(0x50);
    let device_pub = public_key(&seq::<32>(DEVICE_PRIV));
    let shared = diffie_hellman(&eph, &public_key(&host)).unwrap();
    let keys = derive_pair_keys(&shared, &token);
    let req_plaintext = serde_json::to_string(&crate::features::pairing::domain::PairRequestPlain {
        tok: b64u_encode(&token),
        name: "Pixel 8".into(),
        ssh_pub: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH phone".into(),
        ppk: b64u_encode(&device_pub),
        endpoint: "https://ntfy.sh/upTestTopic123?up=1".into(),
    })
    .unwrap();
    let req_line =
        seal_request(&eph, &public_key(&host), &token, &nonce_req, &req_plaintext).unwrap();
    let resp_plaintext = serde_json::to_string(&PairResponsePlain {
        device_id: b64u_encode(&seq::<16>(0xA0)),
        alias: "devbox".into(),
        ssh_user: "me".into(),
        ssh_port: 22,
        ssh_fp: "SHA256:XmA4TG6/WNqReEVbojfXfv3nyHYvdvEr6WbFjs7VdLM".into(),
    })
    .unwrap();
    let resp_line = seal_response_with_nonce(&keys.k_resp, &nonce_resp, &resp_plaintext);
    PairVector {
        host_priv: b64u_encode(&host),
        host_pub: b64u_encode(&public_key(&host)),
        eph_priv: b64u_encode(&eph),
        eph_pub: b64u_encode(&public_key(&eph)),
        token: b64u_encode(&token),
        k_req: b64u_encode(&*keys.k_req),
        k_resp: b64u_encode(&*keys.k_resp),
        nonce_req: b64u_encode(&nonce_req),
        req_plaintext,
        req_line,
        nonce_resp: b64u_encode(&nonce_resp),
        resp_plaintext,
        resp_line,
    }
}

#[test]
#[ignore = "writes tests/vectors; run explicitly to regenerate"]
fn regenerate_vectors() {
    if std::env::var("FALCONER_WRITE_VECTORS").as_deref() != Ok("1") {
        return;
    }
    let push = serde_json::to_string_pretty(&build_push()).unwrap() + "\n";
    let pair = serde_json::to_string_pretty(&build_pair()).unwrap() + "\n";
    std::fs::write(PUSH_PATH, push).unwrap();
    std::fs::write(PAIR_PATH, pair).unwrap();
}

#[test]
fn vectors_match_implementation() {
    let push: PushVector =
        serde_json::from_str(include_str!("../tests/vectors/push_v1.json")).unwrap();
    let pair: PairVector =
        serde_json::from_str(include_str!("../tests/vectors/pair_v1.json")).unwrap();
    assert_eq!(push, build_push(), "push_v1.json is out of date");
    assert_eq!(pair, build_pair(), "pair_v1.json is out of date");
}

#[test]
fn vectors_decrypt_from_the_other_side() {
    use crate::features::pairing::domain::PairCrypto;
    use crate::features::pairing::infrastructure::pair_crypto::X25519PairCrypto;
    use crate::shared::domain::encoding::b64u_decode;
    use crate::shared::infrastructure::crypto::open;

    // Push: the device derives the key from its private key and the host public key.
    let v = build_push();
    let device = b64u_decode_array::<32>(&v.device_priv).unwrap();
    let hpk = b64u_decode_array::<32>(&v.host_pub).unwrap();
    let k = push_key(&device, &hpk, &v.device_id).unwrap();
    assert_eq!(b64u_encode(&*k), v.key);
    let body: serde_json::Value = serde_json::from_str(&v.body).unwrap();
    let n = b64u_decode_array::<24>(body["n"].as_str().unwrap()).unwrap();
    let c = b64u_decode(body["c"].as_str().unwrap()).unwrap();
    let pt = open(&k, &n, &c, b"openmosh/push/v1").unwrap();
    assert_eq!(pt.as_slice(), v.plaintext.as_bytes());

    // Pair: the host opens the request line; the phone opens the response.
    let p = build_pair();
    let host = X25519PairCrypto::new(b64u_decode_array::<32>(&p.host_priv).unwrap());
    let token = b64u_decode_array::<16>(&p.token).unwrap();
    let (req, k_resp) = host.open_request(&p.req_line, &token).unwrap();
    assert_eq!(serde_json::to_string(&req).unwrap(), p.req_plaintext);
    assert_eq!(b64u_encode(&*k_resp), p.k_resp);
    assert_eq!(
        open_response(&k_resp, &p.resp_line).unwrap(),
        p.resp_plaintext
    );
}
