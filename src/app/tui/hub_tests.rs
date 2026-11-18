//! The hub: Home health and fixes, the update banner and the screens
//! added around the existing ones.

use super::tests::*;
use super::*;
use serde_json::json;

fn select_home(app: &mut App, ex: &mut FakeExec, item: HomeItem) {
    app.on_key(ch('1'), ex);
    let i = app.home_items().iter().position(|a| *a == item).unwrap();
    for _ in 0..12 {
        app.on_key(ch('k'), ex);
    }
    for _ in 0..i {
        app.on_key(ch('j'), ex);
    }
}

#[test]
fn home_lists_problems_with_their_fix_and_quick_actions() {
    let (app, _ex) = started();
    let s = dump(&app);
    assert!(s.contains("! herdr"), "{s}");
    assert!(s.contains("→ Install Herdr or tmux"), "{s}");
    assert!(s.contains("✓ 3 OK: config, tailscale, devices"), "{s}");
    assert!(s.contains("Install what's missing"), "{s}");
    assert!(s.contains("Pair a phone"), "{s}");
    assert!(!s.contains("Update Falconer"), "{s}");
}

#[test]
fn home_fix_asks_with_the_url_then_runs_in_the_terminal() {
    let (mut app, mut ex) = started();
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(
        app.mode,
        Mode::Confirm(Pending::Fix {
            id: FixId::InstallRuntime
        })
    );
    assert!(dump(&app).contains("https://herdr.dev/install.sh"));
    let eff = app.on_key(ch('y'), &mut ex);
    assert_eq!(
        eff,
        Effect::Fix {
            id: FixId::InstallRuntime
        }
    );
    assert_eq!(
        format!("{:?}", eff.command().unwrap()),
        "Doctor { fix: Some(Some(\"install_runtime\")), yes: true, out: JsonFlag { json: false } }"
    );
    assert_eq!(
        ran(&ex, "Doctor { fix: Some"),
        0,
        "the terminal loop runs it"
    );
    app.after_suspended(Ok(Reply::new(&json!({}), "Installed Herdr\nmore")));
    app.resume(&mut ex);
    assert_eq!(app.message.as_ref().unwrap().text, "Installed Herdr");
    assert_eq!(ran(&ex, "Doctor { fix: None"), 2, "health is read again");
}

#[test]
fn home_fix_declined_runs_nothing() {
    let (mut app, mut ex) = started();
    app.on_key(ch('f'), &mut ex);
    assert_eq!(app.on_key(ch('n'), &mut ex), Effect::None);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn permissions_fix_runs_inside_the_tui() {
    let (mut app, mut ex) = started();
    select_home(&mut app, &mut ex, HomeItem::Check(3));
    app.on_key(ch('f'), &mut ex);
    assert_eq!(app.on_key(ch('y'), &mut ex), Effect::None);
    assert_eq!(
        ran(
            &ex,
            "Doctor { fix: Some(Some(\"fix_permissions\")), yes: true, out: JsonFlag { json: true } }"
        ),
        1
    );
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Made private: /h/.config/falconer"
    );
}

#[test]
fn fix_all_lists_the_fixes_and_asks_each_in_the_terminal() {
    let (mut app, mut ex) = started();
    app.on_key(ch('F'), &mut ex);
    assert_eq!(
        app.mode,
        Mode::Confirm(Pending::FixAll {
            ids: vec![
                FixId::InstallRuntime,
                FixId::FixPermissions,
                FixId::InstallService
            ]
        })
    );
    let eff = app.on_key(ch('y'), &mut ex);
    assert_eq!(eff, Effect::FixAll);
    assert_eq!(
        format!("{:?}", eff.command().unwrap()),
        "Doctor { fix: Some(None), yes: false, out: JsonFlag { json: false } }"
    );
    // The same from the quick action.
    select_home(&mut app, &mut ex, HomeItem::FixAll);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(matches!(app.mode, Mode::Confirm(Pending::FixAll { .. })));
}

#[test]
fn pair_quick_action_suspends_for_pairing() {
    let (mut app, mut ex) = started();
    select_home(&mut app, &mut ex, HomeItem::Pair);
    assert_eq!(
        app.on_key(key(KeyCode::Enter), &mut ex),
        Effect::Pair { invert: false }
    );
}

#[test]
fn update_banner_only_when_an_update_is_available() {
    let (mut app, mut ex) = started();
    app.checking_updates();
    assert!(dump(&app).contains("Checking for updates"));
    assert!(!app.home_items().contains(&HomeItem::Update));

    app.on_update_check(Ok(
        json!({"current": "0.2.0", "latest": "0.2.0", "tag": "v0.2.0", "update_available": false}),
    ));
    let s = dump(&app);
    assert!(!s.contains("Update Falconer"), "{s}");
    assert!(s.contains("Up to date"), "{s}");

    app.on_update_check(Err("network down".into()));
    let s = dump(&app);
    assert!(!s.contains("Update Falconer"), "{s}");
    assert!(app.message.is_none(), "a failed check is not an error");
    assert_eq!(app.on_key(ch('u'), &mut ex), Effect::None);

    app.on_update_check(Ok(
        json!({"current": "0.2.0", "latest": "0.2.1", "tag": "v0.2.1", "update_available": true}),
    ));
    let s = dump(&app);
    assert!(s.contains("Update Falconer (v0.2.1 available)"), "{s}");
    select_home(&mut app, &mut ex, HomeItem::Update);
    app.on_key(key(KeyCode::Enter), &mut ex);
    let s = dump(&app);
    assert!(
        s.contains("https://raw.githubusercontent.com/lesquel/falconer-hook/v0.2.1/install.sh"),
        "{s}"
    );
    let eff = app.on_key(ch('y'), &mut ex);
    assert_eq!(
        eff,
        Effect::Update {
            tag: "v0.2.1".into()
        }
    );
    assert_eq!(
        format!("{:?}", eff.command().unwrap()),
        "Update { check: false, version: Some(\"v0.2.1\"), yes: true, out: JsonFlag { json: false } }"
    );
}

