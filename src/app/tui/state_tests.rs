use super::*;
use crate::app::tui::view::draw;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

/// Records every command and answers with canned data.
#[derive(Default)]
pub(super) struct FakeExec {
    pub log: Vec<String>,
    pub complete: bool,
    pub empty_plan: bool,
    /// Doctor reports a first run: no config, nothing installed, no phone.
    pub fresh: bool,
    /// Doctor also reports OpenMosh leftovers.
    pub legacy: bool,
    /// Codex's integration was installed through this fake.
    pub codex_installed: bool,
}

fn fix(id: &str) -> serde_json::Value {
    json!({"id": id, "description": format!("fix {id}")})
}

impl FakeExec {
    fn checks(&self) -> serde_json::Value {
        if self.fresh {
            return json!([
                {"name": "config", "status": "fail", "detail": "no config"},
                {"name": "tailscale", "status": "fail", "detail": "not installed", "fix": fix("install_tailscale")},
                {"name": "herdr", "status": "warn", "detail": "not found", "fix": fix("install_runtime")},
                {"name": "sshd", "status": "fail", "detail": "nothing on :22", "fix": fix("enable_ssh")},
                {"name": "devices", "status": "warn", "detail": "no paired phone", "fix": fix("pair")},
                {"name": "service", "status": "fail", "detail": "not installed", "fix": fix("install_service")}
            ]);
        }
        let mut c = json!([
            {"name": "config", "status": "ok", "detail": "/h/.config/falconer/config.toml"},
            {"name": "tailscale", "status": "ok", "detail": "running as me@example.com"},
            {"name": "herdr", "status": "warn", "detail": "not found", "fix": fix("install_runtime")},
            {"name": "permissions", "status": "warn", "detail": "config dir is 0755", "fix": fix("fix_permissions")},
            {"name": "devices", "status": "ok", "detail": "1 paired"},
            {"name": "service", "status": "fail", "detail": "not installed", "fix": fix("install_service")}
        ]);
        if self.legacy {
            c.as_array_mut()
                .unwrap()
                .push(json!({"name": "legacy", "status": "warn", "detail": "OpenMosh leftovers"}));
        }
        c
    }
}

impl Exec for FakeExec {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError> {
        let name = format!("{cmd:?}");
        self.log.push(name);
        Ok(match cmd {
            Command::Doctor { fix: Some(_), .. } => Reply::new(
                &json!({"healthy": false, "checks": self.checks(), "fixes": []}),
                "Made private: /h/.config/falconer\n\n[ok  ] config",
            ),
            Command::Doctor { .. } => {
                Reply::new(&json!({"healthy": false, "checks": self.checks()}), "")
            }
            Command::Version { .. } => Reply::new(&json!({"version": "0.1.0"}), ""),
            Command::Agents { .. } => Reply::new(
                &json!({"agents": [{"pane_id": "w1:p1", "agent": "claude", "status": "blocked", "title": "Fix tests"}]}),
                "",
            ),
            Command::Inbox { .. } => Reply::new(
                &json!({"events": [
                    {"id": "e1", "seq": 1, "agent": "codex", "kind": "done", "status": "resolved", "title": "Build api"},
                    {"id": "e2", "seq": 2, "agent": "claude", "kind": "approval_required", "status": "pending", "title": "Run \u{1b}[2Jcargo test"}
                ], "last_seq": 2}),
                "",
            ),
            Command::Devices {
                cmd: DevicesCmd::List { .. },
            } => Reply::new(
                &json!({"devices": [{"id": "d1", "name": "Pixel", "endpoint": "https://ntfy.example/t1", "endpoint_stale": true, "ssh_authorized": true, "webpush": false}]}),
                "",
            ),
            Command::Integration {
                cmd: IntegrationCmd::List { .. },
            } => Reply::new(
                &json!({"agents": [
                    {"agent": "claude", "name": "Claude Code", "detected": true, "installed": true, "settings": "/h/.claude/settings.json"},
                    {"agent": "codex", "name": "Codex CLI", "detected": self.fresh, "installed": self.codex_installed, "settings": "/h/.codex/hooks.json"}
                ]}),
                "",
            ),
            Command::Integration {
                cmd: IntegrationCmd::Install {
                    agent: AgentArg::Codex,
                    ..
                },
            } => {
                self.codex_installed = true;
                Reply::new(&json!({}), "Installed the Codex CLI integration")
            }
            Command::Uninstall { dry_run: true, .. } => Reply::new(
                &json!({"dry_run": true, "steps": if self.empty_plan { json!([]) } else { json!([{"service": "u"}]) }}),
                "falconer-hook uninstall would:\n- remove the user service\n- delete /h/.config/falconer",
            ),
            Command::Uninstall { .. } => Reply::new(
                &json!({"dry_run": false, "complete": self.complete}),
                "[ok  ] remove the user service\nfalconer-hook was removed.",
            ),
            Command::Setup { .. } => Reply::new(&json!({}), "Wrote config\nNext: pair"),
            Command::Config { .. } => Reply::new(
                &json!({"file": "/h/.config/falconer/config.toml"}),
                "# /h/.config/falconer/config.toml\nhost_alias = \"devbox\"\nssh_port = 22",
            ),
            Command::Usage { days, .. } => {
                Reply::new(&json!({}), format!("Codex        {days} days  1.2M tokens"))
            }
            Command::Service { .. } => Reply::new(&json!({}), "run    systemctl --user"),
            _ => Reply::new(&json!({}), "Done"),
        })
    }

