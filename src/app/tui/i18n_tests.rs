//! Spanish strings, chosen from the locale.

use super::super::i18n::{has, t, Lang};
use super::tests::*;
use super::*;

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        vars.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn locale_precedence_is_lc_all_then_lc_messages_then_lang() {
    assert_eq!(Lang::from_env(env(&[("LANG", "es_EC.UTF-8")])), Lang::Es);
    assert_eq!(Lang::from_env(env(&[("LANG", "es")])), Lang::Es);
    assert_eq!(Lang::from_env(env(&[("LANG", "en_US.UTF-8")])), Lang::En);
    assert_eq!(Lang::from_env(env(&[])), Lang::En);
    assert_eq!(
        Lang::from_env(env(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "es_ES.UTF-8")])),
        Lang::En
    );
    assert_eq!(
        Lang::from_env(env(&[("LC_ALL", "es_MX.UTF-8"), ("LANG", "en_US.UTF-8")])),
        Lang::Es
    );
    assert_eq!(
        Lang::from_env(env(&[
            ("LC_ALL", ""),
            ("LC_MESSAGES", "es_AR.UTF-8"),
            ("LANG", "en_US.UTF-8")
        ])),
        Lang::Es,
        "an empty LC_ALL does not decide"
    );
    assert_eq!(
        Lang::from_env(env(&[("LC_MESSAGES", "C"), ("LANG", "es_ES.UTF-8")])),
        Lang::En
    );
}

#[test]
fn every_label_has_a_spanish_translation() {
    let mut missing = Vec::new();
    for s in Screen::ALL {
        missing.extend([s.tab(), s.title()].into_iter().filter(|x| !has(x)));
    }
    for d in super::super::actions::ACTIONS {
        missing.extend(
            [d.label, d.hint]
                .into_iter()
                .filter(|x| !x.is_empty() && !has(x)),
        );
    }
    for s in WizardStep::ALL {
        missing.extend([s.title(), s.about()].into_iter().filter(|x| !has(x)));
    }
    for id in FixId::ALL {
        let es = fix_title(Lang::Es, id);
        if es == fix_title(Lang::En, id) {
            missing.push(es);
        }
    }
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn home_footer_and_wizard_in_spanish() {
    let mut app = App::with_lang(Lang::Es);
    let mut ex = FakeExec::default();
    app.start(&mut ex);
    let s = dump_at(&app, 80, 24);
    for want in [
        "Falconer ─ Inicio",
        "Salud",
        "Acciones rápidas",
        "Instalar lo que falta",
        "Emparejar un teléfono",
        "→ Instalar Herdr o tmux",
        "/ buscar",
        "? ayuda",
        "8 Avanzado",
    ] {
        assert!(s.contains(want), "{want}: {s}");
    }
    for unwanted in ["Health", "Quick actions", "search", "help", "quit"] {
        assert!(!s.contains(unwanted), "{unwanted}: {s}");
    }
    app.on_key(ch('?'), &mut ex);
    assert!(dump(&app).contains("Buscar en todas las acciones"));
    app.on_key(ch('?'), &mut ex);
    app.on_key(ch('F'), &mut ex);
    assert!(dump(&app).contains("Cada arreglo muestra su comando exacto"));

    let mut app = App::with_lang(Lang::Es);
    let mut ex = FakeExec {
        fresh: true,
        ..Default::default()
    };
    app.start(&mut ex);
    let s = dump_at(&app, 80, 24);
    println!("{s}");
    assert!(s.contains("Asistente de configuración"), "{s}");
    assert!(s.contains("Paso 1/6"), "{s}");
    assert!(s.contains("s omite este paso"), "{s}");
}

#[test]
fn spanish_renders_every_screen_without_english_hints() {
    let mut app = App::with_lang(Lang::Es);
    let mut ex = FakeExec::default();
    app.start(&mut ex);
    for c in ['1', '2', '3', '4', '5', '6', '7', '8'] {
        app.on_key(ch(c), &mut ex);
        let s = dump_at(&app, 100, 30);
        assert!(!s.contains(" refresh"), "{s}");
        dump_at(&app, 40, 12);
    }
    assert_eq!(t(Lang::Es, "not a known string"), "not a known string");
}
