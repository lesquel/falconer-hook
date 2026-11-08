//! Host X25519 static identity stored in `host.key` (`0600`, b64u text).

use std::fs;
use std::path::Path;
use zeroize::Zeroizing;

use super::crypto::{public_key, random_bytes};
use super::fs_secure::{check_private, FsError};
use crate::shared::domain::encoding::{b64u_decode_array, b64u_encode};

pub struct HostIdentity {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl HostIdentity {
    pub fn from_secret(secret: Zeroizing<[u8; 32]>) -> Self {
        let public = public_key(&secret);
        Self { secret, public }
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub fn secret(&self) -> Zeroizing<[u8; 32]> {
        self.secret.clone()
    }

    fn load(path: &Path) -> Result<Self, FsError> {
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
        Ok(Self::from_secret(secret))
    }

    /// Loads `path`, creating a fresh key on first run. Refuses a key file
    /// readable by group/others. Two processes starting at once (the service
    /// and a hook) agree on one key: the new key is written to a private
    /// temporary file and hard-linked into place, which fails if another
    /// process won; the loser then reads the winner's key.
    pub fn load_or_create(path: &Path) -> Result<Self, FsError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if path.exists() {
            return Self::load(path);
        }
        let io = |e: std::io::Error| FsError::Io(path.display().to_string(), e);
        let secret = random_bytes::<32>();
        let text = Zeroizing::new(format!("{}\n", b64u_encode(secret.as_slice())));
        let suffix = b64u_encode(random_bytes::<9>().as_slice());
        let tmp = path.with_extension(format!("tmp-{suffix}"));
        let written = (|| {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::hard_link(&tmp, path)
        })();
        let _ = fs::remove_file(&tmp);
        match written {
            Ok(()) => Ok(Self::from_secret(secret)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Self::load(path),
            Err(e) => Err(io(e)),
        }
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
    fn concurrent_first_runs_agree_on_one_key() {
        let t = tempfile::tempdir().unwrap();
        let p = std::sync::Arc::new(t.path().join("host.key"));
        let keys: Vec<[u8; 32]> = (0..8)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || HostIdentity::load_or_create(&p).unwrap().public())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        assert!(
            keys.iter().all(|k| *k == keys[0]),
            "every process uses the stored key"
        );
        assert_eq!(HostIdentity::load_or_create(&p).unwrap().public(), keys[0]);
        let left: Vec<_> = fs::read_dir(t.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left behind");
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
