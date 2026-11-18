//! The `?` help overlay and the `/` action palette.

use super::tests::*;
use super::*;

#[test]
fn help_toggles_and_lists_global_and_screen_keys() {
    let (mut app, mut ex) = started();
    app.on_key(ch('?'), &mut ex);
    assert!(app.help);
    let s = dump(&app);
    assert!(s.contains("Search every action"), "{s}");
    assert!(s.contains("Fix the selected problem"), "{s}");
    // Keys do nothing else while it is open.
    app.on_key(ch('F'), &mut ex);
    assert_eq!(app.mode, Mode::Normal);
    app.on_key(ch('?'), &mut ex);
    assert!(!app.help);

    app.on_key(ch('3'), &mut ex);
    app.on_key(ch('?'), &mut ex);
    let s = dump(&app);
    assert!(s.contains("Revoke the selected phone"), "{s}");
    assert!(!s.contains("Fix the selected problem"), "{s}");
    assert_eq!(app.on_key(key(KeyCode::Esc), &mut ex), Effect::None);
    assert!(!app.help, "Esc closes it instead of quitting");
}

#[test]
fn help_in_the_wizard_lists_its_keys() {
    let (mut app, mut ex) = start_with(FakeExec {
        fresh: true,
        ..Default::default()
    });
    app.on_key(ch('?'), &mut ex);
    let s = dump(&app);
    assert!(s.contains("Skip this step"), "{s}");
    app.on_key(key(KeyCode::Esc), &mut ex);
    assert!(app.wizard.is_some(), "Esc closed only the help");
}

#[test]
fn palette_filters_and_runs_an_action_on_its_screen() {
    let (mut app, mut ex) = started();
    app.on_key(ch('/'), &mut ex);
    assert!(app.palette.is_some());
    let all = app.palette_matches().len();
    assert!(all > 20, "every action is listed: {all}");
    typed(&mut app, &mut ex, "revoke");
    let m = app.palette_matches();
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].act, Some(Act::Revoke));
    assert!(dump(&app).contains("Revoke the selected phone"));
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(app.palette.is_none());
    assert_eq!(app.screen, Screen::Phones);
    assert!(matches!(app.mode, Mode::Confirm(Pending::Revoke { .. })));
    app.on_key(ch('y'), &mut ex);
    assert_eq!(ran(&ex, "Devices { cmd: Revoke { id: \"d1\""), 1);
}

#[test]
fn palette_jumps_to_screens_and_matches_words_in_any_order() {
    let (mut app, mut ex) = started();
    app.on_key(ch('/'), &mut ex);
    typed(&mut app, &mut ex, "usage go");
    let m = app.palette_matches();
    assert_eq!(m[0].act, None, "{m:?}");
    assert_eq!(m[0].screen, Screen::Usage);
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert_eq!(app.screen, Screen::Usage);
    assert_eq!(ran(&ex, "Usage { days: 7,"), 1);

    // Down moves, Backspace edits, Esc closes without running anything.
    app.on_key(ch('/'), &mut ex);
    typed(&mut app, &mut ex, "servicex");
    assert!(app.palette_matches().is_empty());
    app.on_key(key(KeyCode::Enter), &mut ex);
    assert!(app.palette.is_some(), "nothing to run");
    app.on_key(key(KeyCode::Backspace), &mut ex);
    assert!(app.palette_matches().len() >= 3);
    app.on_key(key(KeyCode::Down), &mut ex);
    assert_eq!(app.palette.as_ref().unwrap().sel, 1);
    let before = ex.log.len();
    app.on_key(key(KeyCode::Esc), &mut ex);
    assert!(app.palette.is_none());
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(ex.log.len(), before);
}

#[test]
fn palette_hides_actions_that_cannot_run_now() {
    let (mut app, mut ex) = started();
    app.on_key(ch('/'), &mut ex);
    let acts: Vec<_> = app.palette_matches().iter().filter_map(|e| e.act).collect();
    assert!(!acts.contains(&Act::Update), "no update available");
    assert!(!acts.contains(&Act::LegacyCleanup), "no leftovers");
    assert!(!acts.contains(&Act::Activate));
    assert!(acts.contains(&Act::Setup));
}

#[test]
fn footer_names_help_and_search() {
    let (app, _ex) = started();
    let s = dump_at(&app, 80, 24);
    assert!(s.contains("/ search"), "{s}");
    assert!(s.contains("? help"), "{s}");
}

#[test]
fn overlays_render_at_any_size() {
    let (mut app, mut ex) = started();
    app.on_key(ch('?'), &mut ex);
    println!("{}", dump_at(&app, 80, 24));
    for (w, h) in [(40, 12), (1, 1)] {
        dump_at(&app, w, h);
    }
    app.on_key(ch('?'), &mut ex);
    app.on_key(ch('/'), &mut ex);
    typed(&mut app, &mut ex, "pa");
    println!("{}", dump_at(&app, 80, 24));
    for (w, h) in [(40, 12), (1, 1)] {
        dump_at(&app, w, h);
    }
}
