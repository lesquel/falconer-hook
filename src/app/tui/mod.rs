//! Interactive terminal UI over the same commands as the CLI.
//!
//! Presentation only: `state` turns keys into `cli::Command`s, `view` draws
//! the state, and this module owns the terminal (raw mode, alternate screen,
//! restore on exit and on panic) and runs each command through `app::run`.

pub mod state;
pub mod view;

use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::Ordering;
use std::sync::Once;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::cli::Command;
use state::{App, Effect, Exec};

/// Runs commands through the composition root, exactly like the CLI.
struct CtxExec<'a>(&'a Ctx);

impl Exec for CtxExec<'_> {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError> {
        super::run(self.0, cmd)
    }
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
    let mut app = App::new();
    terminal.draw(|f| view::draw(f, &app)).map_err(io_err)?;
    app.start(&mut exec);
    loop {
        // Commands may log to stderr; repaint everything.
        terminal.clear().map_err(io_err)?;
        terminal.draw(|f| view::draw(f, &app)).map_err(io_err)?;
        let Event::Key(key) = event::read().map_err(io_err)? else {
            continue;
        };
        match app.on_key(key, &mut exec) {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Pair { invert } => {
                leave();
                let r = pair(ctx, invert);
                enter().map_err(io_err)?;
                app.after_pair(r);
            }
            Effect::HardenSsh => {
                leave();
                let r = super::harden::harden_ssh(ctx, false, false, false);
                if let Err(e) = &r {
                    println!("openmosh-hook: {}", e.message);
                }
                print!("Press Enter to return to the TUI. ");
                let _ = std::io::stdout().flush();
                let _ = std::io::stdin().lock().read_line(&mut String::new());
                enter().map_err(io_err)?;
                app.after_suspended(r);
            }
        }
    }
}

/// The QR pairing flow, run in the plain terminal while the TUI is suspended.
fn pair(ctx: &Ctx, invert: bool) -> Result<Reply, CliError> {
    ctx.stop.store(false, Ordering::SeqCst);
    let r = super::run(
        ctx,
        Command::Pair {
            alias: None,
            timeout: 300,
            invert,
            uri: false,
        },
    );
    match &r {
        Ok(reply) => println!("{}", reply.human),
        Err(e) => println!("openmosh-hook: {}", e.message),
    }
    print!("Press Enter to return to the TUI. ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    ctx.stop.store(false, Ordering::SeqCst);
    r
}
