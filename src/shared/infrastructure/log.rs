//! Redacting logger (control T-7).
//!
//! The API only accepts a static message plus typed fields, so payloads,
//! tokens, keys and terminal text cannot be logged by construction. Free text
//! that must be logged (e.g. an OS error) goes through [`redact`].

use std::fmt::Write as _;
use std::io::Write as _;

/// Field values that are safe to log.
pub enum Field<'a> {
    /// A static reason or state code.
    Code(&'static str),
    /// A count or number.
    Num(u64),
    /// A free-form string that is redacted before printing.
    Text(&'a str),
}

#[derive(Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Masks anything that looks like a secret: long base64/hex-like runs,
/// `key=value` style secrets and URLs (which may carry push topics).
pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (i, word) in input.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let token_like = word.len() >= 16
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_+/=.:".contains(c));
        let url = word.contains("://");
        if token_like || url {
            out.push_str("[redacted]");
        } else {
            out.extend(word.chars().filter(|c| !c.is_control()));
        }
    }
    out
}

fn render(level: Level, msg: &'static str, fields: &[(&'static str, Field<'_>)]) -> String {
    let lvl = match level {
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
    };
    let mut line = format!("falconer-hook {lvl}: {msg}");
    for (k, v) in fields {
        let _ = match v {
            Field::Code(c) => write!(line, " {k}={c}"),
            Field::Num(n) => write!(line, " {k}={n}"),
            Field::Text(t) => write!(line, " {k}=\"{}\"", redact(t)),
        };
    }
    line
}

/// Writes one line to stderr.
pub fn log(level: Level, msg: &'static str, fields: &[(&'static str, Field<'_>)]) {
    let line = render(level, msg, fields);
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_tokens_and_urls() {
        let s = redact("failed for https://ntfy.sh/upAbc token AAAAAAAAAAAAAAAAAAAAAAAA ok");
        assert_eq!(s, "failed for [redacted] token [redacted] ok");
    }

    #[test]
    fn renders_typed_fields_only() {
        let line = render(
            Level::Warn,
            "push failed",
            &[
                ("reason", Field::Code("timeout")),
                ("devices", Field::Num(2)),
                ("err", Field::Text("x\u{1b}[31m https://a.b/c")),
            ],
        );
        assert_eq!(
            line,
            "falconer-hook warn: push failed reason=timeout devices=2 err=\"x[31m [redacted]\""
        );
    }
}
