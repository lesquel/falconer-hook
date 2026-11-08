//! `serve`: poll Herdr, record transitions, publish pushes.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::{open_state, with_notifier, Ctx};
use crate::cli::output::{CliError, Reply};
use crate::features::agents::application::{SignalSink, WatchAgents};
use crate::features::agents::domain::{Agent, Transition, TransitionDetector};
use crate::features::agents::infrastructure::herdr::HerdrRuntime;
use crate::features::events::application::{ClearPane, IngestEvent, IngestOutcome};
use crate::features::events::domain::{
    retention_cutoff, EventKind, EventNotifier, EventStore, NewEvent, PaneRef, Runtime,
};
use crate::features::events::infrastructure::sqlite_store::SqliteEventStore;
use crate::shared::domain::clock::Clock;
use crate::shared::domain::error::ErrorCode;
use crate::shared::infrastructure::log::{log, Field, Level};
use crate::shared::infrastructure::system::{OsIds, SystemClock};

/// Maps a Herdr transition to an inbox event.
pub fn event_for(agent: &Agent, kind: EventKind, now: u64) -> NewEvent {
    let title = match kind {
        EventKind::ApprovalRequired => format!("{} needs approval", agent.agent),
        EventKind::Question => format!("{} is waiting for input", agent.agent),
        EventKind::Done => format!("{} finished", agent.agent),
    };
    NewEvent {
        source: "herdr".into(),
        agent: agent.agent.clone(),
        kind,
        title,
        summary: agent.title.clone(),
        pane: Some(PaneRef {
            runtime: Runtime::Herdr,
            pane: agent.pane_id.clone(),
        }),
        created_at: now,
    }
}

struct Sink<'a> {
    ingest: IngestEvent<'a, SqliteEventStore, OsIds, SystemClock>,
    clear: ClearPane<'a, SqliteEventStore, SystemClock>,
    notifier: &'a dyn EventNotifier,
}

impl Sink<'_> {
    fn record(&self, new: NewEvent) {
        match self.ingest.ingest(new) {
            Ok(IngestOutcome::Recorded { event, superseded }) => {
                for old in &superseded {
                    self.notifier.notify_resolved(old);
                }
                let n = self.notifier.notify(&event);
                log(
                    Level::Info,
                    "event recorded",
                    &[
                        ("kind", Field::Code(event.kind.as_str())),
                        ("pushed", Field::Num(n.into())),
                    ],
                );
            }
            Ok(IngestOutcome::Duplicate) => {}
            Err(e) => log(
                Level::Error,
                "event store failed",
                &[("reason", Field::Code(e.code()))],
            ),
        }
    }
}

impl SignalSink for Sink<'_> {
    fn signal(&mut self, t: Transition) {
        let now = SystemClock.now();
        match t {
            Transition::Blocked(a) => self.record(event_for(&a, EventKind::ApprovalRequired, now)),
            Transition::Done(a) => self.record(event_for(&a, EventKind::Done, now)),
            Transition::Unblocked(a) => match self.clear.clear(&a.pane_id) {
                Ok(cleared) => {
                    for e in &cleared {
                        self.notifier.notify_resolved(e);
                    }
                }
                Err(e) => log(
                    Level::Error,
                    "event store failed",
                    &[("reason", Field::Code(e.code()))],
                ),
            },
        }
    }
}

pub fn serve(ctx: &Ctx) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let st = open_state(ctx)?;
    let herdr = HerdrRuntime {
        runner: &ctx.runner,
        program: cfg.bins.herdr.clone(),
    };
    let poll = Duration::from_millis(cfg.poll_interval_ms);
    with_notifier(ctx, &cfg, &st, |notifier| {
        let mut sink = Sink {
            ingest: IngestEvent {
                store: &st.store,
                ids: &OsIds,
                clock: &SystemClock,
                retention_days: cfg.retention_days,
            },
            clear: ClearPane {
                store: &st.store,
                clock: &SystemClock,
            },
            notifier,
        };
        let mut watch = cfg.source_enabled("herdr").then(|| WatchAgents {
            runtime: &herdr,
            detector: TransitionDetector::default(),
        });
        log(
            Level::Info,
            "serve started",
            &[
                ("poll_ms", Field::Num(cfg.poll_interval_ms)),
                (
                    "herdr",
                    Field::Code(if watch.is_some() { "on" } else { "off" }),
                ),
            ],
        );
        let mut last_err: Option<&'static str> = None;
        let mut last_purge = Instant::now() - Duration::from_secs(3600);
        while !ctx.stop.load(Ordering::SeqCst) {
            if last_purge.elapsed() >= Duration::from_secs(3600) {
                let _ = st
                    .store
                    .purge_before(retention_cutoff(SystemClock.now(), cfg.retention_days));
                last_purge = Instant::now();
            }
            if let Some(w) = watch.as_mut() {
                match w.tick(&mut sink) {
                    Ok(_) if last_err.take().is_some() => {
                        log(Level::Info, "herdr reachable again", &[])
                    }
                    Ok(_) => {}
                    Err(e) if last_err != Some(e.code()) => {
                        last_err = Some(e.code());
                        log(
                            Level::Warn,
                            "herdr poll failed",
                            &[("reason", Field::Code(e.code()))],
                        );
                    }
                    Err(_) => {}
                }
            }
            let start = Instant::now();
            while start.elapsed() < poll && !ctx.stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        log(Level::Info, "serve stopped", &[]);
        Ok(Reply::new(&serde_json::json!({"stopped": true}), ""))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agents::domain::{AgentStatus, Runtime as R};

    #[test]
    fn herdr_transition_maps_to_event() {
        let a = Agent {
            pane_id: "w7:pA".into(),
            agent: "claude".into(),
            status: AgentStatus::Blocked,
            state_change_seq: 1,
            cwd: "/w".into(),
            title: "Fix tests".into(),
            workspace_id: None,
            runtime: R::Herdr,
        };
        let e = event_for(&a, EventKind::ApprovalRequired, 9);
        assert_eq!(e.title, "claude needs approval");
        assert_eq!(e.summary, "Fix tests");
        assert_eq!(
            e.pane,
            Some(PaneRef {
                runtime: Runtime::Herdr,
                pane: "w7:pA".into()
            })
        );
    }
}
