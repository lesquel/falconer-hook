//! Filesystem and service checks.

use std::path::Path;

use crate::features::doctor::domain::Check;
use crate::shared::domain::command::CommandRunner;
use crate::shared::infrastructure::fs_secure::{check_private, FsError};
use crate::shared::infrastructure::ssh_fingerprint::fingerprint;

/// Directories must be `0700` and files `0600` or stricter (control T-8).
pub fn check_permissions(dirs: &[&Path], files: &[&Path]) -> Check {
    let mut problems = Vec::new();
    for (p, mode) in dirs
        .iter()
        .map(|d| (d, 0o700))
        .chain(files.iter().map(|f| (f, 0o600)))
    {
        if !p.exists() {
            continue;
        }
        if let Err(FsError::TooOpen { path, mode: m, .. }) = check_private(p, mode) {
            problems.push(format!("{path} is {m:04o}"));
        }
    }
    if problems.is_empty() {
        Check::ok("permissions", "state and config are private")
    } else {
        Check::fail(
            "permissions",
            format!(
                "too open: {} (chmod 700 dirs, 600 files)",
                problems.join(", ")
            ),
        )
    }
}

pub fn check_ssh(host_key_pub: &Path) -> Check {
    match std::fs::read_to_string(host_key_pub)
        .ok()
        .as_deref()
        .and_then(fingerprint)
    {
        Some(fp) => Check::ok("sshd", format!("host key {fp}")),
        None => Check::fail(
            "sshd",
            format!(
                "{} not readable; install and enable OpenSSH server",
                host_key_pub.display()
            ),
        ),
    }
}

/// Read-only: unit file present and `systemctl --user is-active`.
pub fn check_systemd_service(unit: &Path, runner: &dyn CommandRunner, systemctl: &str) -> Check {
    if !unit.exists() {
        return Check::warn(
            "service",
            "not installed; run `openmosh-hook service install`",
        );
    }
    let args = vec![
        "--user".to_string(),
        "is-active".into(),
        "openmosh-hook.service".into(),
    ];
    match runner.run(systemctl, &args) {
        Ok(o) if o.success() => Check::ok("service", "active"),
        Ok(o) => Check::warn("service", format!("installed but {}", o.stdout.trim())),
        Err(_) => Check::warn("service", "installed; systemctl not available"),
    }
}

pub fn check_launchd_service(plist: &Path) -> Check {
    if plist.exists() {
        Check::ok("service", "launchd agent installed")
    } else {
        Check::warn(
            "service",
            "not installed; run `openmosh-hook service install`",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::doctor::domain::Status;
    use crate::shared::domain::command::fakes::RecordingRunner;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn permissions_flag_loose_modes() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("state");
        std::fs::create_dir(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = check_permissions(&[&d], &[&t.path().join("missing")]);
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("0755"));
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(check_permissions(&[&d], &[]).status, Status::Ok);
    }

    #[test]
    fn ssh_and_service() {
        let t = tempfile::tempdir().unwrap();
        let k = t.path().join("k.pub");
        assert_eq!(check_ssh(&k).status, Status::Fail);
        std::fs::write(
            &k,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH x",
        )
        .unwrap();
        assert_eq!(
            check_ssh(&k).detail,
            "host key SHA256:XmA4TG6/WNqReEVbojfXfv3nyHYvdvEr6WbFjs7VdLM"
        );
        let r = RecordingRunner::default();
        assert_eq!(
            check_systemd_service(&t.path().join("none"), &r, "systemctl").status,
            Status::Warn
        );
        assert!(r.argv().is_empty());
        r.reply_ok("active\n");
        assert_eq!(
            check_systemd_service(&k, &r, "systemctl").status,
            Status::Ok
        );
        assert_eq!(
            r.argv(),
            vec![vec![
                "systemctl",
                "--user",
                "is-active",
                "openmosh-hook.service"
            ]]
        );
    }
}
