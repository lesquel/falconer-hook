//! `attach herdr`: the real Herdr client plus the focus mirror thread.

use crate::cli::output::CliError;
use crate::features::mirror::application::{attach, MirrorLoop};
use crate::features::mirror::domain::valid_session_name;
use crate::features::mirror::infrastructure::{
    socket_path, HerdrSocket, SystemMonoClock, TtyLauncher,
};
use crate::shared::infrastructure::log::{log, Level};

use super::Ctx;

/// Runs Herdr with the inherited terminal and returns its exit code. The
/// mirror is best effort: without a socket Herdr still runs and one warning
/// is printed after it exits (so the TUI is not garbled).
pub fn attach_herdr(ctx: &Ctx, session: Option<&str>) -> Result<u8, CliError> {
    if session.is_some_and(|s| !valid_session_name(s)) {
        return Err(CliError::new("invalid_input", "invalid session name"));
    }
    let cfg = ctx.config()?;
    let args: Vec<String> = match session {
        Some(name) => vec!["session".into(), "attach".into(), name.into()],
        None => Vec::new(),
    };
    let path = socket_path(
        session,
        |k| std::env::var(k).ok(),
        &ctx.paths.xdg_config_home,
    );
    let (code, report) = attach(&TtyLauncher, &cfg.bins.herdr, &args, move |stop| {
        MirrorLoop::new(HerdrSocket::new(path), SystemMonoClock::default()).run(stop)
    })?;
    if !report.connected {
        log(
            Level::Warn,
            "herdr socket unavailable; focus mirroring was off",
            &[],
        );
    }
    Ok(u8::try_from(code).unwrap_or(1))
}
