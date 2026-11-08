//! Whole files created and removed by openmosh-hook: a Copilot CLI hook
//! file, a Kiro CLI global hook file and an OpenCode plugin. A file that
//! exists without our marker is never overwritten or deleted.

use serde_json::json;

use super::{agent_command, IntegrationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedKind {
    Copilot,
    Kiro,
    Opencode,
}

/// First line of the OpenCode plugin; identifies the file as ours.
const JS_HEADER: &str =
    "// openmosh-hook integration: managed by `openmosh-hook integration install opencode`.";

fn agent(kind: OwnedKind) -> &'static str {
    match kind {
        OwnedKind::Copilot => "copilot",
        OwnedKind::Kiro => "kiro",
        OwnedKind::Opencode => "opencode",
    }
}

fn pretty(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

/// File contents for `exe`.
pub fn render(kind: OwnedKind, exe: &str) -> String {
    let a = agent(kind);
    match kind {
        OwnedKind::Copilot => {
            let hook = |k: &str| json!({"type": "command", "bash": agent_command(exe, a, Some(k)), "timeoutSec": 10});
            pretty(&json!({
                "version": 1,
                "hooks": {
                    "notification": [hook("notification")],
                    "agentStop": [hook("done")],
                }
            }))
        }
        OwnedKind::Kiro => pretty(&json!({
            "version": "v1",
            "hooks": [{
                "name": "openmosh: notify the phone when the agent stops",
                "trigger": "Stop",
                "action": {"type": "command", "command": agent_command(exe, a, Some("done"))},
                "timeout": 10,
            }]
        })),
        OwnedKind::Opencode => {
            // A JSON string literal is a valid JS string literal.
            let exe = serde_json::to_string(exe).unwrap_or_default();
            format!(
                r#"{JS_HEADER}
// Remove it with `openmosh-hook integration uninstall opencode`; edits here are overwritten.
// Runs `openmosh-hook emit` (argv only, no shell) when OpenCode needs you or finishes.
import {{ spawn }} from "node:child_process";

const EXE = {exe};
const KINDS = {{
  "permission.asked": "approval_required",
  "question.asked": "question",
  "session.idle": "done",
}};

function emit(kind, event) {{
  try {{
    const child = spawn(EXE, ["emit", "--source", "opencode", "--kind", kind], {{
      stdio: ["pipe", "ignore", "ignore"],
    }});
    child.on("error", () => {{}});
    child.stdin.on("error", () => {{}});
    const title = event?.properties?.title;
    child.stdin.end(JSON.stringify({{ message: typeof title === "string" ? title : "" }}));
  }} catch {{}}
}}

export const OpenMoshPlugin = async () => ({{
  event: async ({{ event }}) => {{
    const kind = KINDS[event?.type];
    if (kind) emit(kind, event);
  }},
}});
"#
            )
        }
    }
}

/// Whether `text` is a file we wrote.
pub fn is_ours(kind: OwnedKind, text: &str) -> bool {
    match kind {
        OwnedKind::Opencode => text.starts_with(JS_HEADER),
        _ => {
            text.contains("openmosh-hook")
                && text.contains(&format!(" emit --source {}", agent(kind)))
        }
    }
}

/// What to do on install given the current file contents.
pub fn plan_install(
    kind: OwnedKind,
    current: Option<&str>,
    exe: &str,
) -> Result<Option<String>, IntegrationError> {
    let want = render(kind, exe);
    match current {
        None => Ok(Some(want)),
        Some(t) if t == want => Ok(None),
        Some(t) if is_ours(kind, t) => Ok(Some(want)),
        Some(_) => Err(IntegrationError::Conflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/home/u/.local/bin/openmosh-hook";

    #[test]
    fn rendered_files_are_ours_and_valid() {
        for k in [OwnedKind::Copilot, OwnedKind::Kiro, OwnedKind::Opencode] {
            let t = render(k, EXE);
            assert!(is_ours(k, &t), "{k:?}");
            if k != OwnedKind::Opencode {
                serde_json::from_str::<serde_json::Value>(&t).unwrap();
            }
        }
        let c: serde_json::Value = serde_json::from_str(&render(OwnedKind::Copilot, EXE)).unwrap();
        assert_eq!(
            c["hooks"]["agentStop"][0]["bash"],
            "'/home/u/.local/bin/openmosh-hook' emit --source copilot --kind done"
        );
        let k: serde_json::Value = serde_json::from_str(&render(OwnedKind::Kiro, EXE)).unwrap();
        assert_eq!(k["hooks"][0]["trigger"], "Stop");
        let js = render(OwnedKind::Opencode, "/opt/a\"b/openmosh-hook");
        assert!(js.contains(r#"const EXE = "/opt/a\"b/openmosh-hook";"#));
    }

    #[test]
    fn install_plan_is_idempotent_repoints_and_refuses_foreign_files() {
        let first = plan_install(OwnedKind::Kiro, None, EXE).unwrap().unwrap();
        assert_eq!(
            plan_install(OwnedKind::Kiro, Some(&first), EXE).unwrap(),
            None
        );
        let moved = plan_install(OwnedKind::Kiro, Some(&first), "/new/openmosh-hook")
            .unwrap()
            .unwrap();
        assert!(moved.contains("/new/openmosh-hook"));
        assert_eq!(
            plan_install(
                OwnedKind::Kiro,
                Some("{\"version\":\"v1\",\"hooks\":[]}"),
                EXE
            ),
            Err(IntegrationError::Conflict)
        );
        assert_eq!(
            plan_install(OwnedKind::Opencode, Some("export const Mine = 1;\n"), EXE),
            Err(IntegrationError::Conflict)
        );
    }
}
