//! SSH host key fingerprint in `ssh-keygen -lf` format: `SHA256:<b64 std, no padding>`.

use sha2::{Digest, Sha256};

use crate::shared::domain::encoding::{b64_std_decode, b64_std_nopad};

/// Computes the fingerprint of an OpenSSH public key line (`type base64 [comment]`).
pub fn fingerprint(pub_line: &str) -> Option<String> {
    let mut parts = pub_line.split_whitespace();
    let _kind = parts.next()?;
    let blob = b64_std_decode(parts.next()?)?;
    Some(format!("SHA256:{}", b64_std_nopad(&Sha256::digest(&blob))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ssh_keygen_format() {
        // Expected value printed by `ssh-keygen -lf` for this throwaway key.
        let line =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH test";
        assert_eq!(
            fingerprint(line).unwrap(),
            "SHA256:XmA4TG6/WNqReEVbojfXfv3nyHYvdvEr6WbFjs7VdLM"
        );
    }

    #[test]
    fn malformed_line_is_none() {
        assert!(fingerprint("ssh-ed25519").is_none());
        assert!(fingerprint("ssh-ed25519 !!!").is_none());
    }
}
