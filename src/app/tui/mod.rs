//! Interactive terminal UI over the same commands as the CLI.
//!
//! Presentation only: `state` turns keys into `cli::Command`s, `view` draws
//! the state, and this module owns the terminal (raw mode, alternate screen,
//! restore on exit and on panic) and runs each command through `app::run`.

pub mod actions;
pub mod i18n;
pub mod state;
pub mod view;

use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Once};
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use serde_json::Value;

use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::cli::{Command, DevicesCmd};
use crate::shared::infrastructure::process::ProcessRunner;
use i18n::{t, Lang};
use state::{App, Effect, Exec};

/// Runs commands through the composition root, exactly like the CLI.
struct CtxExec<'a>(&'a Ctx);

impl Exec for CtxExec<'_> {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError> {
        super::run(self.0, cmd)
    }

    /// The command reads the process stdin, which is the terminal here, so
    /// it runs as a child `falconer-hook` with `input` piped in.
    fn exec_stdin(&mut self, cmd: Command, input: &str) -> Result<Reply, CliError> {
        let Command::Devices {
            cmd: DevicesCmd::SetWebpush { id, .. },
        } = cmd
        else {
            return Err(CliError::new("internal", "this command reads no stdin"));
        };
        let mut child = std::process::Command::new(&self.0.exe)
            .args(["devices", "set-webpush", "--json", "--", &id])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| CliError::new("internal", format!("could not run itself: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        let out = child
            .wait_with_output()
            .map_err(|e| CliError::new("internal", e.to_string()))?;
        let v: Value = serde_json::from_slice(&out.stdout)
            .map_err(|_| CliError::new("internal", "unexpected output from set-webpush"))?;
        if v["ok"] == true {
            Ok(Reply::new(&v["data"], "Web Push registered"))
        } else {
            Err(CliError::new(
                "internal",
                v["error"]["message"]
                    .as_str()
                    .unwrap_or("set-webpush failed")
                    .to_string(),
            ))
        }
    }
}

/// `LC_ALL`, then `LC_MESSAGES`, then `LANG`: the first one set decides.
pub fn lang_from(get: impl Fn(&str) -> Option<String>) -> Lang {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| get(k).filter(|v| !v.is_empty()))
        .unwrap_or_default();
    if locale.starts_with("es") {
        Lang::Es
    } else {
        Lang::En
    }
}

/// `update --check` on a background thread (a network call of up to 60 s),
/// so the TUI never waits for it.
fn check_updates(ctx: &Ctx) -> Receiver<Result<Value, String>> {
    let (tx, rx) = mpsc::channel();
    let c = Ctx {
        paths: ctx.paths.clone(),
        runner: ProcessRunner::default(),
        user: ctx.user.clone(),
        pane: None,
        exe: ctx.exe.clone(),
        path_env: ctx.path_env.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        migrated_dirs: Vec::new(),
    };
    std::thread::spawn(move || {
        let r = super::update::check(&c)
            .map(|u| serde_json::to_value(u).unwrap_or_default())
            .map_err(|e| e.message);
        let _ = tx.send(r);
    });
    rx
}

fn enter() -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)
}

fn leave() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
}

/// Restores the terminal before the default panic message is printed.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            leave();
            previous(info);
        }));
    });
}

/// Restores the terminal when dropped (normal exit and early returns).
struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        leave();
    }
}

fn io_err(e: std::io::Error) -> CliError {
    CliError::new("internal", format!("terminal error: {e}"))
}

pub fn run_tui(ctx: &Ctx) -> Result<(), CliError> {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Err(CliError::new(
            "invalid_input",
            "the TUI needs an interactive terminal",
        ));
    }
    // Ctrl+C while pairing (outside raw mode) burns the token instead of
    // killing the process.
    let stop = ctx.stop.clone();
    let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));

    install_panic_hook();
    enter().map_err(io_err)?;
    let _guard = Guard;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout())).map_err(io_err)?;
    let mut exec = CtxExec(ctx);
    let mut app = App::with_lang(lang_from(|k| std::env::var(k).ok()));
    let lang = app.lang;
    terminal.draw(|f| view::draw(f, &app)).map_err(io_err)?;
    app.start(&mut exec);
    let updates = check_updates(ctx);
    app.checking_updates();
    let mut repaint = true;
    loop {
        // Commands may log to stderr; repaint everything after one ran.
        if repaint {
            terminal.clear().map_err(io_err)?;
            repaint = false;
        }
        terminal.draw(|f| view::draw(f, &app)).map_err(io_err)?;
        if let Ok(r) = updates.try_recv() {
            app.on_update_check(r);
            continue;
        }
        if !event::poll(Duration::from_millis(250)).map_err(io_err)? {
            continue;
        }
        let Event::Key(key) = event::read().map_err(io_err)? else {
            continue;
        };
        repaint = true;
        match app.on_key(key, &mut exec) {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Pair { invert } => {
                leave();
                let r = pair(ctx, invert, lang);
                enter().map_err(io_err)?;
                app.after_pair(r);
                app.resume(&mut exec);
            }
            other => {
                let Some(cmd) = other.command() else { continue };
                leave();
                let r = suspended(ctx, cmd, lang);
                enter().map_err(io_err)?;
                app.after_suspended(r);
                app.resume(&mut exec);
            }
        }
    }
}

fn wait_for_enter(lang: Lang) {
    print!("{} ", t(lang, "Press Enter to return to the TUI."));
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

/// Runs `cmd` in the plain terminal while the TUI is suspended: its prompts,
/// sudo and installers talk to the person directly.
fn suspended(ctx: &Ctx, cmd: Command, lang: Lang) -> Result<Reply, CliError> {
    let r = super::run(ctx, cmd);
    match &r {
        Ok(reply) => println!("{}", reply.human),
        Err(e) => println!("falconer-hook: {}", e.message),
    }
    wait_for_enter(lang);
    r
}

/// The QR pairing flow, run in the plain terminal while the TUI is suspended.
fn pair(ctx: &Ctx, invert: bool, lang: Lang) -> Result<Reply, CliError> {
    ctx.stop.store(false, Ordering::SeqCst);
    let r = super::run(
        ctx,
        Command::Pair {
            alias: None,
            timeout: 300,
            invert,
            uri: false,
            yes: false,
        },
    );
    match &r {
        Ok(reply) => println!("{}", reply.human),
        Err(e) => println!("falconer-hook: {}", e.message),
    }
    wait_for_enter(lang);
    ctx.stop.store(false, Ordering::SeqCst);
    r
}
