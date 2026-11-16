//! `harden-ssh`: make sshd accept public keys only, through a drop-in file.
//!
//! Every privileged step goes through `sudo` by absolute path (a `sudo` earlier
//! in `$PATH` could capture the password), so the user sees the password
//! prompt and nothing is changed without it. The drop-in content is a
//! constant piped into `sudo tee` (no staged file a local process could swap).
//! It is validated with `sshd -t` before sshd reloads, and removed again when
//! validation fails.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::json;

use super::{CliError, Ctx, Reply};
use crate::features::doctor::domain::Status;
use crate::features::doctor::infrastructure::{sshd_auth_check, sshd_password_logins};
use crate::shared::domain::command::{CommandError, CommandOutput, CommandRunner};

pub const DROP_IN: &str = "/etc/ssh/sshd_config.d/10-falconer.conf";
const MAIN_CONFIG: &str = "/etc/ssh/sshd_config";

pub const DROP_IN_CONTENT: &str = "# Written by falconer-hook harden-ssh: public keys only.\n\
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

/// The first of `candidates` that exists, else the first one (so the error
/// names the expected path).
fn first_existing(candidates: &[&'static str]) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|p| Path::new(p).exists())
        .unwrap_or(candidates[0])
}

/// Absolute paths of the programs run as root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootTools {
    pub sudo: &'static str,
    pub tee: &'static str,
}

impl RootTools {
    pub fn find() -> Self {
        Self {
            sudo: first_existing(&["/usr/bin/sudo", "/run/wrappers/bin/sudo", "/bin/sudo"]),
            tee: first_existing(&["/usr/bin/tee", "/bin/tee"]),
        }
    }
}

/// The reload commands to try in order; the first that succeeds wins.
pub fn reload_commands(sudo: &str, macos: bool) -> Vec<Vec<String>> {
    if macos {
        vec![argv(&[
            sudo,
            "launchctl",
            "kickstart",
            "-k",
            "system/com.openssh.sshd",
        ])]
    } else {
        vec![
            argv(&[sudo, "systemctl", "reload", "sshd"]),
            argv(&[sudo, "systemctl", "reload", "ssh"]),
        ]
    }
}

/// Human description of the plan, for the confirmation and `--dry-run`.
pub fn describe(tools: RootTools, macos: bool) -> String {
    let reload = reload_commands(tools.sudo, macos)
        .iter()
        .map(|c| c.join(" "))
        .collect::<Vec<_>>()
        .join("` or `");
    let sudo = tools.sudo;
    format!(
        "- write {DROP_IN} (`{sudo} {tee} {DROP_IN}`, mode 0644):\n{}\
         - check the configuration: `{sudo} sshd -t` (the file is removed again if it fails)\n\
         - reload sshd: `{reload}`\n\
         Password logins over SSH stop working. Logins with an SSH key, and the\n\
         computer's own keyboard and screen, are not affected.",
        DROP_IN_CONTENT
            .lines()
            .map(|l| format!("    {l}\n"))
            .collect::<String>(),
        tee = tools.tee,
    )
}

/// Runs `argv` with `input` on its stdin; true when it exits 0.
pub type PipeRunner<'a> = &'a dyn Fn(&[String], &[u8]) -> bool;

