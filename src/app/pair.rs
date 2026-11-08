//! `pair` (QR) and `pair-ssh`.

use std::net::{SocketAddr, TcpListener};

use super::commands::read_stdin;
use super::{open_state, Ctx};
use crate::cli::output::{CliError, Reply};
use crate::features::pairing::application::{
    HostFacts, PairError, Preflight, QrPairing, SshPairing,
};
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

pub fn pair(
    ctx: &Ctx,
    alias: Option<String>,
    timeout: u64,
    invert: bool,
    show_uri: bool,
) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let st = open_state(ctx)?;
    let tailscale = TailscaleCli {
        runner: &ctx.runner,
        program: cfg.bins.tailscale.clone(),
    };
    let preflight = Preflight::check(&tailscale)?;
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
    let mut session = QrPairing::new(
        &tailscale,
        &crypto,
        &st.registry,
        &st.keys,
        &SystemClock,
        &OsIds,
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
    use std::io::Write as _;
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
            "pairing cancelled; token burned",
        )),
        ListenEnd::Closed { cancelled: false } => Err(CliError::new(
            "pair_closed",
            "pairing closed (timeout or too many failed attempts); token burned",
        )),
    }
}

pub fn pair_ssh(ctx: &Ctx) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
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
