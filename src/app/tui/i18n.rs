//! TUI strings. The English text is the key; other languages map it to
//! their translation and fall back to English when one is missing.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Es,
}

/// The translation of the English text `en`.
pub fn t(lang: Lang, en: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Es => ES.iter().find(|(k, _)| *k == en).map_or(en, |(_, es)| es),
    }
}

/// `t` with each `{}` replaced by the next of `args`.
pub fn tf(lang: Lang, en: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut parts = t(lang, en).split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for p in parts {
        out.push_str(args.next().copied().unwrap_or_default());
        out.push_str(p);
    }
    out
}

/// English → Spanish.
static ES: &[(&str, &str)] = &[];