/// Runs the privileged steps: [`DROP_IN_CONTENT`] is piped into
/// `sudo tee <drop-in>` (stdout discarded), then the mode is fixed to 0644.
pub fn apply(
    runner: &dyn CommandRunner,
    pipe: PipeRunner<'_>,
    tools: RootTools,
    macos: bool,
) -> Result<(), CliError> {
    let ok = |c: &[String]| {
        runner
            .run(&c[0], &c[1..])
            .is_ok_and(|o: CommandOutput| o.success())
    };
    let sudo = tools.sudo;
    let written = pipe(
        &argv(&[sudo, tools.tee, DROP_IN]),
        DROP_IN_CONTENT.as_bytes(),
    ) && ok(&argv(&[sudo, "chmod", "0644", DROP_IN]));
    if !written {
        let _ = ok(&argv(&[sudo, "rm", "-f", DROP_IN]));
        return Err(CliError::new(
            "harden_failed",
            format!("could not write {DROP_IN}; nothing changed"),
        ));
    }
    if !ok(&argv(&[sudo, "sshd", "-t"])) {
        let _ = ok(&argv(&[sudo, "rm", "-f", DROP_IN]));
        return Err(CliError::new(
            "harden_failed",
            "`sshd -t` rejected the configuration; the file was removed and sshd was not reloaded",
        ));
    }
    if !reload_commands(sudo, macos).iter().any(|c| ok(c)) {
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

/// Pipes `input` into `argv`, with the terminal attached for sudo's prompt
/// (stdout is discarded so `tee` does not echo the file).
fn pipe_interactive(argv: &[String], input: &[u8]) -> bool {
    let Ok(mut child) = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut s| s.write_all(input).is_ok());
    child.wait().is_ok_and(|s| s.success()) && wrote
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

pub fn harden_ssh(ctx: &Ctx, dry_run: bool, yes: bool, json: bool) -> Result<Reply, CliError> {
    let macos = cfg!(target_os = "macos");
    // Unknown (sshd -G unavailable) is not "hardened": go on and let sshd -t judge.
    let user = ctx.user.as_deref();
    let tools = RootTools::find();
    let before = sshd_password_logins(&ctx.runner, "sshd", user).map(|g| sshd_auth_check(&g));
    if let Some(check) = before.filter(|c| c.status == Status::Ok) {
        if !dry_run {
            return Ok(Reply::new(
                &json!({"changed": false, "sshd_auth": check.detail}),
                format!("sshd already accepts public keys only ({}).", check.detail),
            ));
        }
    }
    let keys =
        std::fs::read_to_string(ctx.paths.home.join(".ssh/authorized_keys")).unwrap_or_default();
    if !has_key(&keys) {
        return Err(CliError::new(
            "no_authorized_keys",
            "~/.ssh/authorized_keys has no key: turning off passwords would lock out every remote \
             login. Pair a phone or add your key first",
        ));
    }
    let main = std::fs::read_to_string(MAIN_CONFIG).unwrap_or_default();
    if !main.is_empty() && !includes_drop_ins(&main) {
        return Err(CliError::new(
            "unsupported",
            format!(
                "{MAIN_CONFIG} does not include /etc/ssh/sshd_config.d/*.conf; add the three \
                 lines of `falconer-hook harden-ssh --dry-run` to it by hand"
            ),
        ));
    }
    let plan = describe(tools, macos);
    if dry_run {
        return Ok(Reply::new(
            &json!({"dry_run": true, "file": DROP_IN, "content": DROP_IN_CONTENT}),
            format!("falconer-hook harden-ssh would:\n{plan}"),
        ));
    }
    if !yes {
        if json || !std::io::stdin().is_terminal() {
            return Err(CliError::new(
                "confirmation_required",
                "harden-ssh needs confirmation: pass --yes, or --dry-run to see the plan",
            ));
        }
        eprintln!("falconer-hook harden-ssh will:\n{plan}");
        if !ask("Proceed?") {
            return Err(CliError::new(
                "cancelled",
                "harden-ssh cancelled; nothing changed",
            ));
        }
    }
    apply(&InteractiveRunner, &pipe_interactive, tools, macos)?;
    let detail = match sshd_password_logins(&ctx.runner, "sshd", user) {
        Some(g) => sshd_auth_check(&g).detail,
        None => "drop-in installed and sshd reloaded".to_string(),
    };
    Ok(Reply::new(
        &json!({"changed": true, "file": DROP_IN, "sshd_auth": detail}),
        format!("sshd now: {detail}."),
    ))
}

/// At least one non-comment line in an authorized_keys file.
pub fn has_key(authorized_keys: &str) -> bool {
    authorized_keys
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
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

    const TOOLS: RootTools = RootTools {
        sudo: "/usr/bin/sudo",
        tee: "/usr/bin/tee",
    };

    #[test]
    fn pipes_the_constant_through_absolute_sudo_tee_then_checks_and_reloads() {
        let r = RecordingRunner::default();
        let log: std::cell::RefCell<Vec<(String, Vec<u8>)>> = Default::default();
        let pipe = |a: &[String], i: &[u8]| {
            log.borrow_mut().push((a.join(" "), i.to_vec()));
            true
        };
        apply(&r, &pipe, TOOLS, false).unwrap();
        assert_eq!(
            log.borrow().as_slice(),
            [(
                format!("/usr/bin/sudo /usr/bin/tee {DROP_IN}"),
                DROP_IN_CONTENT.as_bytes().to_vec()
            )]
        );
        assert_eq!(
            calls(&r),
            [
                format!("/usr/bin/sudo chmod 0644 {DROP_IN}"),
                "/usr/bin/sudo sshd -t".to_string(),
                "/usr/bin/sudo systemctl reload sshd".to_string(),
            ]
        );
    }

    #[test]
    fn a_failed_write_changes_nothing_else() {
        let r = RecordingRunner::default();
        let pipe = |_: &[String], _: &[u8]| false;
        let e = apply(&r, &pipe, TOOLS, false).unwrap_err();
        assert_eq!(e.code, "harden_failed");
        assert!(!calls(&r)
            .iter()
            .any(|c| c.contains("sshd -t") || c.contains("reload")));
    }

    #[test]
    fn a_rejected_config_is_removed_and_sshd_not_reloaded() {
        let r = RecordingRunner::default();
        status(r.reply_ok(""), 255).reply_ok("");
        let pipe = |_: &[String], _: &[u8]| true;
        let e = apply(&r, &pipe, TOOLS, false).unwrap_err();
        assert_eq!(e.code, "harden_failed");
        assert_eq!(
            calls(&r).last().unwrap(),
            &format!("/usr/bin/sudo rm -f {DROP_IN}")
        );
        assert!(!calls(&r).iter().any(|c| c.contains("reload")));
    }

    #[test]
    fn falls_back_to_the_debian_service_name() {
        let r = RecordingRunner::default();
        status(r.reply_ok("").reply_ok(""), 5).reply_ok("");
        let pipe = |_: &[String], _: &[u8]| true;
        apply(&r, &pipe, TOOLS, false).unwrap();
        assert_eq!(
            calls(&r).last().unwrap(),
            "/usr/bin/sudo systemctl reload ssh"
        );
    }

    #[test]
    fn sshd_g_evaluates_match_blocks_for_the_user() {
        let r = RecordingRunner::default();
        r.reply_ok("passwordauthentication no\n");
        assert!(sshd_password_logins(&r, "sshd", Some("me")).is_some());
        assert_eq!(
            calls(&r),
            ["sshd -G -C user=me,host=localhost,addr=127.0.0.1"]
        );
    }

    #[test]
    fn needs_an_authorized_key_before_turning_passwords_off() {
        assert!(has_key("ssh-ed25519 AAAA openmosh:x\n"));
        assert!(!has_key("# only a comment\n\n"));
        assert!(!has_key(""));
    }

    #[test]
    fn an_unrunnable_sshd_is_unknown_not_hardened() {
        let r = RecordingRunner::default();
        r.reply(Err(CommandError::NotFound("sshd".into())))
            .reply(Err(CommandError::NotFound("/usr/sbin/sshd".into())));
        assert_eq!(sshd_password_logins(&r, "sshd", None), None);
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
