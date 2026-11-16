//! Service definitions rendered as pure text, and the plan that installs them.

use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

pub const SYSTEMD_UNIT_NAME: &str = "falconer-hook.service";
pub const LAUNCHD_LABEL: &str = "dev.falconer.hook";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Systemd,
    Launchd,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ServiceError {
    #[error("the executable path contains characters that cannot be used in a service definition")]
    UnsafePath,
    #[error("service manager command failed: {0}")]
    Command(String),
    #[error("could not write the service file")]
    Io,
}

impl ErrorCode for ServiceError {
    fn code(&self) -> &'static str {
        match self {
            ServiceError::UnsafePath => "invalid_input",
            ServiceError::Command(_) => "service_error",
            ServiceError::Io => "io_error",
        }
    }
}

/// Files to write/remove and commands to run, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServicePlan {
    pub platform: Platform,
    pub write: Vec<(PathBuf, String)>,
    pub remove: Vec<PathBuf>,
    pub commands: Vec<Vec<String>>,
}

fn safe_path(p: &Path) -> Result<&str, ServiceError> {
    let s = p.to_str().ok_or(ServiceError::UnsafePath)?;
    if !s.starts_with('/') || s.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
        return Err(ServiceError::UnsafePath);
    }
    Ok(s)
}

/// systemd user unit running `<exe> serve`.
///
/// Sandboxing uses only what an unprivileged user manager can always apply
/// (seccomp and `no_new_privs`): system calls limited to `@system-service`,
/// sockets to `AF_UNIX` (Herdr's socket, the resolver) and `AF_INET`/`AF_INET6`
/// (pushes), no namespaces, realtime or SUID. Mount sandboxing
/// (`ProtectSystem=strict`, `ReadWritePaths=`, `PrivateTmp=`) is left out on
/// purpose: in a user service it needs unprivileged user namespaces, and where
/// those are off or blocked (e.g. AppArmor on Ubuntu 24.04) the service would
/// not start at all; `serve` also runs the Herdr CLI, whose own state lives
/// outside the Falconer directories. (`PrivateTmp=` alone would not affect
/// tmux, which `serve` never calls: tmux answers go through `respond`.)
pub fn systemd_unit(exe: &Path) -> Result<String, ServiceError> {
    // `%` is a systemd specifier prefix and must be doubled.
    let exe = safe_path(exe)?.replace('%', "%%");
    Ok(format!(
        "[Unit]\n\
Description=Falconer hook: agent events and encrypted push\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart=\"{exe}\" serve\n\
Restart=on-failure\n\
RestartSec=5\n\
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin\n\
UMask=0077\n\
NoNewPrivileges=yes\n\
LockPersonality=yes\n\
RestrictSUIDSGID=yes\n\
RestrictRealtime=yes\n\
RestrictNamespaces=yes\n\
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n\
SystemCallArchitectures=native\n\
SystemCallFilter=@system-service\n\
SystemCallErrorNumber=EPERM\n\
\n\
[Install]\n\
WantedBy=default.target\n"
    ))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// launchd agent running `<exe> serve`.
pub fn launchd_plist(exe: &Path, home: &Path) -> Result<String, ServiceError> {
    let exe = xml_escape(safe_path(exe)?);
    let home = xml_escape(safe_path(home)?);
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>Umask</key><integer>63</integer>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string></dict>
</dict>
</plist>
"#
    ))
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub fn install_plan(
    platform: Platform,
    exe: &Path,
    home: &Path,
    unit_path: &Path,
    systemctl: &str,
    launchctl: &str,
) -> Result<ServicePlan, ServiceError> {
    Ok(match platform {
        Platform::Systemd => ServicePlan {
            platform,
            write: vec![(unit_path.to_path_buf(), systemd_unit(exe)?)],
            remove: vec![],
            commands: vec![
                argv(&[systemctl, "--user", "daemon-reload"]),
                argv(&[systemctl, "--user", "enable", "--now", SYSTEMD_UNIT_NAME]),
            ],
        },
        Platform::Launchd => {
            let p = safe_path(unit_path)?;
            ServicePlan {
                platform,
                write: vec![(unit_path.to_path_buf(), launchd_plist(exe, home)?)],
                remove: vec![],
                commands: vec![argv(&[launchctl, "load", "-w", p])],
            }
        }
    })
}

