//! A marked block of top-level keys appended to `~/.aider.conf.yml`:
//!
//! ```yaml
//! # >>> falconer-hook (managed: `falconer-hook integration uninstall aider` removes it)
//! notifications: true
//! notifications_command: '<exe>' emit --source aider --kind done < /dev/null
//! # <<< falconer-hook
//! ```
//!
//! Aider runs the command through a shell when it is waiting for input.
//! A `notifications` key outside our block is a conflict: we never
//! override the user's own notification setting.

use super::{agent_command, IntegrationError};

const BEGIN: &str = "# >>> falconer-hook";
const END: &str = "# <<< falconer-hook";

fn yaml_single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub fn block(exe: &str) -> String {
    let cmd = format!("{} < /dev/null", agent_command(exe, "aider", Some("done")));
    format!(
        "{BEGIN} (managed: `falconer-hook integration uninstall aider` removes it)\nnotifications: true\nnotifications_command: {}\n{END}\n",
        yaml_single_quoted(&cmd)
    )
}

/// Splits `text` into (before, our block, after). `Err` when the markers are unbalanced.
fn split(text: &str) -> Result<Option<(usize, usize)>, IntegrationError> {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let l = line.trim_end();
        if l.starts_with(BEGIN) {
            if start.is_some() {
                return Err(IntegrationError::BadHooks);
            }
            start = Some(offset);
        } else if l == END {
            let s = start.ok_or(IntegrationError::BadHooks)?;
            return Ok(Some((s, offset + line.len())));
        }
        offset += line.len();
    }
    match start {
        Some(_) => Err(IntegrationError::BadHooks),
        None => Ok(None),
    }
}

fn foreign_notifications(text: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with("notifications:") || l.starts_with("notifications_command:"))
}

/// New file contents, or `None` when nothing changes.
pub fn install(current: Option<&str>, exe: &str) -> Result<Option<String>, IntegrationError> {
    let text = current.unwrap_or("");
    let want = block(exe);
    match split(text)? {
        Some((s, e)) => {
            if foreign_notifications(&format!("{}{}", &text[..s], &text[e..])) {
                return Err(IntegrationError::Conflict);
            }
            if text[s..e] == want {
                return Ok(None);
            }
            Ok(Some(format!("{}{want}{}", &text[..s], &text[e..])))
        }
        None => {
            if foreign_notifications(text) {
                return Err(IntegrationError::Conflict);
            }
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&want);
            Ok(Some(out))
        }
    }
}

/// Contents without our block, or `None` when it is absent.
pub fn uninstall(current: &str) -> Result<Option<String>, IntegrationError> {
    Ok(split(current)?.map(|(s, e)| format!("{}{}", &current[..s], &current[e..])))
}

pub fn installed(current: &str) -> Result<bool, IntegrationError> {
    Ok(split(current)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/home/u/.local/bin/falconer-hook";

    #[test]
    fn install_appends_once_and_uninstall_restores_bytes() {
        let original = "model: sonnet\n# keep me\ndark-mode: true\n";
        let installed_text = install(Some(original), EXE).unwrap().unwrap();
        assert!(installed_text.starts_with(original));
        assert!(installed_text.contains(
            "notifications_command: '''/home/u/.local/bin/falconer-hook'' emit --source aider --kind done < /dev/null'"
        ));
        assert_eq!(install(Some(&installed_text), EXE).unwrap(), None);
        assert!(installed(&installed_text).unwrap());
        assert_eq!(
            uninstall(&installed_text).unwrap().as_deref(),
            Some(original)
        );
        assert_eq!(uninstall(original).unwrap(), None);
    }

    #[test]
    fn repoints_and_refuses_conflicts_and_broken_markers() {
        let first = install(None, EXE).unwrap().unwrap();
        let moved = install(Some(&first), "/new/falconer-hook")
            .unwrap()
            .unwrap();
        assert!(moved.contains("/new/falconer-hook") && !moved.contains(EXE));
        assert_eq!(
            install(Some("notifications: false\n"), EXE),
            Err(IntegrationError::Conflict)
        );
        assert_eq!(
            install(Some("# >>> falconer-hook\nnotifications: true\n"), EXE),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            uninstall("# <<< falconer-hook\n"),
            Err(IntegrationError::BadHooks)
        );
    }
}
