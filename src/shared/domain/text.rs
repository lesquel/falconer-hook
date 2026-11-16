//! Sanitizing text that comes from other programs before it is shown.

/// Control, bidi, zero-width and other invisible format characters could
/// spoof what a label says (e.g. a right-to-left override that reverses
/// `txt.exe`, or a zero-width space hiding inside a command).
pub fn is_spoofing_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

/// `s` without spoofing characters, at most `max` characters long.
pub fn clean_label(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !is_spoofing_char(*c))
        .take(max)
        .collect()
}

/// One line of display text: line breaks and tabs become spaces, runs of
/// spaces collapse, spoofing characters are removed, and text longer than
/// `max` characters is cut to `max` with a trailing `…` so the reader knows.
pub fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if is_spoofing_char(c) {
            continue;
        }
        if c == ' ' {
            if space || out.is_empty() {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(c);
    }
    let out = out.trim_end();
    if out.chars().count() <= max {
        return out.to_string();
    }
    let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// The project of a working directory: its last folder, cleaned and capped
/// at 40 characters. `None` for the root or an empty name.
pub fn project_of(cwd: &str) -> Option<String> {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|p| clean_label(p, 40))
        .filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_is_the_last_folder_without_spoofing_characters() {
        assert_eq!(project_of("/w/Falconer/").as_deref(), Some("Falconer"));
        assert_eq!(
            project_of("/w/Fal\u{202E}coner\u{200B}\n").as_deref(),
            Some("Falconer")
        );
        assert_eq!(project_of("/"), None);
        assert_eq!(
            project_of(&format!("/w/{}", "a".repeat(60))).unwrap().len(),
            40
        );
    }

    #[test]
    fn format_and_tag_characters_are_spoofing() {
        for c in [
            '\u{00AD}',
            '\u{061C}',
            '\u{2060}',
            '\u{2066}',
            '\u{2069}',
            '\u{FEFF}',
            '\u{E0041}',
            '\u{9B}',
        ] {
            assert!(is_spoofing_char(c), "{:X}", c as u32);
        }
        for c in ['a', 'ñ', '日', ' ', '…', '😀'] {
            assert!(!is_spoofing_char(c), "{c}");
        }
    }

    #[test]
    fn clip_makes_one_marked_line() {
        assert_eq!(clip("rm -rf\n\t/tmp/x  \u{202E}y", 200), "rm -rf /tmp/x y");
        assert_eq!(clip("  a  ", 10), "a");
        let long = "x".repeat(300);
        let c = clip(&long, 200);
        assert_eq!(c.chars().count(), 200);
        assert!(c.ends_with('…'));
        assert_eq!(clip(&"y".repeat(200), 200), "y".repeat(200));
        assert_eq!(clip("", 5), "");
    }
}
