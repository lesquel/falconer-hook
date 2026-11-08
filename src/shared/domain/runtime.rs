use serde::Serialize;

/// Terminal multiplexer that owns an agent pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Herdr,
    Tmux,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Herdr => "herdr",
            Runtime::Tmux => "tmux",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "herdr" => Some(Runtime::Herdr),
            "tmux" => Some(Runtime::Tmux),
            _ => None,
        }
    }
}