    fn exec_stdin(&mut self, cmd: Command, input: &str) -> Result<Reply, CliError> {
        self.log.push(format!("{cmd:?} <<< {input}"));
        Ok(Reply::new(&json!({}), "Web Push registered"))
    }
}

pub(super) fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

pub(super) fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

pub(super) fn typed(app: &mut App, ex: &mut FakeExec, s: &str) {
    for c in s.chars() {
        app.on_key(ch(c), ex);
    }
}

pub(super) fn started() -> (App, FakeExec) {
    start_with(FakeExec::default())
}

pub(super) fn start_with(mut ex: FakeExec) -> (App, FakeExec) {
    let mut app = App::new();
    app.start(&mut ex);
    (app, ex)
}

pub(super) fn dump_at(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

pub(super) fn dump(app: &App) -> String {
    dump_at(app, 100, 30)
}

pub(super) fn ran(ex: &FakeExec, prefix: &str) -> usize {
    ex.log.iter().filter(|l| l.starts_with(prefix)).count()
}

#[test]
fn starts_on_home_with_doctor_and_version() {
    let (app, ex) = started();
    assert_eq!(app.screen, Screen::Home);
    assert_eq!(ran(&ex, "Doctor"), 1);
    assert_eq!(ran(&ex, "Version"), 1);
    let s = dump(&app);
    assert!(s.contains("Version 0.1.0"), "{s}");
    assert!(s.contains("✗ service"), "{s}");
    assert!(s.contains("tailscale"), "{s}");
}

#[test]
fn navigation_wraps_and_loads_each_screen_once() {
    let (mut app, mut ex) = started();
    app.on_key(key(KeyCode::BackTab), &mut ex);
    assert_eq!(app.screen, Screen::Advanced);
    app.on_key(key(KeyCode::Tab), &mut ex);
    assert_eq!(app.screen, Screen::Home);
    app.on_key(ch('2'), &mut ex);
    app.on_key(ch('1'), &mut ex);
    app.on_key(ch('2'), &mut ex);
    assert_eq!(app.screen, Screen::Agents);
    assert_eq!(ran(&ex, "Inbox"), 1, "cached after the first visit");
    app.on_key(ch('r'), &mut ex);
    assert_eq!(ran(&ex, "Inbox"), 2, "r refreshes");
    assert_eq!(app.on_key(ch('q'), &mut ex), Effect::Quit);
}

#[test]
fn respond_requires_confirmation_and_a_pending_event() {
    let (mut app, mut ex) = started();
    app.on_key(ch('2'), &mut ex);
    // Newest first: the pending e2, then the resolved e1.
    assert_eq!(app.events[0].id, "e2");
    assert!(
        !app.events[0].title.contains('\u{1b}'),
        "escape sequences are stripped"
    );
    app.on_key(ch('a'), &mut ex);
    assert!(matches!(
        app.mode,
        Mode::Confirm(Pending::Respond {
            action: ActionArg::Allow,
            ..
        })
    ));
    assert_eq!(ran(&ex, "Respond"), 0, "nothing runs before y");
    app.on_key(ch('n'), &mut ex);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(ran(&ex, "Respond"), 0);

    app.on_key(ch('d'), &mut ex);
    app.on_key(ch('x'), &mut ex); // ignored while confirming
    assert!(matches!(app.mode, Mode::Confirm(_)));
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Respond { event_id: \"e2\", action: Deny"), 1);

    app.on_key(ch('j'), &mut ex);
    app.on_key(ch('a'), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "resolved events cannot be answered");
    assert!(app.message.as_ref().unwrap().error);
}

