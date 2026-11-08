//! `harden-ssh`: make sshd accept public keys only, through a drop-in file.
//!
//! Every privileged step goes through `sudo`, so the user sees the password
//! prompt and nothing is changed without it. The drop-in is validated with
//! `sshd -t` before sshd reloads, and removed again when validation fails.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::json;

use super::{CliError, Ctx, Reply};
use crate::features::doctor::domain::Status;
use crate::features::doctor::infrastructure::check_sshd_auth;
use crate::shared::domain::command::{CommandError, CommandOutput, CommandRunner};

pub const DROP_IN: &str = "/etc/ssh/sshd_config.d/10-openmosh.conf";
const MAIN_CONFIG: &str = "/etc/ssh/sshd_config";

pub const DROP_IN_CONTENT: &str = "# Written by openmosh-hook harden-ssh: public keys only.\n\
PasswordAuthentication no\n\
KbdInteractiveAuthentication no\n\
AuthenticationMethods publickey\n";

/// Whether `sshd_config` reads the drop-in directory (Debian, Ubuntu, Arch,
/// Fedora and macOS do by default).
pub fn includes_drop_ins(main_config: &str) -> bool {
    main_config.lines().any(|l| {
        let mut it = l.split_whitespace();
        it.next().is_some_and(|k| k.eq_ignore_ascii_case("include"))
            && it.any(|p| p.starts_with("/etc/ssh/sshd_config.d/") || p == "sshd_config.d/*.conf")
    })
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// The reload commands to try in order; the first that succeeds wins.
pub fn reload_commands(macos: bool) -> Vec<Vec<String>> {
    if macos {
        vec![argv(&[
            "sudo",
            "launchctl",
            "kickstart",
            "-k",
            "system/com.openssh.sshd",
        ])]
    } else {
        vec![
            argv(&["sudo", "systemctl", "reload", "sshd"]),
            argv(&["sudo", "systemctl", "reload", "ssh"]),
        ]
    }
}

/// Human description of the plan, for the confirmation and `--dry-run`.
pub fn describe(macos: bool) -> String {
    let reload = reload_commands(macos)
        .iter()
        .map(|c| c.join(" "))
        .collect::<Vec<_>>()
        .join("` or `");
    format!(
        "- write {DROP_IN} (sudo):\n{}\
         - check the configuration: `sudo sshd -t` (the file is removed again if it fails)\n\
         - reload sshd: `{reload}`\n\
         Password logins over SSH stop working. Logins with an SSH key, and the\n\
         computer's own keyboard and screen, are not affected.",
        DROP_IN_CONTENT
            .lines()
            .map(|l| format!("    {l}\n"))
            .collect::<String>()
    )
}

/// Runs the privileged steps. `staged` is a private file holding
/// [`DROP_IN_CONTENT`].
pub fn apply(runner: &dyn CommandRunner, staged: &Path, macos: bool) -> Result<(), CliError> {
    let ok = |c: &[String]| {
        runner
            .run(&c[0], &c[1..])
            .is_ok_and(|o: CommandOutput| o.success())
    };
    let staged = staged.to_string_lossy().to_string();
    if !ok(&argv(&["sudo", "install", "-m", "0644", &staged, DROP_IN])) {
        return Err(CliError::new(
            "harden_failed",
            format!("could not write {DROP_IN}; nothing changed"),
        ));
    }
    if !ok(&argv(&["sudo", "sshd", "-t"])) {
        let _ = ok(&argv(&["sudo", "rm", "-f", DROP_IN]));
        return Err(CliError::new(
            "harden_failed",
            "`sshd -t` rejected the configuration; the file was removed and sshd was not reloaded",
        ));
    }
    if !reload_commands(macos).iter().any(|c| ok(c)) {
        return Err(CliError::new(
            "harden_failed",
            format!("{DROP_IN} is in place but sshd could not be reloaded; reload it or reboot"),
        ));
    }
    Ok(())
}

/// Runs programs with the terminal attached, so `sudo` can ask for the
/// password, and without a timeout.
struct InteractiveRunner;

impl CommandRunner for InteractiveRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, CommandError> {
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|_| CommandError::Spawn(program.to_string()))?;
        Ok(CommandOutput {
            status: status.code().unwrap_or(1),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

pub fn harden_ssh(ctx: &Ctx, dry_run: bool, yes: bool, json: bool) -> Result<Reply, CliError> {
    let macos = cfg!(target_os = "macos");
    let before = check_sshd_auth(&ctx.runner, "sshd");
    if before.status == Status::Ok && !dry_run {
        return Ok(Reply::new(
            &json!({"changed": false, "sshd_auth": before.detail}),
            format!("sshd already accepts public keys only ({}).", before.detail),
        ));
    }
    let main = std::fs::read_to_string(MAIN_CONFIG).unwrap_or_default();
    if !main.is_empty() && !includes_drop_ins(&main) {
        return Err(CliError::new(
            "unsupported",
            format!(
                "{MAIN_CONFIG} does not include /etc/ssh/sshd_config.d/*.conf; add the three \
                 lines of `openmosh-hook harden-ssh --dry-run` to it by hand"
            ),
        ));
    }
    let plan = describe(macos);
    if dry_run {
        return Ok(Reply::new(
            &json!({"dry_run": true, "file": DROP_IN, "content": DROP_IN_CONTENT}),
            format!("openmosh-hook harden-ssh would:\n{plan}"),
        ));
    }
    if !yes {
        if json || !std::io::stdin().is_terminal() {
            return Err(CliError::new(
                "confirmation_required",
                "harden-ssh needs confirmation: pass --yes, or --dry-run to see the plan",
            ));
        }
        eprintln!("openmosh-hook harden-ssh will:\n{plan}");
        if !ask("Proceed?") {
            return Err(CliError::new(
                "cancelled",
                "harden-ssh cancelled; nothing changed",
            ));
        }
    }
    let staged = ctx.paths.state_dir.join("sshd-drop-in.conf");
    std::fs::create_dir_all(&ctx.paths.state_dir)
        .and_then(|_| std::fs::write(&staged, DROP_IN_CONTENT))
        .map_err(|_| CliError::new("io", "could not stage the sshd drop-in"))?;
    let result = apply(&InteractiveRunner, &staged, macos);
    let _ = std::fs::remove_file(&staged);
    result?;
    let after = check_sshd_auth(&ctx.runner, "sshd");
    Ok(Reply::new(
        &json!({"changed": true, "file": DROP_IN, "sshd_auth": after.detail}),
        format!("sshd now: {}.", after.detail),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;

    fn status(r: &RecordingRunner, code: i32) -> &RecordingRunner {
        r.reply(Ok(CommandOutput {
            status: code,
            stdout: String::new(),
            stderr: String::new(),
        }))
    }

    fn calls(r: &RecordingRunner) -> Vec<String> {
        r.calls.borrow().iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn writes_checks_then_reloads() {
        let r = RecordingRunner::default();
        r.reply_ok("").reply_ok("").reply_ok("");
        apply(&r, Path::new("/s/x.conf"), false).unwrap();
        assert_eq!(
            calls(&r),
            [
                format!("sudo install -m 0644 /s/x.conf {DROP_IN}"),
                "sudo sshd -t".to_string(),
                "sudo systemctl reload sshd".to_string(),
            ]
        );
    }

    #[test]
    fn a_rejected_config_is_removed_and_sshd_not_reloaded() {
        let r = RecordingRunner::default();
        status(r.reply_ok(""), 255).reply_ok("");
        let e = apply(&r, Path::new("/s/x.conf"), false).unwrap_err();
        assert_eq!(e.code, "harden_failed");
        assert_eq!(calls(&r).last().unwrap(), &format!("sudo rm -f {DROP_IN}"));
        assert!(!calls(&r).iter().any(|c| c.contains("reload")));
    }

    #[test]
    fn falls_back_to_the_debian_service_name() {
        let r = RecordingRunner::default();
        status(r.reply_ok("").reply_ok(""), 5).reply_ok("");
        apply(&r, Path::new("/s/x.conf"), false).unwrap();
        assert_eq!(calls(&r).last().unwrap(), "sudo systemctl reload ssh");
    }

    #[test]
    fn detects_the_drop_in_include() {
        assert!(includes_drop_ins(
            "Include /etc/ssh/sshd_config.d/*.conf\nPort 22\n"
        ));
        assert!(!includes_drop_ins(
            "# Include /etc/ssh/sshd_config.d/*.conf\nPort 22\n"
        ));
        assert!(!includes_drop_ins("Port 22\n"));
    }

    #[test]
    fn the_drop_in_turns_off_passwords() {
        let c = crate::features::doctor::infrastructure::sshd_auth_check(
            &DROP_IN_CONTENT.to_lowercase(),
        );
        assert_eq!(c.status, Status::Ok, "{}", c.detail);
    }
}
