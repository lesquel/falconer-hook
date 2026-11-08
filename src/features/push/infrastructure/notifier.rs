//! `EventNotifier` over the publish use case: turns an inbox event into an
//! encrypted push for every paired device.

use crate::features::events::domain::{Event, EventNotifier};
use crate::features::pairing::domain::DeviceRegistry;
use crate::features::push::application::PublishEvent;
use crate::features::push::domain::{PushPlaintext, PushPublisher, Sealer};
use crate::shared::infrastructure::log::{log, Field, Level};

pub struct PushNotifier<'a, R, S, P> {
    pub publish: PublishEvent<'a, R, S, P>,
    pub host_alias: String,
}

pub fn plaintext_for(event: &Event, host: &str, resolved: bool) -> PushPlaintext {
    PushPlaintext::new(
        &event.id,
        event.seq,
        host,
        &event.agent,
        if resolved {
            "resolved"
        } else {
            event.kind.as_str()
        },
        &event.title,
        &event.summary,
        event.pane.as_deref().unwrap_or_default(),
        event
            .resolved_at
            .filter(|_| resolved)
            .unwrap_or(event.created_at),
    )
}

impl<R: DeviceRegistry, S: Sealer, P: PushPublisher> PushNotifier<'_, R, S, P> {
    fn send(&self, pt: &PushPlaintext) -> u32 {
        match self.publish.publish(pt) {
            Ok(r) => r.delivered,
            Err(_) => {
                log(
                    Level::Warn,
                    "push skipped",
                    &[("reason", Field::Code("registry_unreadable"))],
                );
                0
            }
        }
    }
}

impl<R: DeviceRegistry, S: Sealer, P: PushPublisher> EventNotifier for PushNotifier<'_, R, S, P> {
    fn notify(&self, event: &Event) -> u32 {
        self.send(&plaintext_for(event, &self.host_alias, false))
    }
    fn notify_resolved(&self, event: &Event) -> u32 {
        self.send(&plaintext_for(event, &self.host_alias, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::events::domain::{EventKind, EventStatus, Runtime};

    #[test]
    fn resolved_push_keeps_id_and_marks_kind() {
        let e = Event {
            id: "e1".into(),
            seq: 9,
            source: "herdr".into(),
            agent: "claude".into(),
            kind: EventKind::ApprovalRequired,
            status: EventStatus::Resolved,
            title: "t".into(),
            summary: "s".into(),
            pane: Some("w1:p1".into()),
            runtime: Some(Runtime::Herdr),
            created_at: 1,
            resolved_at: Some(5),
            resolution: Some("allow".into()),
        };
        let p = plaintext_for(&e, "box", true);
        assert_eq!(
            (p.id.as_str(), p.kind.as_str(), p.seq, p.ts, p.pane.as_str()),
            ("e1", "resolved", 9, 5, "w1:p1")
        );
        let p = plaintext_for(&e, "box", false);
        assert_eq!((p.kind.as_str(), p.ts), ("approval_required", 1));
    }
}