#[test]
fn allow_issues_the_same_respond_command() {
    let (mut app, mut ex) = started();
    app.on_key(ch('2'), &mut ex);
    app.on_key(ch('a'), &mut ex);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Respond { event_id: \"e2\", action: Allow, text: None, out: JsonFlag { json: true } }"
        ),
        1
    );
}

#[test]
fn text_reply_is_typed_then_confirmed() {
    let (mut app, mut ex) = started();
    app.on_key(ch('2'), &mut ex);
    app.on_key(ch('t'), &mut ex);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "empty text is refused");
    app.on_key(ch('t'), &mut ex);
    typed(&mut app, &mut ex, "yes q");
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(
        matches!(&app.mode, Mode::Confirm(Pending::Respond { text: Some(t), .. }) if t == "yes q")
    );
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Respond { event_id: \"e2\", action: Text, text: Some(\"yes q\")"
        ),
        1
    );
}

#[test]
fn revoke_and_integrations_need_confirmation() {
    let (mut app, mut ex) = started();
    app.on_key(ch('3'), &mut ex);
    assert!(dump(&app).contains("(endpoint stale)"));
    app.on_key(ch('x'), &mut ex);
    assert_eq!(ran(&ex, "Devices { cmd: Revoke"), 0);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Devices { cmd: Revoke { id: \"d1\""), 1);

    app.on_key(ch('4'), &mut ex);
    app.on_key(ch('j'), &mut ex);
    app.on_key(ch('i'), &mut ex);
    app.on_key(key(KeyCode::Esc), &mut ex);
    assert_eq!(ran(&ex, "Integration { cmd: Install"), 0);
    app.on_key(ch('i'), &mut ex);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Integration { cmd: Install { agent: Codex, settings: None, out: JsonFlag { json: true } } }"
        ),
        1
    );
    app.on_key(ch('k'), &mut ex);
    app.on_key(ch('u'), &mut ex);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Integration { cmd: Uninstall { agent: Claude"), 1);
}

/// Selects the Advanced list entry `act`.
pub(super) fn select_advanced(app: &mut App, ex: &mut FakeExec, act: Act) {
    app.on_key(ch('8'), ex);
    let i = app.advanced_items().iter().position(|a| *a == act).unwrap();
    for _ in 0..8 {
        app.on_key(ch('k'), ex);
    }
    for _ in 0..i {
        app.on_key(ch('j'), ex);
    }
}

#[test]
fn setup_runs_non_interactively_after_confirmation() {
    let (mut app, mut ex) = started();
    select_advanced(&mut app, &mut ex, Act::Setup);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(ran(&ex, "Setup"), 0);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Setup { yes: true, no_service: false, out: JsonFlag { json: true } }"
        ),
        0
    );
    assert_eq!(
        ran(
            &ex,
            "Setup { yes: false, no_service: false, out: JsonFlag { json: true } }"
        ),
        1
    );
    assert!(dump(&app).contains("Next: pair"));
}

