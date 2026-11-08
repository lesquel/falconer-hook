use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use crate::features::mirror::domain::FocusKind::{Pane, Tab, Workspace};

#[derive(Default)]
struct Shared {
    now: Cell<u64>,
    log: RefCell<Vec<String>>,
}

impl Shared {
    fn push(&self, s: String) {
        self.log.borrow_mut().push(s);
    }
}

enum Step {
    Event(SocketEvent),
    /// No event: the read times out after the requested wait.
    Idle,
    Fail(MirrorError),
}

struct FakeClock(Rc<Shared>);

impl MonoClock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.now.get()
    }
    fn sleep(&self, d: Duration) {
        self.0.now.set(self.0.now.get() + d.as_millis() as u64);
    }
}

/// Scripted socket. When the script runs out it raises `stop`, like a child exiting.
struct FakeSocket {
    shared: Rc<Shared>,
    steps: VecDeque<Step>,
    connects: VecDeque<Result<(), MirrorError>>,
    snapshots: VecDeque<FocusSnapshot>,
    /// Raise `stop` after this many connect attempts (for unreachable sockets).
    stop_after_connects: Option<usize>,
    attempts: usize,
    stop: Arc<AtomicBool>,
}

impl FakeSocket {
    fn new(shared: &Rc<Shared>, stop: &Arc<AtomicBool>) -> Self {
        Self {
            shared: shared.clone(),
            steps: VecDeque::new(),
            connects: VecDeque::new(),
            snapshots: VecDeque::new(),
            stop_after_connects: None,
            attempts: 0,
            stop: stop.clone(),
        }
    }
    fn snap(mut self, tab: &str) -> Self {
        self.snapshots.push_back(FocusSnapshot {
            workspace: Some("w1".into()),
            tab: Some(tab.into()),
            pane: Some("w1:p1".into()),
        });
        self
    }
    fn ev(mut self, kind: FocusKind, id: &str) -> Self {
        self.steps.push_back(Step::Event(SocketEvent::Focused(
            FocusTarget::new(kind, id).unwrap(),
        )));
        self
    }
    fn step(mut self, s: Step) -> Self {
        self.steps.push_back(s);
        self
    }
}

impl FocusSocket for FakeSocket {
    fn connect(&mut self) -> Result<(), MirrorError> {
        self.attempts += 1;
        self.shared
            .push(format!("connect @{}", self.shared.now.get()));
        if Some(self.attempts) == self.stop_after_connects {
            self.stop.store(true, Ordering::SeqCst);
        }
        self.connects.pop_front().unwrap_or(Ok(()))
    }
    fn next_event(&mut self, wait: Duration) -> Result<Option<SocketEvent>, MirrorError> {
        match self.steps.pop_front() {
            Some(Step::Event(e)) => Ok(Some(e)),
            Some(Step::Fail(e)) => Err(e),
            Some(Step::Idle) => {
                self.shared
                    .now
                    .set(self.shared.now.get() + wait.as_millis() as u64);
                Ok(None)
            }
            None => {
                self.stop.store(true, Ordering::SeqCst);
                Ok(None)
            }
        }
    }
    fn snapshot(&mut self) -> Result<FocusSnapshot, MirrorError> {
        self.shared.push("snapshot".into());
        Ok(self.snapshots.pop_front().unwrap_or_default())
    }
    fn focus(&mut self, t: &FocusTarget) -> Result<(), MirrorError> {
        self.shared.push(format!(
            "focus {:?} {} @{}",
            t.kind,
            t.id,
            self.shared.now.get()
        ));
        Ok(())
    }
}

fn run(build: impl FnOnce(FakeSocket) -> FakeSocket) -> (MirrorReport, Vec<String>) {
    let shared = Rc::new(Shared::default());
    let stop = Arc::new(AtomicBool::new(false));
    let socket = build(FakeSocket::new(&shared, &stop));
    let mut lp = MirrorLoop::new(socket, FakeClock(shared.clone()));
    let report = lp.run(&stop);
    let log = shared.log.borrow().clone();
    (report, log)
}

fn focuses(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|l| l.starts_with("focus"))
        .cloned()
        .collect()
}

#[test]
fn applies_focus_for_a_new_target() {
    let (report, log) = run(|s| s.snap("w1:t1").ev(Tab, "w1:t2"));
    assert_eq!(log, ["connect @0", "snapshot", "focus Tab w1:t2 @0"]);
    assert!(report.connected);
    assert_eq!(report.applied, 1);
}

#[test]
fn ignores_the_current_target_and_its_own_echo() {
    let (report, log) = run(|s| {
        s.snap("w1:t1")
            .ev(Tab, "w1:t1") // already focused (snapshot baseline)
            .ev(Tab, "w1:t2")
            .step(Step::Idle)
            .ev(Tab, "w1:t2") // echo of our own tab.focus
            .step(Step::Idle)
            .step(Step::Idle)
    });
    assert_eq!(focuses(&log), ["focus Tab w1:t2 @0"]);
    assert_eq!(report.applied, 1);
}

#[test]
fn debounces_and_coalesces_rapid_changes() {
    let (_, log) = run(|s| {
        s.snap("w1:t1")
            .ev(Tab, "w1:t2")
            .ev(Tab, "w1:t3")
            .ev(Tab, "w1:t4")
            .step(Step::Idle)
            .step(Step::Idle)
            .step(Step::Idle)
    });
    // t2 at once, t3 dropped, t4 only when the 150 ms window ends.
    assert_eq!(
        focuses(&log),
        [
            "focus Tab w1:t2 @0",
            &format!("focus Tab w1:t4 @{DEBOUNCE_MS}")
        ]
    );
}

