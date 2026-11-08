use super::*;
use crate::app::tui::view::draw;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

/// Records every command and answers with canned data.
#[derive(Default)]
struct FakeExec {
    log: Vec<String>,
    complete: bool,
    empty_plan: bool,
}

impl Exec for FakeExec {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError> {
        let name = format!("{cmd:?}");
        self.log.push(name);
        Ok(match cmd {
            Command::Doctor { .. } => Reply::new(
                &json!({"healthy": false, "checks": [
                    {"name": "config", "status": "ok", "detail": "/h/.config/openmosh/config.toml"},
                    {"name": "tailscale", "status": "ok", "detail": "running as me@example.com"},
                    {"name": "herdr", "status": "warn", "detail": "not found"},
                    {"name": "service", "status": "fail", "detail": "not installed"}
                ]}),
                "",
            ),
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
            Command::Devices { cmd: DevicesCmd::List { .. } } => Reply::new(
                &json!({"devices": [{"id": "d1", "name": "Pixel", "endpoint_stale": true, "ssh_authorized": true}]}),
                "",
            ),
            Command::Integration { cmd: IntegrationCmd::List { .. } } => Reply::new(
                &json!({"agents": [
                    {"agent": "claude", "name": "Claude Code", "detected": true, "installed": true, "settings": "/h/.claude/settings.json"},
                    {"agent": "codex", "name": "Codex CLI", "detected": false, "installed": false, "settings": "/h/.codex/hooks.json"}
                ]}),
                "",
            ),
            Command::Uninstall { dry_run: true, .. } => Reply::new(
                &json!({"dry_run": true, "steps": if self.empty_plan { json!([]) } else { json!([{"service": "u"}]) }}),
                "openmosh-hook uninstall would:\n- remove the user service\n- delete /h/.config/openmosh",
            ),
            Command::Uninstall { .. } => Reply::new(
                &json!({"dry_run": false, "complete": self.complete}),
                "[ok  ] remove the user service\nopenmosh-hook was removed.",
            ),
            Command::Setup { .. } => Reply::new(&json!({}), "Wrote config\nNext: pair"),
            _ => Reply::new(&json!({}), "Done"),
        })
    }
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn started() -> (App, FakeExec) {
    let mut app = App::new();
    let mut ex = FakeExec::default();
    app.start(&mut ex);
    (app, ex)
}

fn dump(app: &App) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 22)).unwrap();
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

fn ran(ex: &FakeExec, prefix: &str) -> usize {
    ex.log.iter().filter(|l| l.starts_with(prefix)).count()
}

#[test]
fn starts_on_status_with_doctor_and_version() {
    let (app, ex) = started();
    assert_eq!(app.screen, Screen::Status);
    assert_eq!(ran(&ex, "Doctor"), 1);
    assert_eq!(ran(&ex, "Version"), 1);
    let s = dump(&app);
    assert!(s.contains("Version 0.1.0"), "{s}");
    assert!(s.contains("[FAIL] service"), "{s}");
    assert!(s.contains("tailscale"), "{s}");
}

#[test]
fn navigation_wraps_and_loads_each_screen_once() {
    let (mut app, mut ex) = started();
    app.on_key(key(KeyCode::BackTab), &mut ex);
    assert_eq!(app.screen, Screen::Uninstall);
    app.on_key(key(KeyCode::Tab), &mut ex);
    assert_eq!(app.screen, Screen::Status);
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
fn text_reply_is_typed_then_confirmed() {
    let (mut app, mut ex) = started();
    app.on_key(ch('2'), &mut ex);
    app.on_key(ch('t'), &mut ex);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "empty text is refused");
    app.on_key(ch('t'), &mut ex);
    for c in "yes q".chars() {
        app.on_key(ch(c), &mut ex);
    }
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

    app.on_key(ch('5'), &mut ex);
    app.on_key(ch('j'), &mut ex);
    app.on_key(ch('i'), &mut ex);
    app.on_key(key(KeyCode::Esc), &mut ex);
    assert_eq!(ran(&ex, "Integration { cmd: Install"), 0);
    app.on_key(ch('i'), &mut ex);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Integration { cmd: Install { agent: Codex"), 1);
    app.on_key(ch('k'), &mut ex);
    app.on_key(ch('u'), &mut ex);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Integration { cmd: Uninstall { agent: Claude"), 1);
}

#[test]
fn setup_runs_non_interactively_after_confirmation() {
    let (mut app, mut ex) = started();
    app.on_key(ch('6'), &mut ex);
    app.on_key(ch('a'), &mut ex);
    assert_eq!(ran(&ex, "Setup"), 0);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Setup { yes: true, no_service: false, out: JsonFlag { json: true } }"
        ),
        1
    );
    assert!(dump(&app).contains("Next: pair"));
}

#[test]
fn pair_suspends_the_tui() {
    let (mut app, mut ex) = started();
    app.on_key(ch('4'), &mut ex);
    app.on_key(ch('i'), &mut ex);
    assert_eq!(
        app.on_key(key(KeyCode::Enter), &mut ex),
        Effect::Pair { invert: true }
    );
    assert_eq!(ran(&ex, "Pair"), 0, "the terminal loop runs pairing");
    app.after_pair(Ok(Reply::new(&json!({}), "Paired Pixel")));
    assert_eq!(app.message.as_ref().unwrap().text, "Paired Pixel");
}

#[test]
fn uninstall_shows_the_plan_and_needs_the_typed_word() {
    let (mut app, mut ex) = started();
    app.on_key(ch('7'), &mut ex);
    assert_eq!(ran(&ex, "Uninstall { all: false, dry_run: true"), 1);
    assert!(dump(&app).contains("- remove the user service"));
    app.on_key(ch('a'), &mut ex);
    assert_eq!(ran(&ex, "Uninstall { all: true, dry_run: true"), 1);

    app.on_key(key(KeyCode::Enter), &mut ex);
    for c in "uninstal".chars() {
        app.on_key(ch(c), &mut ex);
    }
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "a wrong word cancels");
    assert_eq!(ran(&ex, "Uninstall { all: true, dry_run: false"), 0);

    app.on_key(key(KeyCode::Enter), &mut ex);
    for c in "uninstall".chars() {
        app.on_key(ch(c), &mut ex);
    }
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
    assert_eq!(ex.log.len(), before);
}

#[test]
fn empty_plan_cannot_be_confirmed() {
    let mut app = App::new();
    let mut ex = FakeExec {
        empty_plan: true,
        ..Default::default()
    };
    app.start(&mut ex);
    app.on_key(ch('7'), &mut ex);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal);
}

/// Prints the main screens (`cargo test tui_screens -- --nocapture`).
#[test]
fn tui_screens() {
    let (mut app, mut ex) = started();
    for c in ['1', '2', '5', '7'] {
        app.on_key(ch(c), &mut ex);
        println!("{}", dump(&app));
    }
    app.on_key(key(KeyCode::Enter), &mut ex);
    println!("{}", dump(&app));
}

#[test]
fn status_offers_ssh_hardening_in_the_terminal() {
    let (mut app, mut ex) = started();
    app.on_key(ch('1'), &mut ex);
    assert_eq!(app.on_key(ch('s'), &mut ex), Effect::HardenSsh);
    app.after_suspended(Ok(Reply::new(&json!({}), "sshd now: public key only.")));
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "sshd now: public key only."
    );
}
