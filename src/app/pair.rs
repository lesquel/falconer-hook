//! `pair` (QR) and `pair-ssh`.

use std::io::{IsTerminal, Write as _};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::commands::read_stdin;
use super::{open_state, Ctx};
use crate::cli::output::{CliError, Reply};
use crate::features::pairing::application::{
    HostFacts, PairError, Preflight, QrPairing, SshPairing,
};
use crate::features::pairing::domain::{tailscale_ssh_warning, PairConfirm};
use crate::features::pairing::infrastructure::listener::{run_listener, ListenEnd};
use crate::features::pairing::infrastructure::pair_crypto::X25519PairCrypto;
use crate::features::pairing::infrastructure::qr_render::render_qr;
use crate::features::pairing::infrastructure::tailscale::TailscaleCli;
use crate::shared::domain::command::CommandRunner;
use crate::shared::infrastructure::crypto::random_bytes;
use crate::shared::infrastructure::log::{log, Field, Level};
use crate::shared::infrastructure::ssh_fingerprint::fingerprint;
use crate::shared::infrastructure::system::{OsIds, SystemClock};

fn ssh_user(ctx: &Ctx) -> Result<String, CliError> {
    if let Some(u) = &ctx.user {
        return Ok(u.clone());
    }
    ctx.runner
        .run("id", &["-un".to_string()])
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| CliError::new("internal", "could not determine the SSH user"))
}

/// How long the host user has to answer the pairing prompt.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);

/// `y`/`yes` (any case) is the only yes.
pub fn is_yes(answer: &str) -> bool {
    let a = answer.trim();
    a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
}

/// `--yes`: the user accepted any phone that passes the checks in advance.
struct PreApproved;
impl PairConfirm for PreApproved {
    fn confirm(&self, _: &str, _: &str, _: IpAddr) -> bool {
        true
    }
}

/// Asks on the terminal; no answer within the timeout, Ctrl+C or EOF is a no.
struct TerminalConfirm<'a> {
    stop: &'a AtomicBool,
}

impl PairConfirm for TerminalConfirm<'_> {
    fn confirm(&self, name: &str, node: &str, ip: IpAddr) -> bool {
        print!("Pair \"{name}\" from {node} ({ip})? [y/N] ");
        let _ = std::io::stdout().flush();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = std::io::stdin().read_line(&mut line);
            let _ = tx.send(read.ok().filter(|n| *n > 0).map(|_| line));
        });
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(line)) => {
                    let yes = is_yes(&line);
                    if !yes {
                        println!("Not paired.");
                    }
                    return yes;
                }
                Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                        println!("\nNo answer: not paired.");
                        return false;
                    }
                }
            }
        }
    }
}

