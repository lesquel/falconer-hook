//! `pair-ssh --json`: pairing for a host the user already reaches over SSH.
//! SSH authenticated the user, so no token or listener is involved and no
//! key is added to `authorized_keys`.

use serde::{Deserialize, Serialize};

use super::qr::PairError;
use crate::features::pairing::domain::{sanitize_name, valid_ppk, Device, DeviceRegistry};
use crate::features::push::domain::validate_endpoint;
use crate::shared::domain::clock::Clock;
use crate::shared::domain::encoding::b64u_encode;
use crate::shared::domain::ids::IdGenerator;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    name: String,
    ppk: String,
    endpoint: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SshPairResult {
    pub device_id: String,
    pub alias: String,
    pub hpk: String,
}

pub struct SshPairing<'a, R, K, I> {
    pub registry: &'a R,
    pub clock: &'a K,
    pub ids: &'a I,
    pub alias: String,
    pub hpk: [u8; 32],
}

impl<R: DeviceRegistry, K: Clock, I: IdGenerator> SshPairing<'_, R, K, I> {
    pub fn pair(&self, input: &str) -> Result<SshPairResult, PairError> {
        let req: Request = serde_json::from_str(input).map_err(|_| PairError::InvalidInput)?;
        let name = sanitize_name(&req.name).ok_or(PairError::InvalidInput)?;
        // An empty endpoint pairs without notifications.
        let bad_endpoint = !req.endpoint.is_empty() && validate_endpoint(&req.endpoint).is_err();
        if !valid_ppk(&req.ppk) || bad_endpoint {
            return Err(PairError::InvalidInput);
        }
        let device = Device {
            id: self.ids.new_id(),
            name,
            ppk: req.ppk,
            endpoint: req.endpoint,
            paired_at: self.clock.now(),
            endpoint_stale: false,
            ssh_key: None,
            peer_ips: Vec::new(),
        };
        let device_id = device.id.clone();
        self.registry.add(device).map_err(|_| PairError::Storage)?;
        Ok(SshPairResult {
            device_id,
            alias: self.alias.clone(),
            hpk: b64u_encode(&self.hpk),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::fakes::MemRegistry;
    use crate::shared::domain::clock::fakes::FixedClock;
    use crate::shared::domain::ids::fakes::SeqIds;

    #[test]
    fn registers_device_and_returns_host_key() {
        let reg = MemRegistry::default();
        let (clock, ids) = (FixedClock::at(5), SeqIds::default());
        let uc = SshPairing {
            registry: &reg,
            clock: &clock,
            ids: &ids,
            alias: "devbox".into(),
            hpk: [1; 32],
        };
        let input = format!(
            r#"{{"name":"Pixel","ppk":"{}","endpoint":"https://ntfy.sh/up"}}"#,
            b64u_encode(&[7; 32])
        );
        let r = uc.pair(&input).unwrap();
        assert_eq!(
            r,
            SshPairResult {
                device_id: "AAAAAAAAAAAAAAAAAAAA01".into(),
                alias: "devbox".into(),
                hpk: b64u_encode(&[1; 32])
            }
        );
        let d = reg.get(&r.device_id).unwrap();
        assert_eq!(
            (d.name.as_str(), d.paired_at, d.ssh_key.is_none()),
            ("Pixel", 5, true)
        );
    }

    #[test]
    fn pairs_without_a_push_endpoint() {
        let reg = MemRegistry::default();
        let (clock, ids) = (FixedClock::at(5), SeqIds::default());
        let uc = SshPairing {
            registry: &reg,
            clock: &clock,
            ids: &ids,
            alias: "devbox".into(),
            hpk: [1; 32],
        };
        let input = format!(
            r#"{{"name":"Pixel","ppk":"{}","endpoint":""}}"#,
            b64u_encode(&[7; 32])
        );
        let r = uc.pair(&input).unwrap();
        assert_eq!(reg.get(&r.device_id).unwrap().endpoint, "");
    }

    #[test]
    fn rejects_invalid_input() {
        let reg = MemRegistry::default();
        let (clock, ids) = (FixedClock::at(5), SeqIds::default());
        let uc = SshPairing {
            registry: &reg,
            clock: &clock,
            ids: &ids,
            alias: "a".into(),
            hpk: [1; 32],
        };
        let ppk = b64u_encode(&[7; 32]);
        for bad in [
            "nope".to_string(),
            format!(r#"{{"name":"P","ppk":"{ppk}","endpoint":"http://ntfy.sh/up"}}"#),
            r#"{"name":"P","ppk":"abc","endpoint":"https://ntfy.sh/up"}"#.to_string(),
            format!(r#"{{"name":"","ppk":"{ppk}","endpoint":"https://ntfy.sh/up"}}"#),
            format!(r#"{{"name":"P","ppk":"{ppk}","endpoint":"https://ntfy.sh/up","extra":1}}"#),
        ] {
            assert_eq!(uc.pair(&bad), Err(PairError::InvalidInput), "{bad}");
        }
        assert!(reg.list().unwrap().is_empty());
    }
}
