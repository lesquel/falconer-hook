//! Pairing validation rules.

use crate::shared::domain::encoding::{b64_std_decode, b64u_decode_array};

const ED25519: &str = "ssh-ed25519";

/// Validates an OpenSSH `ssh-ed25519` public key line and returns it
/// normalized to `ssh-ed25519 <base64>` (the phone's comment is dropped so
/// nothing attacker-controlled reaches `authorized_keys` besides the key).
pub fn normalize_ssh_ed25519(line: &str) -> Option<String> {
    if line.len() > 1024 || line.contains(['\n', '\r', '\0']) {
        return None;
    }
    let mut parts = line.split_ascii_whitespace();
    if parts.next()? != ED25519 {
        return None;
    }
    let b64 = parts.next()?;
    let blob = b64_std_decode(b64)?;
    // string "ssh-ed25519" || string key(32)
    let mut expected = Vec::with_capacity(51);
    expected.extend_from_slice(&(ED25519.len() as u32).to_be_bytes());
    expected.extend_from_slice(ED25519.as_bytes());
    expected.extend_from_slice(&32u32.to_be_bytes());
    if blob.len() != expected.len() + 32 || blob[..expected.len()] != expected[..] {
        return None;
    }
    Some(format!("{ED25519} {b64}"))
}

/// Comment that tags a device's line in `authorized_keys`.
pub fn device_comment(device_id: &str) -> String {
    format!("openmosh:{device_id}")
}

/// `from=` for devices paired before addresses were pinned: the whole tailnet.
pub const LEGACY_FROM: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";

/// The exact `authorized_keys` line for a paired phone (control T-3).
///
/// `restrict` turns off every forwarding, `~/.ssh/rc` and any option a future
/// OpenSSH adds; `pty` gives the interactive terminal back. `from=` pins the
/// phone's own tailnet addresses (`peer_ips`), or the whole tailnet for
/// devices paired before addresses were recorded. Addresses are re-parsed, so
/// nothing but IP literals reaches the line.
pub fn authorized_keys_line(normalized_key: &str, device_id: &str, peer_ips: &[String]) -> String {
    let ips: Vec<String> = peer_ips
        .iter()
        .filter_map(|ip| ip.parse::<std::net::IpAddr>().ok())
        .map(|ip| ip.to_string())
        .collect();
    let from = if ips.is_empty() {
        LEGACY_FROM.to_string()
    } else {
        ips.join(",")
    };
    format!(
        "restrict,pty,from=\"{from}\" {normalized_key} {}",
        device_comment(device_id)
    )
}

/// Maximum device name length, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// Unicode format characters (general category `Cf`): bidi controls,
/// zero-width characters, BOM, soft hyphen, invisible operators and tags.
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{0600}'..='\u{0605}'
        | '\u{061C}'
        | '\u{06DD}'
        | '\u{070F}'
        | '\u{0890}'..='\u{0891}'
        | '\u{08E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}'
        | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}'
        | '\u{E0020}'..='\u{E007F}')
}

/// Device display name: control (`Cc`), format (`Cf`) and line/paragraph
/// separator (`Zl`, `Zp`) characters removed, trimmed, ≤ 64 characters.
pub fn sanitize_name(name: &str) -> Option<String> {
    let clean: String = name
        .chars()
        .filter(|&c| !(c.is_control() || is_format(c) || c == '\u{2028}' || c == '\u{2029}'))
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    let capped: String = clean.chars().take(MAX_NAME_CHARS).collect();
    Some(capped.trim_end().to_string())
}

/// A push public key is a b64u 32-byte X25519 key.
pub fn valid_ppk(ppk: &str) -> bool {
    b64u_decode_array::<32>(ppk).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH";

    #[test]
    fn valid_key_is_normalized_without_comment() {
        assert_eq!(
            normalize_ssh_ed25519(&format!("{KEY} phone@x")).as_deref(),
            Some(KEY)
        );
        assert_eq!(
            normalize_ssh_ed25519(&format!("{KEY} ")).as_deref(),
            Some(KEY)
        );
    }

    #[test]
    fn invalid_keys_are_rejected() {
        let rsa = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ";
        for bad in [
            "",
            "ssh-ed25519",
            rsa,
            "ssh-ed25519 !!!!",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXe",
            &format!("{KEY}\ncommand=\"sh\" ssh-ed25519 AAAA"),
            // Valid base64 of a blob with the wrong type string.
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE4AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH",
        ] {
            assert!(normalize_ssh_ed25519(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn authorized_keys_line_exact_format() {
        let ips = ["100.101.1.2".to_string(), "fd7a:115c:a1e0::1:2".to_string()];
        assert_eq!(
            authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01", &ips),
            format!("restrict,pty,from=\"100.101.1.2,fd7a:115c:a1e0::1:2\" {KEY} openmosh:AAAAAAAAAAAAAAAAAAAA01")
        );
        // Devices paired before addresses were recorded keep the tailnet range.
        assert_eq!(
            authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01", &[]),
            format!("restrict,pty,from=\"100.64.0.0/10,fd7a:115c:a1e0::/48\" {KEY} openmosh:AAAAAAAAAAAAAAAAAAAA01")
        );
        // Only IP literals reach the line.
        let evil = ["1.2.3.4\",command=\"sh".to_string()];
        assert!(authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01", &evil)
            .starts_with("restrict,pty,from=\"100.64.0.0/10"));
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(
            sanitize_name(" Pixel\u{1b}[2J 8 \n").as_deref(),
            Some("Pixel[2J 8")
        );
        assert!(sanitize_name("\n\t ").is_none());
        assert_eq!(sanitize_name(&"x".repeat(100)).unwrap().len(), 64);
    }

    #[test]
    fn names_lose_format_and_separator_characters() {
        // Bidi overrides/isolates, zero-width, BOM, soft hyphen, line/paragraph separators.
        let tricky = "\u{202E}Pix\u{202A}el\u{2066}\u{2069}\u{200B}\u{200D}\u{200F} \u{FEFF}8\u{00AD}\u{2028}\u{2029}\u{2060}\u{E0041}";
        assert_eq!(sanitize_name(tricky).as_deref(), Some("Pixel 8"));
        assert!(sanitize_name("\u{202E}\u{200B}\u{2028}").is_none());
        // Ordinary non-ASCII text is kept; the cap counts characters, not bytes.
        assert_eq!(
            sanitize_name("Teléfono ñ 日本").as_deref(),
            Some("Teléfono ñ 日本")
        );
        assert_eq!(sanitize_name(&"é".repeat(100)).unwrap().chars().count(), 64);
    }

    #[test]
    fn ppk_must_be_32_bytes() {
        assert!(valid_ppk(&crate::shared::domain::encoding::b64u_encode(
            &[1; 32]
        )));
        assert!(!valid_ppk(&crate::shared::domain::encoding::b64u_encode(
            &[1; 31]
        )));
        assert!(!valid_ppk("not base64!"));
    }
}