#[test]
fn phones_set_and_clear_web_push() {
    let (mut app, mut ex) = started();
    app.on_key(ch('3'), &mut ex);
    app.on_key(ch('w'), &mut ex);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.mode, Mode::Normal, "empty input is refused");
    app.on_key(ch('w'), &mut ex);
    typed(&mut app, &mut ex, "{\"endpoint\":\"x\"}");
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(matches!(
        app.mode,
        Mode::Confirm(Pending::SetWebpush { .. })
    ));
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Devices { cmd: SetWebpush { id: \"d1\", out: JsonFlag { json: true } } } <<< {\"endpoint\":\"x\"}"
        ),
        1
    );
    app.on_key(ch('c'), &mut ex);
    assert_eq!(ran(&ex, "Devices { cmd: ClearWebpush"), 0);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Devices { cmd: ClearWebpush { id: \"d1\", out: JsonFlag { json: true } } }"
        ),
        1
    );
}

#[test]
fn service_install_restart_and_uninstall() {
    let (mut app, mut ex) = started();
    app.on_key(ch('5'), &mut ex);
    assert!(dump(&app).contains("not installed"));
    for (k, cmd) in [
        (
            'i',
            "Service { cmd: Install { dry_run: false, out: JsonFlag { json: true } } }",
        ),
        (
            's',
            "Service { cmd: Restart { out: JsonFlag { json: true } } }",
        ),
        (
            'u',
            "Service { cmd: Uninstall { dry_run: false, out: JsonFlag { json: true } } }",
        ),
    ] {
        app.on_key(ch(k), &mut ex);
        assert_eq!(ran(&ex, cmd), 0, "{k} asks first");
        app.on_key(ch('y'), &mut ex);
        assert_eq!(ran(&ex, cmd), 1, "{k}");
    }
    assert!(dump(&app).contains("run    systemctl --user"));
}

#[test]
fn settings_show_the_config_and_set_a_push_endpoint() {
    let (mut app, mut ex) = started();
    app.on_key(ch('6'), &mut ex);
    assert_eq!(ran(&ex, "Config { out: JsonFlag { json: true } }"), 1);
    let s = dump(&app);
    assert!(s.contains("host_alias = \"devbox\""), "{s}");
    assert!(s.contains("https://ntfy.example/t1"), "{s}");
    app.on_key(ch('e'), &mut ex);
    typed(&mut app, &mut ex, "https://ntfy.example/t2");
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(ran(&ex, "Devices { cmd: SetEndpoint"), 0);
    app.on_key(ch('y'), &mut ex);
    assert_eq!(
        ran(
            &ex,
            "Devices { cmd: SetEndpoint { id: \"d1\", url: \"https://ntfy.example/t2\", out: JsonFlag { json: true } } }"
        ),
        1
    );
}

#[test]
fn usage_range_is_selectable() {
    let (mut app, mut ex) = started();
    app.on_key(ch('7'), &mut ex);
    assert_eq!(ran(&ex, "Usage { days: 7,"), 1);
    assert!(dump(&app).contains("7 days  1.2M tokens"));
    app.on_key(ch('+'), &mut ex);
    assert_eq!(ran(&ex, "Usage { days: 14,"), 1);
    app.on_key(ch('+'), &mut ex);
    app.on_key(ch('+'), &mut ex);
    assert_eq!(ran(&ex, "Usage { days: 31,"), 1, "stops at 31");
    for _ in 0..4 {
        app.on_key(ch('-'), &mut ex);
    }
    assert_eq!(ran(&ex, "Usage { days: 1,"), 1, "stops at 1");
}

#[test]
fn legacy_cleanup_only_when_doctor_reports_leftovers() {
    let (mut app, mut ex) = started();
    assert!(!app.advanced_items().contains(&Act::LegacyCleanup));
    let (mut app2, mut ex2) = start_with(FakeExec {
        legacy: true,
        ..Default::default()
    });
    assert!(app2.advanced_items().contains(&Act::LegacyCleanup));
    select_advanced(&mut app2, &mut ex2, Act::LegacyCleanup);
    app2.on_key(key(KeyCode::Enter), &mut ex2);
    app2.on_key(ch('y'), &mut ex2);
    assert_eq!(
        ran(
            &ex2,
            "Setup { yes: false, no_service: false, out: JsonFlag { json: true } }"
        ),
        1
    );
    app.on_key(ch('8'), &mut ex);
}

#[test]
fn every_screen_renders_on_small_terminals() {
    let (mut app, mut ex) = started();
    for c in ['1', '2', '3', '4', '5', '6', '7', '8'] {
        app.on_key(ch(c), &mut ex);
        for (w, h) in [(80, 24), (40, 12), (20, 6), (1, 1)] {
            dump_at(&app, w, h);
        }
    }
    app.on_key(ch('u'), &mut ex);
    dump_at(&app, 40, 12);
}
