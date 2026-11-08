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

/// The exact `authorized_keys` line for a paired phone (control T-3).
pub fn authorized_keys_line(normalized_key: &str, device_id: &str) -> String {
    format!(
        "from=\"100.64.0.0/10,fd7a:115c:a1e0::/48\",no-agent-forwarding,no-X11-forwarding {normalized_key} {}",
        device_comment(device_id)
    )
}

/// Device display name: control characters removed, trimmed, ≤ 64 chars.
pub fn sanitize_name(name: &str) -> Option<String> {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    Some(clean.chars().take(64).collect())
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
        assert_eq!(
            authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01"),
            format!("from=\"100.64.0.0/10,fd7a:115c:a1e0::/48\",no-agent-forwarding,no-X11-forwarding {KEY} openmosh:AAAAAAAAAAAAAAAAAAAA01")
        );
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