pub fn uninstall_plan(
    platform: Platform,
    unit_path: &Path,
    systemctl: &str,
    launchctl: &str,
) -> Result<ServicePlan, ServiceError> {
    Ok(match platform {
        Platform::Systemd => ServicePlan {
            platform,
            write: vec![],
            remove: vec![unit_path.to_path_buf()],
            commands: vec![
                argv(&[systemctl, "--user", "disable", "--now", SYSTEMD_UNIT_NAME]),
                argv(&[systemctl, "--user", "daemon-reload"]),
            ],
        },
        Platform::Launchd => ServicePlan {
            platform,
            write: vec![],
            remove: vec![unit_path.to_path_buf()],
            commands: vec![argv(&[launchctl, "unload", "-w", safe_path(unit_path)?])],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_runs_serve_with_hardening() {
        let u = systemd_unit(Path::new("/home/u/.local/bin/falconer-hook")).unwrap();
        assert!(u.contains("ExecStart=\"/home/u/.local/bin/falconer-hook\" serve\n"));
        assert!(u.contains("UMask=0077\n"));
        assert!(u.contains("NoNewPrivileges=yes\n"));
        assert!(u.contains("WantedBy=default.target\n"));
        for line in [
            "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n",
            "SystemCallFilter=@system-service\n",
            "SystemCallArchitectures=native\n",
            "RestrictNamespaces=yes\n",
        ] {
            assert!(u.contains(line), "{line}");
        }
        // Mount sandboxing would stop the service where user namespaces are off.
        assert!(!u.contains("ProtectSystem") && !u.contains("PrivateTmp"));
        assert!(systemd_unit(Path::new("/a/100%/x"))
            .unwrap()
            .contains("/a/100%%/x"));
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for p in ["relative/bin", "/a\"b", "/a\nExecStartPre=/bin/sh", "/a\\b"] {
            assert_eq!(
                systemd_unit(Path::new(p)),
                Err(ServiceError::UnsafePath),
                "{p:?}"
            );
        }
    }

    #[test]
    fn plist_escapes_and_lists_arguments() {
        let p = launchd_plist(
            Path::new("/Users/a&b/bin/falconer-hook"),
            Path::new("/Users/a&b"),
        )
        .unwrap();
        assert!(
            p.contains("<string>/Users/a&amp;b/bin/falconer-hook</string><string>serve</string>")
        );
        assert!(p.contains("<key>Label</key><string>dev.falconer.hook</string>"));
    }

    #[test]
    fn plans_have_exact_commands() {
        let unit = Path::new("/h/.config/systemd/user/falconer-hook.service");
        let i = install_plan(
            Platform::Systemd,
            Path::new("/h/bin/x"),
            Path::new("/h"),
            unit,
            "systemctl",
            "launchctl",
        )
        .unwrap();
        assert_eq!(
            i.commands,
            vec![
                argv(&["systemctl", "--user", "daemon-reload"]),
                argv(&[
                    "systemctl",
                    "--user",
                    "enable",
                    "--now",
                    "falconer-hook.service"
                ]),
            ]
        );
        let u = uninstall_plan(Platform::Systemd, unit, "systemctl", "launchctl").unwrap();
        assert_eq!(u.remove, vec![unit.to_path_buf()]);
        assert_eq!(
            u.commands[0],
            argv(&[
                "systemctl",
                "--user",
                "disable",
                "--now",
                "falconer-hook.service"
            ])
        );
        let l = install_plan(
            Platform::Launchd,
            Path::new("/h/bin/x"),
            Path::new("/h"),
            Path::new("/h/Library/LaunchAgents/dev.falconer.hook.plist"),
            "systemctl",
            "launchctl",
        )
        .unwrap();
        assert_eq!(
            l.commands,
            vec![argv(&[
                "launchctl",
                "load",
                "-w",
                "/h/Library/LaunchAgents/dev.falconer.hook.plist"
            ])]
        );
    }
}
