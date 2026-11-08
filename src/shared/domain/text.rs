//! Sanitizing text that comes from other programs before it is shown.

/// Control, bidi and zero-width characters could spoof what a label says.
pub fn is_spoofing_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// `s` without spoofing characters, at most `max` characters long.
pub fn clean_label(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !is_spoofing_char(*c))
        .take(max)
        .collect()
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
        assert_eq!(project_of("/w/OpenMosh/").as_deref(), Some("OpenMosh"));
        assert_eq!(
            project_of("/w/Open\u{202E}Mosh\u{200B}\n").as_deref(),
            Some("OpenMosh")
        );
        assert_eq!(project_of("/"), None);
        assert_eq!(
            project_of(&format!("/w/{}", "a".repeat(60))).unwrap().len(),
            40
        );
    }
}