#[test]
fn returning_to_the_applied_target_cancels_a_pending_one() {
    let (_, log) = run(|s| {
        s.snap("w1:t1")
            .ev(Tab, "w1:t2")
            .ev(Tab, "w1:t3")
            .ev(Tab, "w1:t2")
            .step(Step::Idle)
            .step(Step::Idle)
    });
    assert_eq!(focuses(&log), ["focus Tab w1:t2 @0"]);
}

#[test]
fn kinds_are_independent_and_applied_in_order() {
    let (_, log) = run(|s| {
        s.snap("w1:t1")
            .ev(Pane, "w2:p4")
            .ev(Workspace, "w2")
            .ev(Tab, "w2:t1")
    });
    assert_eq!(
        focuses(&log),
        [
            "focus Pane w2:p4 @0",
            "focus Workspace w2 @0",
            "focus Tab w2:t1 @0"
        ]
    );
}

#[test]
fn resyncs_from_a_snapshot_after_events_lost() {
    let (report, log) = run(|s| {
        s.snap("w1:t1")
            .snap("w1:t5")
            .step(Step::Event(SocketEvent::EventsLost))
            .ev(Tab, "w1:t5") // current after resync: not re-applied
            .ev(Tab, "w1:t6")
    });
    assert_eq!(
        log,
        [
            "connect @0",
            "snapshot",
            "connect @0",
            "snapshot",
            "focus Tab w1:t6 @0"
        ]
    );
    assert_eq!(report.resyncs, 1);
}

#[test]
fn reconnects_with_backoff_after_a_socket_error() {
    let (report, log) = run(|s| {
        s.snap("w1:t1")
            .snap("w1:t1")
            .step(Step::Fail(MirrorError::Closed))
            .ev(Tab, "w1:t2")
    });
    assert_eq!(
        log,
        [
            "connect @0",
            "snapshot",
            "connect @500",
            "snapshot",
            "focus Tab w1:t2 @500"
        ]
    );
    assert_eq!(report.resyncs, 1);
}

#[test]
fn unreachable_socket_leaves_the_mirror_off() {
    let (report, log) = run(|mut s| {
        s.connects = (0..3).map(|_| Err(MirrorError::Unavailable)).collect();
        s.stop_after_connects = Some(3);
        s
    });
    assert_eq!(log, ["connect @0", "connect @500", "connect @1000"]);
    assert!(!report.connected);
    assert_eq!(report.applied, 0);
}

struct FakeChild(i32);

impl ChildProcess for FakeChild {
    fn wait(&mut self) -> Result<i32, MirrorError> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(self.0)
    }
}

#[derive(Default)]
struct FakeLauncher {
    argv: RefCell<Vec<String>>,
    fail: bool,
}

impl Launcher for FakeLauncher {
    type Child = FakeChild;
    fn spawn(&self, program: &str, args: &[String]) -> Result<FakeChild, MirrorError> {
        let mut argv = vec![program.to_string()];
        argv.extend(args.iter().cloned());
        *self.argv.borrow_mut() = argv;
        if self.fail {
            return Err(MirrorError::Launch(program.into()));
        }
        Ok(FakeChild(3))
    }
}

#[test]
fn attach_stops_the_mirror_when_the_child_exits_and_returns_its_code() {
    let launcher = FakeLauncher::default();
    let args = vec!["session".to_string(), "attach".into(), "work".into()];
    let (code, report) = attach(&launcher, "herdr", &args, |stop| {
        let mut ticks = 0;
        while !stop.load(Ordering::SeqCst) {
            ticks += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        MirrorReport {
            connected: true,
            applied: ticks.min(1),
            resyncs: 0,
        }
    })
    .unwrap();
    assert_eq!(code, 3);
    assert!(report.connected);
    assert_eq!(
        *launcher.argv.borrow(),
        ["herdr", "session", "attach", "work"]
    );
}

#[test]
fn attach_does_not_start_the_mirror_when_the_child_cannot_start() {
    let launcher = FakeLauncher {
        fail: true,
        ..Default::default()
    };
    let started = AtomicBool::new(false);
    let r = attach(&launcher, "herdr", &[], |_| {
        started.store(true, Ordering::SeqCst);
        MirrorReport::default()
    });
    assert!(matches!(r, Err(MirrorError::Launch(_))));
    assert!(!started.load(Ordering::SeqCst));
}

#[test]
fn follows_tab_changes_seen_only_in_the_snapshot() {
    // Herdr 0.9 emits no tab.focused event when a client switches tabs; the
    // session snapshot still moves to the last-navigated client's tab.
    let mut steps = Vec::new();
    for _ in 0..12 {
        steps.push(Step::Idle);
    }
    let (report, log) = run(|s| {
        let mut s = s.snap("w1:t1").snap("w1:t2");
        for st in steps {
            s = s.step(st);
        }
        s
    });
    let f = focuses(&log);
    assert_eq!(f.len(), 1, "{log:?}");
    let at: u64 = f[0].rsplit('@').next().unwrap().parse().unwrap();
    assert!(f[0].starts_with("focus Tab w1:t2 @"));
    assert!(
        (SNAPSHOT_POLL_MS..SNAPSHOT_POLL_MS + 60).contains(&at),
        "{at}"
    );
    assert_eq!(report.applied, 1);
}

#[test]
fn a_snapshot_matching_what_was_applied_sends_nothing() {
    let (_, log) = run(|s| {
        let mut s = s.snap("w1:t1").snap("w1:t1").snap("w1:t1");
        for _ in 0..12 {
            s = s.step(Step::Idle);
        }
        s
    });
    assert!(focuses(&log).is_empty(), "{log:?}");
}
