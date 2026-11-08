//! Mirror use cases: the focus loop and `attach` (child + mirror thread).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::features::mirror::domain::ChildProcess;
use crate::features::mirror::domain::{
    FocusKind, FocusSnapshot, FocusSocket, FocusTarget, Launcher, MirrorError, MonoClock,
    SocketEvent,
};

/// Minimum time between two focus requests of the same kind. Newer targets
/// seen inside the window are coalesced and the latest is applied when it ends.
pub const DEBOUNCE_MS: u64 = 150;

/// Pure focus bookkeeping: what was applied last, what is waiting, and when.
#[derive(Debug, Default)]
pub struct FocusState {
    applied: [Option<String>; 3],
    pending: [Option<String>; 3],
    last_apply_ms: [Option<u64>; 3],
}

impl FocusState {
    /// Takes the server's current focus as the new baseline (after connect/resync).
    pub fn reset(&mut self, snap: &FocusSnapshot) {
        for kind in FocusKind::ALL {
            let i = kind.index();
            self.applied[i] = snap.get(kind).map(str::to_string);
            self.pending[i] = None;
            self.last_apply_ms[i] = None;
        }
    }

    /// Records a focus event. An event equal to the last applied target is our
    /// own echo (or a return to it) and cancels anything pending.
    pub fn observe(&mut self, target: FocusTarget) {
        let i = target.kind.index();
        if self.applied[i].as_deref() == Some(target.id.as_str()) {
            self.pending[i] = None;
        } else {
            self.pending[i] = Some(target.id);
        }
    }

    /// Targets to focus now, in workspace → tab → pane order.
    pub fn due(&mut self, now_ms: u64) -> Vec<FocusTarget> {
        let mut out = Vec::new();
        for kind in FocusKind::ALL {
            let i = kind.index();
            if self.pending[i].is_none() || !self.window_open(i, now_ms) {
                continue;
            }
            if let Some(id) = self.pending[i].take() {
                self.applied[i] = Some(id.clone());
                self.last_apply_ms[i] = Some(now_ms);
                out.push(FocusTarget { kind, id });
            }
        }
        out
    }

    /// How long to wait for the next event before a pending target is due.
    pub fn wait(&self, now_ms: u64, poll: Duration) -> Duration {
        let mut wait = poll;
        for kind in FocusKind::ALL {
            let i = kind.index();
            if self.pending[i].is_some() {
                let due_at = self.last_apply_ms[i].map_or(now_ms, |t| t + DEBOUNCE_MS);
                wait = wait.min(Duration::from_millis(due_at.saturating_sub(now_ms)));
            }
        }
        wait.max(Duration::from_millis(1))
    }

    fn window_open(&self, i: usize, now_ms: u64) -> bool {
        self.last_apply_ms[i].is_none_or(|t| now_ms.saturating_sub(t) >= DEBOUNCE_MS)
    }
}

/// What the mirror did before it stopped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MirrorReport {
    /// Whether the socket was reachable at least once.
    pub connected: bool,
    /// Focus requests sent.
    pub applied: u64,
    /// Reconnects after `events_lost` or a socket error.
    pub resyncs: u64,
}

/// Follows focus events and re-applies them so every attached client follows.
pub struct MirrorLoop<S, C> {
    pub socket: S,
    pub clock: C,
    pub poll: Duration,
    pub retry_ms: u64,
    state: FocusState,
}

impl<S: FocusSocket, C: MonoClock> MirrorLoop<S, C> {
    pub fn new(socket: S, clock: C) -> Self {
        Self {
            socket,
            clock,
            poll: Duration::from_millis(50),
            retry_ms: 500,
            state: FocusState::default(),
        }
    }

    /// Runs until `stop` is set. Socket errors never escape: the loop
    /// reconnects (with `retry_ms` backoff) and resyncs from a snapshot.
    pub fn run(&mut self, stop: &AtomicBool) -> MirrorReport {
        let mut report = MirrorReport::default();
        let mut connected = false;
        let mut next_try = self.clock.now_ms();
        while !stop.load(Ordering::SeqCst) {
            if !connected {
                let now = self.clock.now_ms();
                if now < next_try {
                    self.clock
                        .sleep(self.poll.min(Duration::from_millis(next_try - now)));
                    continue;
                }
                match self.connect() {
                    Ok(snap) => {
                        self.state.reset(&snap);
                        if report.connected {
                            report.resyncs += 1;
                        }
                        report.connected = true;
                        connected = true;
                    }
                    Err(_) => {
                        next_try = now + self.retry_ms;
                        continue;
                    }
                }
            }
            let wait = self.state.wait(self.clock.now_ms(), self.poll);
            match self.socket.next_event(wait) {
                Ok(Some(SocketEvent::Focused(t))) => self.state.observe(t),
                Ok(Some(SocketEvent::EventsLost)) => {
                    connected = false;
                    next_try = self.clock.now_ms();
                    continue;
                }
                Ok(None) => {}
                Err(_) => {
                    connected = false;
                    next_try = self.clock.now_ms() + self.retry_ms;
                    continue;
                }
            }
            for target in self.state.due(self.clock.now_ms()) {
                if self.socket.focus(&target).is_err() {
                    connected = false;
                    next_try = self.clock.now_ms() + self.retry_ms;
                    break;
                }
                report.applied += 1;
            }
        }
        report
    }

    fn connect(&mut self) -> Result<FocusSnapshot, MirrorError> {
        self.socket.connect()?;
        self.socket.snapshot()
    }
}

/// Runs `program` with the inherited terminal while `mirror` runs on another
/// thread; stops the mirror when the child exits and returns its exit code.
pub fn attach<L, F>(
    launcher: &L,
    program: &str,
    args: &[String],
    mirror: F,
) -> Result<(i32, MirrorReport), MirrorError>
where
    L: Launcher,
    F: FnOnce(&AtomicBool) -> MirrorReport + Send,
{
    let mut child = launcher.spawn(program, args)?;
    let stop = AtomicBool::new(false);
    std::thread::scope(|s| {
        let handle = s.spawn(|| mirror(&stop));
        let code = child.wait();
        stop.store(true, Ordering::SeqCst);
        let report = handle.join().unwrap_or_default();
        code.map(|c| (c, report))
    })
}

#[cfg(test)]
mod tests;
