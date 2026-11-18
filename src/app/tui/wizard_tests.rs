//! The first-run setup wizard.

use super::tests::*;
use super::*;

fn fresh() -> (App, FakeExec) {
    start_with(FakeExec {
        fresh: true,
        ..Default::default()
    })
}

#[test]
fn wizard_opens_on_first_run_only() {
    let (app, ex) = fresh();
    assert_eq!(app.wizard, Some(0));
    assert_eq!(ran(&ex, "Integration { cmd: List"), 1, "step 4 needs it");
    let s = dump(&app);
    assert!(s.contains("Setup wizard"), "{s}");
    assert!(s.contains("1/6"), "{s}");

    let (app, _) = started();
    assert_eq!(app.wizard, None, "a phone is paired");
    let (app, _) = start_with(FakeExec {
        fresh: true,
        ..Default::default()
    });
    assert!(app.wizard.is_some());
}

#[test]
fn wizard_steps_run_skip_and_advance() {
    let (mut app, mut ex) = fresh();
    // 1 Prerequisites: the runtime fix, asked first, run in the terminal.
    assert_eq!(app.wizard_step(), Some(WizardStep::Prereqs));
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(
        app.mode,
        Mode::Confirm(Pending::Fix {
            id: FixId::InstallRuntime
        })
    );
    assert_eq!(
        app.on_key(ch('y'), &mut ex),
        Effect::Fix {
            id: FixId::InstallRuntime
        }
    );
    app.on_key(ch('s'), &mut ex);
    // 2 SSH.
    assert_eq!(app.wizard_step(), Some(WizardStep::Ssh));
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(
        app.mode,
        Mode::Confirm(Pending::Fix {
            id: FixId::EnableSsh
        })
    );
    app.on_key(ch('n'), &mut ex);
    app.on_key(ch('s'), &mut ex);
    // 3 Tailscale.
    assert_eq!(app.wizard_step(), Some(WizardStep::Tailscale));
    app.on_key(ch('s'), &mut ex);
    // 4 Integrations: installs every detected one, then moves on by itself.
    assert_eq!(app.wizard_step(), Some(WizardStep::Integrations));
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(matches!(
        app.mode,
        Mode::Confirm(Pending::InstallDetected { .. })
    ));
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Integration { cmd: Install { agent: Codex, settings: None, out: JsonFlag { json: true } } }"
        ),
        1
    );
    assert_eq!(app.wizard_step(), Some(WizardStep::Service));
    // 5 Service.
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(
        app.mode,
        Mode::Confirm(Pending::Fix {
            id: FixId::InstallService
        })
    );
    app.on_key(key(KeyCode::Esc), &mut ex);
    assert_eq!(
        app.wizard_step(),
        Some(WizardStep::Service),
        "Esc only closes the dialog"
    );
    app.on_key(ch('s'), &mut ex);
    // 6 Pair.
    assert_eq!(app.wizard_step(), Some(WizardStep::Pair));
    assert_eq!(
        app.on_key(key(KeyCode::Enter), &mut ex),
        Effect::Pair { invert: false }
    );
    app.on_key(ch('s'), &mut ex);
    assert_eq!(app.wizard, None, "finished");
    assert_eq!(app.screen, Screen::Home);
}

#[test]
fn esc_leaves_the_wizard_for_home_and_home_reopens_it() {
    let (mut app, mut ex) = fresh();
    app.on_key(ch('s'), &mut ex);
    assert_eq!(app.on_key(key(KeyCode::Esc), &mut ex), Effect::None);
    assert_eq!(app.wizard, None);
    assert_eq!(app.screen, Screen::Home);
    // Keys work on Home again.
    app.on_key(ch('2'), &mut ex);
    assert_eq!(app.screen, Screen::Agents);

    let (mut app, mut ex) = started();
    app.on_key(ch('w'), &mut ex);
    assert_eq!(app.wizard, Some(0));
    app.on_key(key(KeyCode::Esc), &mut ex);
    let i = app
        .home_items()
        .iter()
        .position(|h| *h == HomeItem::Wizard)
        .unwrap();
    for _ in 0..i {
        app.on_key(ch('j'), &mut ex);
    }
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.wizard, Some(0));
}

#[test]
fn wizard_renders_at_any_size() {
    let (app, _ex) = fresh();
    println!("{}", dump_at(&app, 80, 24));
    for (w, h) in [(40, 12), (20, 6), (1, 1)] {
        dump_at(&app, w, h);
    }
}