pub fn pair(
    ctx: &Ctx,
    alias: Option<String>,
    timeout: u64,
    invert: bool,
    show_uri: bool,
    yes: bool,
) -> Result<Reply, CliError> {
    // The host user confirms each phone; without a terminal that needs --yes.
    if !yes && !std::io::stdin().is_terminal() {
        return Err(CliError::new(
            "confirmation_required",
            "pairing asks for confirmation on this terminal; run it interactively or pass --yes",
        ));
    }
    let cfg = ctx.config()?;
    let st = open_state(ctx)?;
    let tailscale = TailscaleCli {
        runner: &ctx.runner,
        program: cfg.bins.tailscale.clone(),
    };
    let preflight = Preflight::check(&tailscale)?;
    if let Some(w) = tailscale_ssh_warning(&tailscale, cfg.ssh_port) {
        eprintln!("{w}");
    }
    let ssh_fp = std::fs::read_to_string(&cfg.ssh_host_key_pub)
        .ok()
        .as_deref()
        .and_then(fingerprint)
        .ok_or(PairError::NoSshHostKey)?;
    let mut cfg_alias = cfg.clone();
    if let Some(a) = alias {
        cfg_alias.host_alias = Some(a);
    }
    let facts = HostFacts {
        alias: ctx.host_alias(&cfg_alias),
        ssh_user: ssh_user(ctx)?,
        ssh_port: cfg.ssh_port,
        ssh_fp,
        hpk: st.identity.public(),
    };
    // Bind only to the Tailscale IPv4, ephemeral port.
    let listener = TcpListener::bind(SocketAddr::new(preflight.ipv4.into(), 0))
        .map_err(|_| CliError::new("listen_failed", "could not listen on the Tailscale address"))?;
    let port = listener
        .local_addr()
        .map_err(|_| CliError::new("listen_failed", "could not read the listener port"))?
        .port();
    let crypto = X25519PairCrypto::new(st.identity.secret());
    let ip = preflight.ipv4;
    let alias_shown = facts.alias.clone();
    let terminal = TerminalConfirm { stop: &ctx.stop };
    let confirm: &dyn PairConfirm = if yes { &PreApproved } else { &terminal };
    let mut session = QrPairing::new(
        &tailscale,
        &crypto,
        &st.registry,
        &st.keys,
        &SystemClock,
        &OsIds,
        confirm,
        preflight,
        facts,
        random_bytes::<{ crate::features::pairing::domain::PAIR_TOKEN_LEN }>(),
        timeout,
    );
    let offer = session.offer(port);
    let qr = render_qr(&offer.uri(), invert)
        .ok_or_else(|| CliError::new("internal", "could not render the QR code"))?;
    println!("{qr}");
    if show_uri {
        println!("{}", offer.uri());
    }
    println!(
        "Scan with OpenMosh: Add host > Scan QR  (host \"{}\")",
        alias_shown
    );
    println!("Listening on {ip}:{port} (Tailscale only) for {timeout} s. Ctrl+C cancels.");
    if !yes {
        println!("You will be asked to confirm the phone before its key is authorized.");
    }
    let _ = std::io::stdout().flush();
    let end = run_listener(&listener, &mut session, &ctx.stop).map_err(|e| {
        log(
            Level::Error,
            "pairing listener failed",
            &[("err", Field::Text(&e.to_string()))],
        );
        CliError::new("listen_failed", "pairing listener failed")
    })?;
    match end {
        ListenEnd::Paired(device) => Ok(Reply::new(
            &serde_json::json!({"device_id": device.id, "name": device.name}),
            format!("Paired {}", device.name),
        )),
        ListenEnd::Closed { cancelled: true } => Err(CliError::new(
            "pair_cancelled",
            "pairing cancelled or declined; token burned",
        )),
        ListenEnd::Closed { cancelled: false } => Err(CliError::new(
            "pair_closed",
            "pairing closed (timeout or too many failed attempts); token burned",
        )),
    }
}

pub fn pair_ssh(ctx: &Ctx) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let tailscale = TailscaleCli {
        runner: &ctx.runner,
        program: cfg.bins.tailscale.clone(),
    };
    if let Some(w) = tailscale_ssh_warning(&tailscale, cfg.ssh_port) {
        eprintln!("{w}");
    }
    let input = read_stdin(16 * 1024)?;
    let st = open_state(ctx)?;
    let uc = SshPairing {
        registry: &st.registry,
        clock: &SystemClock,
        ids: &OsIds,
        alias: ctx.host_alias(&cfg),
        hpk: st.identity.public(),
    };
    let r = uc.pair(&input)?;
    let human = format!("Paired device {}", r.device_id);
    Ok(Reply::new(&r, human))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::fakes::FakeTailnet;

    #[test]
    fn ssh_warning_only_when_enabled_on_port_22() {
        let mut t = FakeTailnet::running("me", &[]);
        t.ssh = Some(true);
        let w = tailscale_ssh_warning(&t, 22).unwrap();
        assert!(w.contains("sudo tailscale set --ssh=false"), "{w}");
        assert_eq!(tailscale_ssh_warning(&t, 2222), None);
        t.ssh = Some(false);
        assert_eq!(tailscale_ssh_warning(&t, 22), None);
        t.ssh = None;
        assert_eq!(tailscale_ssh_warning(&t, 22), None);
    }

    #[test]
    fn only_y_or_yes_confirms() {
        for y in ["y", "Y", "yes", " YES \n"] {
            assert!(is_yes(y), "{y:?}");
        }
        for n in ["", "\n", "n", "no", "yep", "y es", "1"] {
            assert!(!is_yes(n), "{n:?}");
        }
    }
}