#[test]
fn setup_with_every_integration_runs_in_the_terminal() {
    // `setup --yes` may now run official installers and sudo: it needs the
    // real terminal, so the TUI steps aside after the confirmation.
    let (mut app, mut ex) = started();
    select_advanced(&mut app, &mut ex, Act::SetupAll);
    app.on_key(key(KeyCode::Enter), &mut ex);
    let q = dump(&app);
    assert!(q.contains("https://herdr.dev/install.sh"), "{q}");
    let eff = app.on_key(ch('y'), &mut ex);
    assert_eq!(eff, Effect::SetupAll);
    assert_eq!(
        format!("{:?}", eff.command().unwrap()),
        "Setup { yes: true, no_service: false, out: JsonFlag { json: false } }"
    );
    assert_eq!(ran(&ex, "Setup"), 0, "the terminal loop runs it");
}

#[test]
fn pair_suspends_the_tui() {
    let (mut app, mut ex) = started();
    app.on_key(ch('3'), &mut ex);
    app.on_key(ch('i'), &mut ex);
    assert_eq!(app.on_key(ch('p'), &mut ex), Effect::Pair { invert: true });
    assert_eq!(ran(&ex, "Pair"), 0, "the terminal loop runs pairing");
    app.after_pair(Ok(Reply::new(&json!({}), "Paired Pixel")));
    assert_eq!(app.message.as_ref().unwrap().text, "Paired Pixel");
}

#[test]
fn uninstall_shows_the_plan_and_needs_the_typed_word() {
    let (mut app, mut ex) = started();
    app.on_key(ch('8'), &mut ex);
    assert_eq!(ran(&ex, "Uninstall { all: false, dry_run: true"), 1);
    assert!(dump(&app).contains("- remove the user service"));
    app.on_key(ch('a'), &mut ex);
    assert_eq!(ran(&ex, "Uninstall { all: true, dry_run: true"), 1);

    app.on_key(ch('u'), &mut ex);
    typed(&mut app, &mut ex, "uninstal");
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "a wrong word cancels");
    assert_eq!(ran(&ex, "Uninstall { all: true, dry_run: false"), 0);

    select_advanced(&mut app, &mut ex, Act::Uninstall);
    app.on_key(key(KeyCode::Enter), &mut ex);
    typed(&mut app, &mut ex, "uninstall");
    assert!(dump(&app).contains("> uninstall"));
    ex.complete = true;
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(
        ran(&ex, "Uninstall { all: true, dry_run: false, yes: true"),
        1
    );
    assert!(app.uninstalled);

    // Nothing else runs afterwards: it would recreate state.
    let before = ex.log.len();
    app.on_key(ch('1'), &mut ex);
    app.on_key(ch('r'), &mut ex);
    app.on_key(ch('3'), &mut ex);
    app.on_key(ch('x'), &mut ex);
    assert_eq!(app.on_key(ch('p'), &mut ex), Effect::None);
    assert_eq!(ex.log.len(), before);
}

#[test]
fn empty_plan_cannot_be_confirmed() {
    let (mut app, mut ex) = start_with(FakeExec {
        empty_plan: true,
        ..Default::default()
    });
    app.on_key(ch('8'), &mut ex);
    app.on_key(ch('u'), &mut ex);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn ssh_hardening_runs_in_the_terminal() {
    let (mut app, mut ex) = started();
    app.on_key(ch('8'), &mut ex);
    let eff = app.on_key(ch('s'), &mut ex);
    assert_eq!(eff, Effect::HardenSsh);
    assert_eq!(
        format!("{:?}", eff.command().unwrap()),
        "HardenSsh { dry_run: false, yes: false, out: JsonFlag { json: false } }"
    );
    app.after_suspended(Ok(Reply::new(&json!({}), "sshd now: public key only.")));
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "sshd now: public key only."
    );
}

/// Prints every screen (`cargo test tui_screens -- --nocapture`).
#[test]
fn tui_screens() {
    let (mut app, mut ex) = started();
    for c in ['1', '2', '3', '4', '5', '6', '7', '8'] {
        app.on_key(ch(c), &mut ex);
        println!("{}", dump_at(&app, 80, 24));
    }
}
