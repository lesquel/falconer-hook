//! Host X25519 static identity stored in `host.key` (`0600`, b64u text).

use std::fs;
use std::path::Path;
use zeroize::Zeroizing;

use super::crypto::{public_key, random_bytes};
use super::fs_secure::{check_private, write_atomic, FsError};
use crate::shared::domain::encoding::{b64u_decode_array, b64u_encode};

pub struct HostIdentity {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl HostIdentity {
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = public_key(&secret);
        Self {
            secret: Zeroizing::new(secret),
            public,
        }
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub fn secret(&self) -> Zeroizing<[u8; 32]> {
        self.secret.clone()
    }

    /// Loads `path`, creating a fresh key on first run. Refuses a key file
    /// readable by group/others.
    pub fn load_or_create(path: &Path) -> Result<Self, FsError> {
        if path.exists() {
            check_private(path, 0o600)?;
            let text = Zeroizing::new(
                fs::read_to_string(path).map_err(|e| FsError::Io(path.display().to_string(), e))?,
            );
            let secret = b64u_decode_array::<32>(text.trim()).ok_or_else(|| {
                FsError::Io(
                    path.display().to_string(),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed host key"),
                )
            })?;
            return Ok(Self::from_secret(secret));
        }
        let secret = random_bytes::<32>();
        let text = Zeroizing::new(format!("{}\n", b64u_encode(&secret)));
        write_atomic(path, text.as_bytes(), 0o600)?;
        Ok(Self::from_secret(secret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn created_once_then_reloaded() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("host.key");
        let a = HostIdentity::load_or_create(&p).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let b = HostIdentity::load_or_create(&p).unwrap();
        assert_eq!(a.public(), b.public());
    }

    #[test]
    fn loose_key_file_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("host.key");
        HostIdentity::load_or_create(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(HostIdentity::load_or_create(&p).is_err());
    }
}
