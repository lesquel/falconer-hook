//! Filesystem and service checks.

use std::path::{Path, PathBuf};

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

/// True when something accepts TCP connections on `port` of this machine.
pub fn port_listening(port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|ip| {
        TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_millis(500)).is_ok()
    })
}

/// The host key can exist while sshd is off (macOS "Remote Login"), so this
/// checks that something actually listens. `fix` says how to turn it on.
pub fn check_ssh_listening(port: u16, listening: bool, fix: &str) -> Check {
    if listening {
        Check::ok("ssh_port", format!("listening on port {port}"))
    } else {
        Check::fail("ssh_port", format!("nothing listens on port {port}; {fix}"))
    }
}

/// Read-only: `sshd -G` effective settings. Password or keyboard-interactive
/// logins bypass every key restriction Falconer adds, so they are a warning.
pub fn check_sshd_auth(effective: Option<&str>) -> Check {
    match effective {
        Some(effective) => sshd_auth_check(effective),
        None => Check::ok("sshd auth", "not checked (`sshd -G` unavailable)"),
    }
}

/// The files sshd reads keys from (`authorizedkeysfile` in `sshd -G`), with
/// `%h`, `%u` and `%%` expanded and relative paths taken from the home.
/// `None` when a token cannot be expanded here.
pub fn authorized_keys_files(effective: &str, home: &Path, user: &str) -> Option<Vec<PathBuf>> {
    let line = effective.lines().find(|l| {
        l.split_whitespace()
            .next()
            .is_some_and(|k| k.eq_ignore_ascii_case("authorizedkeysfile"))
    })?;
    let mut out = Vec::new();
    for tok in line.split_whitespace().skip(1) {
        if tok.eq_ignore_ascii_case("none") {
            continue;
        }
        let mut p = String::new();
        let mut chars = tok.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                p.push(c);
                continue;
            }
            match chars.next()? {
                '%' => p.push('%'),
                'h' => p.push_str(&home.to_string_lossy()),
                'u' => p.push_str(user),
                _ => return None,
            }
        }
        let path = PathBuf::from(&p);
        out.push(if path.is_absolute() {
            path
        } else {
            home.join(path)
        });
    }
    Some(out)
}

/// Read-only: sshd reads the file Falconer writes, and no unexpected key was
/// added after the first phone was paired (keys without an `openmosh:` tag
/// below the first tagged line).
pub fn check_authorized_keys(
    effective: Option<&str>,
    home: &Path,
    user: &str,
    keys: Option<&str>,
) -> Check {
    use crate::features::pairing::domain::is_tagged_line;
    let ours = home.join(".ssh/authorized_keys");
    if let Some(files) = effective.and_then(|e| authorized_keys_files(e, home, user)) {
        if !files.contains(&ours) {
            let list: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
            return Check::warn(
                "authorized keys",
                format!(
                    "sshd reads {} but Falconer writes {}: paired phones cannot log in",
                    if list.is_empty() {
                        "no key file".to_string()
                    } else {
                        list.join(" ")
                    },
                    ours.display()
                ),
            );
        }
    }
    let text = keys.unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let phones = lines.iter().filter(|l| is_tagged_line(l)).count();
    let later = lines
        .iter()
        .skip_while(|l| !is_tagged_line(l))
        .filter(|l| !is_tagged_line(l))
        .count();
    if later > 0 {
        return Check::warn(
            "authorized keys",
            format!(
                "{later} key line(s) without an openmosh tag were added after the first phone \
                 was paired; check that you added them ({})",
                ours.display()
            ),
        );
    }
    Check::ok(
        "authorized keys",
        format!("{phones} phone key(s) in {}", ours.display()),
    )
}

/// `sshd -G` output, or None when it cannot run (sshd outside a non-root
/// PATH, as on Debian, or an OpenSSH without `-G`). With a user, `-C
/// user=<u>,host=localhost,addr=127.0.0.1` makes sshd apply its `Match`
/// blocks for that user, so a `Match` that turns passwords back on is seen.
pub fn sshd_password_logins(
    runner: &dyn CommandRunner,
    sshd: &str,
    user: Option<&str>,
) -> Option<String> {
    let mut args = vec!["-G".to_string()];
    if let Some(u) = user {
        args.push("-C".into());
        args.push(format!("user={u},host=localhost,addr=127.0.0.1"));
    }
    [sshd, "/usr/sbin/sshd"].iter().find_map(|bin| {
        runner
            .run(bin, &args)
            .ok()
            .filter(|o| o.success())
            .map(|o| o.stdout)
    })
}

