//! Publish one event to every paired device, over each transport it has.

use serde::Serialize;

use crate::features::pairing::domain::{Device, DeviceRegistry, RegistryError};
use crate::features::push::domain::{
    PostOutcome, PushPlaintext, PushPublisher, PushTarget, Sealer, WebPushSender,
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

pub struct PublishEvent<'a, R, S, P, W> {
    pub registry: &'a R,
    pub sealer: &'a S,
    pub publisher: &'a P,
    pub webpush: &'a W,
}

impl<R: DeviceRegistry, S: Sealer, P: PushPublisher, W: WebPushSender>
    PublishEvent<'_, R, S, P, W>
{
    /// Sends `plaintext` over each transport a device has: the sealed ntfy /
    /// UnifiedPush POST (skipped without an endpoint or when stale) and the
    /// generic Web Push message (skipped without a subscription or when
    /// stale). A `404`/`410` marks only that transport stale. `delivered`
    /// counts devices reached by at least one transport; `stale` and `failed`
    /// count attempts; `skipped` counts devices with nothing to send to.
    pub fn publish(&self, plaintext: &PushPlaintext) -> Result<PublishReport, RegistryError> {
        let mut report = PublishReport::default();
        for device in self.registry.list()? {
            let ntfy = !device.endpoint_stale && !device.endpoint.is_empty();
            // A resolved event only updates the inbox. Browsers must show every
            // web push, so sending it would surface an empty notification.
            let webpush = device
                .webpush
                .as_ref()
                .filter(|w| !w.stale && plaintext.kind != "resolved");
            if !ntfy && webpush.is_none() {
                report.skipped += 1;
                continue;
            }
            let mut delivered = false;
            if ntfy {
                delivered |= self.publish_ntfy(&device, plaintext, &mut report)?;
            }
            if let Some(wp) = webpush {
                match self.webpush.send(wp, &plaintext.kind) {
                    PostOutcome::Delivered => delivered = true,
                    PostOutcome::Stale => {
                        report.stale += 1;
                        // Re-checked under the registry lock, like the endpoint.
                        self.registry.mark_webpush_stale(&device.id, &wp.endpoint)?;
                    }
                    PostOutcome::Failed(reason) => {
                        report.failed += 1;
                        log(
                            Level::Warn,
                            "web push delivery failed",
                            &[("reason", Field::Code(reason))],
                        );
                    }
                }
            }
            if delivered {
                report.delivered += 1;
            }
        }
        Ok(report)
    }

    /// The ntfy transport for one device; whether it was delivered.
    fn publish_ntfy(
        &self,
        device: &Device,
        plaintext: &PushPlaintext,
        report: &mut PublishReport,
    ) -> Result<bool, RegistryError> {
        let Some(ppk) = b64u_decode_array::<32>(&device.ppk) else {
            report.failed += 1;
            return Ok(false);
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
                return Ok(false);
            }
        };
        match self.publisher.post(&target.endpoint, &body) {
            PostOutcome::Delivered => return Ok(true),
            PostOutcome::Stale => {
                report.stale += 1;
                // Re-checked under the registry lock: the user may have
                // set a new endpoint while this push was in flight.
                self.registry.mark_stale(&device.id, &target.endpoint)?;
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
        Ok(false)
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

    /// Records the kinds it was asked to send and answers per endpoint.
    struct ScriptedWebPush(RefCell<Vec<(String, String)>>, fn(&str) -> PostOutcome);
    impl WebPushSender for ScriptedWebPush {
        fn send(&self, wp: &crate::features::pairing::domain::WebPush, kind: &str) -> PostOutcome {
            self.0.borrow_mut().push((wp.endpoint.clone(), kind.into()));
            (self.1)(&wp.endpoint)
        }
    }

    fn no_webpush() -> ScriptedWebPush {
        ScriptedWebPush(RefCell::new(vec![]), |_| PostOutcome::Delivered)
    }

    fn with_webpush(mut d: Device, endpoint: &str, stale: bool) -> Device {
        use crate::features::pairing::domain::{webpush_fixtures::input, WebPushInput};
        let mut v = input("mailto:a@b.c");
        v["endpoint"] = endpoint.into();
        let mut wp = WebPushInput::parse(&v.to_string())
            .unwrap()
            .validate()
            .unwrap();
        wp.stale = stale;
        d.webpush = Some(Box::new(wp));
        d
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
            peer_ips: Vec::new(),
            webpush: None,
        }
    }

    #[test]
    fn publishes_to_each_device_and_marks_gone_endpoints_stale() {
        let reg = MemRegistry::with(vec![
            device("a", "https://ok", false),
            device("b", "https://gone", false),
            device("c", "https://skip", true),
            device("d", "https://fail", false),
            device("e", "", false),
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
            webpush: &no_webpush(),
        };
        let pt = PushPlaintext::new("e1", 1, "h", "claude", "done", "t", "s", "p", 1);
        let r = uc.publish(&pt).unwrap();
        assert_eq!(
            r,
            PublishReport {
                delivered: 1,
                stale: 1,
                failed: 1,
                skipped: 2
            }
        );
        assert_eq!(
            publisher.0.borrow()[0],
            ("https://ok".to_string(), "a:e1".to_string())
        );
        assert!(reg.get("b").unwrap().endpoint_stale);
        assert!(!reg.get("a").unwrap().endpoint_stale);
    }

    #[test]
    fn an_endpoint_rotated_during_the_push_is_not_marked_stale() {
        let reg = MemRegistry::with(vec![device("b", "https://gone", false)]);
        // The user runs `devices set-endpoint` while the 410 is on its way.
        struct Rotating<'a>(&'a MemRegistry);
        impl PushPublisher for Rotating<'_> {
            fn post(&self, _: &str, _: &str) -> PostOutcome {
                let mut d = self.0.get("b").unwrap();
                d.endpoint = "https://new".into();
                self.0.update(d).unwrap();
                PostOutcome::Stale
            }
        }
        let uc = PublishEvent {
            registry: &reg,
            sealer: &EchoSealer,
            publisher: &Rotating(&reg),
            webpush: &no_webpush(),
        };
        let pt = PushPlaintext::new("e1", 1, "h", "claude", "done", "t", "s", "p", 1);
        uc.publish(&pt).unwrap();
        let d = reg.get("b").unwrap();
        assert_eq!(d.endpoint, "https://new");
        assert!(!d.endpoint_stale, "the new endpoint stays active");
    }

    #[test]
    fn a_webpush_only_device_is_sent_and_410_marks_only_webpush_stale() {
        let reg = MemRegistry::with(vec![
            with_webpush(device("a", "", false), "https://push.example/ok", false),
            with_webpush(device("b", "", false), "https://push.example/gone", false),
            with_webpush(device("c", "", false), "https://push.example/old", true),
            // Both transports: ntfy answers 410, web push is delivered.
            with_webpush(
                device("d", "https://gone", false),
                "https://push.example/ok",
                false,
            ),
            with_webpush(
                device("e", "https://ok", false),
                "https://push.example/fail",
                false,
            ),
        ]);
        let ntfy = ScriptedPublisher(RefCell::new(vec![]), |e| match e {
            "https://ok" => PostOutcome::Delivered,
            _ => PostOutcome::Stale,
        });
        let wp = ScriptedWebPush(RefCell::new(vec![]), |e| match e {
            "https://push.example/ok" => PostOutcome::Delivered,
            "https://push.example/gone" => PostOutcome::Stale,
            _ => PostOutcome::Failed("network"),
        });
        let uc = PublishEvent {
            registry: &reg,
            sealer: &EchoSealer,
            publisher: &ntfy,
            webpush: &wp,
        };
        let pt = PushPlaintext::new(
            "e1",
            1,
            "h",
            "claude",
            "approval_required",
            "t",
            "s",
            "p",
            1,
        );
        let r = uc.publish(&pt).unwrap();
        assert_eq!(
            r,
            PublishReport {
                delivered: 3, // a, d (web push) and e (ntfy)
                stale: 2,     // b (web push) and d (ntfy)
                failed: 1,    // e (web push)
                skipped: 1,   // c
            }
        );
        let sent: Vec<_> =
            wp.0.borrow()
                .iter()
                .map(|(e, k)| (e.clone(), k.clone()))
                .collect();
        assert_eq!(sent.len(), 4, "the stale subscription is not contacted");
        assert!(sent.iter().all(|(_, k)| k == "approval_required"));
        assert_eq!(ntfy.0.borrow().len(), 2, "empty endpoints skip only ntfy");

        let b = reg.get("b").unwrap();
        assert!(b.webpush.unwrap().stale);
        assert!(!b.endpoint_stale);
        let d = reg.get("d").unwrap();
        assert!(d.endpoint_stale, "ntfy 410 marks the endpoint");
        assert!(!d.webpush.unwrap().stale, "delivered web push stays active");
        assert!(!reg.get("a").unwrap().webpush.unwrap().stale);
    }

    #[test]
    fn a_resolved_event_skips_web_push() {
        let reg = MemRegistry::with(vec![
            with_webpush(device("a", "", false), "https://push.example/ok", false),
            with_webpush(
                device("b", "https://ok", false),
                "https://push.example/ok",
                false,
            ),
        ]);
        let ntfy = ScriptedPublisher(RefCell::new(vec![]), |_| PostOutcome::Delivered);
        let wp = ScriptedWebPush(RefCell::new(vec![]), |_| PostOutcome::Delivered);
        let uc = PublishEvent {
            registry: &reg,
            sealer: &EchoSealer,
            publisher: &ntfy,
            webpush: &wp,
        };
        let pt = PushPlaintext::new("e1", 2, "h", "claude", "resolved", "t", "s", "p", 1);
        let r = uc.publish(&pt).unwrap();
        assert!(wp.0.borrow().is_empty(), "no visible web push for resolved");
        assert_eq!(ntfy.0.borrow().len(), 1, "ntfy still updates the app");
        assert_eq!(r.delivered, 1);
        assert_eq!(r.skipped, 1);
    }
}
