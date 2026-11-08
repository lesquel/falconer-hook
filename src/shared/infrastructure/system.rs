//! System clock and OS-random id generator.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::shared::domain::clock::Clock;
use crate::shared::domain::encoding::b64u_encode;
use crate::shared::domain::ids::IdGenerator;
use crate::shared::infrastructure::crypto::random_bytes;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// 16 random bytes from the OS CSPRNG, b64u encoded.
pub struct OsIds;

impl IdGenerator for OsIds {
    fn new_id(&self) -> String {
        b64u_encode(random_bytes::<16>().as_slice())
    }
}

/// Short host name: `/proc/sys/kernel/hostname` on Linux, else `hostname -s`.
pub fn hostname(runner: &dyn crate::shared::domain::command::CommandRunner) -> String {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| {
            runner
                .run("hostname", &["-s".to_string()])
                .ok()
                .filter(|o| o.success())
                .map(|o| o.stdout)
        });
    let clean: String = raw
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    if clean.is_empty() {
        "host".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::ids::is_valid_id;

    #[test]
    fn os_ids_are_valid_and_distinct() {
        let a = OsIds.new_id();
        let b = OsIds.new_id();
        assert!(is_valid_id(&a));
        assert_ne!(a, b);
    }
}

/// Whether an executable file named `name` is in one of the `PATH` directories.
pub fn on_path(name: &str, path_env: Option<&str>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path_env.unwrap_or("").split(':').any(|dir| {
        !dir.is_empty()
            && std::fs::metadata(std::path::Path::new(dir).join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod path_tests {
    use super::on_path;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn finds_only_executable_files() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let path = format!("/nonexistent:{}", t.path().display());
        assert!(!on_path("tool", Some(&path)), "not executable yet");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(on_path("tool", Some(&path)));
        assert!(!on_path("other", Some(&path)));
        assert!(!on_path("tool", None));
    }
}
