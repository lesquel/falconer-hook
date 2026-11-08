//! Publish one plaintext to every paired device.

use serde::Serialize;

use crate::features::pairing::domain::{DeviceRegistry, RegistryError};
use crate::features::push::domain::{
    PostOutcome, PushPlaintext, PushPublisher, PushTarget, Sealer,
};
use crate::shared::domain::encoding::b64u_decode_array;
use crate::shared::domain::error::ErrorCode;
use crate::shared::infrastructure::log::{log, Field, Level};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PublishReport {
    pub delivered: u32,
    pub stale: u32,
    pub failed: u32,
    pub skipped: u32,
}

pub struct PublishEvent<'a, R, S, P> {
    pub registry: &'a R,
    pub sealer: &'a S,
    pub publisher: &'a P,
}

impl<R: DeviceRegistry, S: Sealer, P: PushPublisher> PublishEvent<'_, R, S, P> {
    /// Seals and posts `plaintext` for each device. Devices with a stale
    /// endpoint are skipped; a `404`/`410` marks the device `endpoint_stale`.
    pub fn publish(&self, plaintext: &PushPlaintext) -> Result<PublishReport, RegistryError> {
        let mut report = PublishReport::default();
        for device in self.registry.list()? {
            if device.endpoint_stale {
                report.skipped += 1;
                continue;
            }
            let Some(ppk) = b64u_decode_array::<32>(&device.ppk) else {
                report.failed += 1;
                continue;
            };
            let target = PushTarget {
                device_id: device.id.clone(),
                ppk: *ppk,
                endpoint: device.endpoint.clone(),
            };
            let body = match self.sealer.seal(&target, plaintext) {
                Ok(b) => b,
                Err(e) => {
                    report.failed += 1;
                    // Code only (e.g. `push_too_large`); never the body or device.
                    log(
                        Level::Warn,
                        "push sealing failed",
                        &[("reason", Field::Code(e.code()))],
                    );
                    continue;
                }
            };
            match self.publisher.post(&target.endpoint, &body) {
                PostOutcome::Delivered => report.delivered += 1,
                PostOutcome::Stale => {
                    report.stale += 1;
                    let mut d = device;
                    d.endpoint_stale = true;
                    self.registry.update(d)?;
                }
                PostOutcome::Failed(reason) => {
                    report.failed += 1;
                    log(
                        Level::Warn,
                        "push delivery failed",
                        &[("reason", Field::Code(reason))],
                    );
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::fakes::MemRegistry;
    use crate::features::pairing::domain::Device;
    use crate::features::push::domain::PushError;
    use crate::shared::domain::encoding::b64u_encode;
    use std::cell::RefCell;

    struct EchoSealer;
    impl Sealer for EchoSealer {
        fn seal(&self, t: &PushTarget, p: &PushPlaintext) -> Result<String, PushError> {
            Ok(format!("{}:{}", t.device_id, p.id))
        }
    }

    struct ScriptedPublisher(RefCell<Vec<(String, String)>>, fn(&str) -> PostOutcome);
    impl PushPublisher for ScriptedPublisher {
        fn post(&self, endpoint: &str, body: &str) -> PostOutcome {
            self.0.borrow_mut().push((endpoint.into(), body.into()));
            (self.1)(endpoint)
        }
    }

    fn device(id: &str, endpoint: &str, stale: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            ppk: b64u_encode(&[9; 32]),
            endpoint: endpoint.into(),
            paired_at: 1,
            endpoint_stale: stale,
            ssh_key: None,
        }
    }

    #[test]
    fn publishes_to_each_device_and_marks_gone_endpoints_stale() {
        let reg = MemRegistry::with(vec![
            device("a", "https://ok", false),
            device("b", "https://gone", false),
            device("c", "https://skip", true),
            device("d", "https://fail", false),
        ]);
        let publisher = ScriptedPublisher(RefCell::new(vec![]), |e| match e {
            "https://ok" => PostOutcome::Delivered,
            "https://gone" => PostOutcome::Stale,
            _ => PostOutcome::Failed("network"),
        });
        let uc = PublishEvent {
            registry: &reg,
            sealer: &EchoSealer,
            publisher: &publisher,
        };
        let pt = PushPlaintext::new("e1", 1, "h", "claude", "done", "t", "s", "p", 1);
        let r = uc.publish(&pt).unwrap();
        assert_eq!(
            r,
            PublishReport {
                delivered: 1,
                stale: 1,
                failed: 1,
                skipped: 1
            }
        );
        assert_eq!(
            publisher.0.borrow()[0],
            ("https://ok".to_string(), "a:e1".to_string())
        );
        assert!(reg.get("b").unwrap().endpoint_stale);
        assert!(!reg.get("a").unwrap().endpoint_stale);
    }
}