pub fn sshd_auth_check(effective: &str) -> Check {
    let enabled = |key: &str| {
        effective.lines().any(|l| {
            let mut it = l.split_whitespace();
            it.next().is_some_and(|k| k.eq_ignore_ascii_case(key))
                && it.next().is_some_and(|v| v.eq_ignore_ascii_case("yes"))
        })
    };
    let open: Vec<&str> = ["passwordauthentication", "kbdinteractiveauthentication"]
        .into_iter()
        .filter(|k| enabled(k))
        .collect();
    if open.is_empty() {
        Check::ok("sshd auth", "public key only")
    } else {
        Check::warn(
            "sshd auth",
            format!(
                "{} enabled: anyone who reaches port 22 can guess passwords. Fix: \
                 `falconer-hook harden-ssh` (or `s` on the TUI Status screen)",
                open.join(", ")
            ),
        )
    }
}

/// Read-only: unit file present and `systemctl --user is-active`.
pub fn check_systemd_service(unit: &Path, runner: &dyn CommandRunner, systemctl: &str) -> Check {
    if !unit.exists() {
        return Check::warn(
            "service",
            "not installed; run `falconer-hook service install`",
        );
    }
    let args = vec![
        "--user".to_string(),
        "is-active".into(),
        "falconer-hook.service".into(),
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
            "not installed; run `falconer-hook service install`",
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
    fn ssh_port_check_reports_the_fix() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_listening(port));
        assert_eq!(check_ssh_listening(port, true, "x").status, Status::Ok);
        drop(l);
        assert!(!port_listening(port));
        let c = check_ssh_listening(port, false, "run `fix`");
        assert_eq!(c.status, Status::Fail);
        assert!(c.detail.contains("nothing listens") && c.detail.contains("run `fix`"));
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
                "falconer-hook.service"
            ]]
        );
    }

    #[test]
    fn sshd_key_files_must_include_ours() {
        use crate::features::doctor::domain::Status;
        let home = Path::new("/home/me");
        assert_eq!(
            super::authorized_keys_files(
                "port 22\nauthorizedkeysfile .ssh/authorized_keys /etc/keys/%u %h/k%%\n",
                home,
                "me"
            )
            .unwrap(),
            vec![
                PathBuf::from("/home/me/.ssh/authorized_keys"),
                PathBuf::from("/etc/keys/me"),
                PathBuf::from("/home/me/k%"),
            ]
        );
        assert_eq!(
            super::authorized_keys_files("authorizedkeysfile %U", home, "me"),
            None
        );
        let elsewhere = super::check_authorized_keys(
            Some("authorizedkeysfile /etc/ssh/keys/%u\n"),
            home,
            "me",
            None,
        );
        assert_eq!(elsewhere.status, Status::Warn);
        assert!(
            elsewhere.detail.contains("cannot log in"),
            "{}",
            elsewhere.detail
        );
        let fine = super::check_authorized_keys(
            Some("authorizedkeysfile .ssh/authorized_keys .ssh/authorized_keys2\n"),
            home,
            "me",
            Some("ssh-ed25519 AAAA laptop\nrestrict ssh-ed25519 BBBB openmosh:AAAAAAAAAAAAAAAAAAAA01\n"),
        );
        assert_eq!(fine.status, Status::Ok, "{}", fine.detail);
        assert!(fine.detail.starts_with("1 phone key(s)"));
    }

    #[test]
    fn keys_added_after_the_first_pairing_are_flagged() {
        use crate::features::doctor::domain::Status;
        let c = super::check_authorized_keys(
            None,
            Path::new("/home/me"),
            "me",
            Some("ssh-ed25519 AAAA laptop\nrestrict ssh-ed25519 BBBB openmosh:AAAAAAAAAAAAAAAAAAAA01\n# note\nssh-ed25519 CCCC who\n"),
        );
        assert_eq!(c.status, Status::Warn);
        assert!(
            c.detail
                .starts_with("1 key line(s) without an openmosh tag"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn sshd_password_logins_are_a_warning() {
        let open = super::sshd_auth_check(
            "port 22\npasswordauthentication yes\npubkeyauthentication yes\n",
        );
        assert_eq!(open.status, crate::features::doctor::domain::Status::Warn);
        assert!(open.detail.contains("passwordauthentication"));
        let closed =
            super::sshd_auth_check("passwordauthentication no\nkbdinteractiveauthentication no\n");
        assert_eq!(closed.status, crate::features::doctor::domain::Status::Ok);
        let kbd = super::sshd_auth_check("kbdinteractiveauthentication yes\n");
        assert!(kbd.detail.contains("kbdinteractiveauthentication"));
    }
}
