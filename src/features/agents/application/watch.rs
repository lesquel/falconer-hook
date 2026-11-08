//! One polling tick of the daemon: list agents, detect transitions, signal.

use crate::features::agents::domain::{AgentError, AgentRuntime, Transition, TransitionDetector};

/// Receives transitions; implemented in the composition root (records the
/// event and publishes a push).
pub trait SignalSink {
    fn signal(&mut self, transition: Transition);
}

pub struct WatchAgents<'a> {
    pub runtime: &'a dyn AgentRuntime,
    pub detector: TransitionDetector,
}

impl WatchAgents<'_> {
    /// Returns how many transitions were signalled.
    pub fn tick(&mut self, sink: &mut dyn SignalSink) -> Result<usize, AgentError> {
        let agents = self.runtime.list_agents()?;
        let transitions = self.detector.observe(&agents);
        let n = transitions.len();
        for t in transitions {
            sink.signal(t);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agents::domain::fakes::FakeRuntime;
    use crate::features::agents::domain::AgentStatus;

    struct Collect(Vec<Transition>);
    impl SignalSink for Collect {
        fn signal(&mut self, t: Transition) {
            self.0.push(t);
        }
    }

    #[test]
    fn tick_signals_transitions_after_priming() {
        let rt = FakeRuntime::herdr_with_agents(1);
        let mut w = WatchAgents {
            runtime: &rt,
            detector: TransitionDetector::default(),
        };
        let mut sink = Collect(vec![]);
        assert_eq!(w.tick(&mut sink).unwrap(), 0);
        rt.set_status(0, AgentStatus::Blocked, 2);
        assert_eq!(w.tick(&mut sink).unwrap(), 1);
        assert!(matches!(sink.0[0], Transition::Blocked(_)));
    }
}
