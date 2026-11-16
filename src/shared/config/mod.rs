//! XDG paths and `config.toml` loading.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;
use crate::shared::infrastructure::fs_secure::{check_private_file_if_exists, FsError};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("HOME is not set")]
    NoHome,
    #[error("invalid config.toml: {0}")]
    Parse(String),
    #[error(transparent)]
    Fs(#[from] FsError),
}

impl ErrorCode for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            ConfigError::NoHome | ConfigError::Parse(_) => "config_error",
            ConfigError::Fs(e) => e.code(),
        }
    }
}

/// Every filesystem location the hook touches. Built once in `main.rs` from
/// the environment and injected; no other module reads `$HOME`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Where OpenMosh (the previous name) kept the config; moved on upgrade.
    pub legacy_config_dir: PathBuf,
    /// Where OpenMosh kept the state (devices, host key, inbox).
    pub legacy_state_dir: PathBuf,
}

/// Directory and service names used before the rename to Falconer. Only the
/// upgrade path and `doctor` read them.
pub const LEGACY_DIR_NAME: &str = "openmosh";
pub const LEGACY_SYSTEMD_UNIT: &str = "openmosh-hook.service";
pub const LEGACY_LAUNCHD_PLIST: &str = "dev.openmosh.hook.plist";

impl Paths {
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let home = get("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or(ConfigError::NoHome)?;
        let abs = |v: Option<String>| v.filter(|s| s.starts_with('/')).map(PathBuf::from);
        let xdg_config_home = abs(get("XDG_CONFIG_HOME")).unwrap_or_else(|| home.join(".config"));
        let xdg_state_home =
            abs(get("XDG_STATE_HOME")).unwrap_or_else(|| home.join(".local/state"));
        Ok(Self {
            config_dir: xdg_config_home.join("falconer"),
            state_dir: xdg_state_home.join("falconer"),
            legacy_config_dir: xdg_config_home.join(LEGACY_DIR_NAME),
            legacy_state_dir: xdg_state_home.join(LEGACY_DIR_NAME),
            xdg_config_home,
            home,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn events_db(&self) -> PathBuf {
        self.state_dir.join("events.db")
    }
    pub fn devices_file(&self) -> PathBuf {
        self.state_dir.join("devices.toml")
    }
    pub fn host_key(&self) -> PathBuf {
        self.state_dir.join("host.key")
    }
    pub fn authorized_keys(&self) -> PathBuf {
        self.home.join(".ssh/authorized_keys")
    }
    pub fn systemd_unit(&self) -> PathBuf {
        self.xdg_config_home
            .join("systemd/user/falconer-hook.service")
    }
    pub fn launchd_plist(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents/dev.falconer.hook.plist")
    }
    pub fn legacy_systemd_unit(&self) -> PathBuf {
        self.xdg_config_home
            .join("systemd/user")
            .join(LEGACY_SYSTEMD_UNIT)
    }
    pub fn legacy_launchd_plist(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(LEGACY_LAUNCHD_PLIST)
    }

    /// Moves each OpenMosh directory to its Falconer name when only the old
    /// one exists. A rename keeps every file (pairing keys, inbox) as is;
    /// nothing is deleted, and an existing Falconer directory is never
    /// touched. Returns the directories moved.
    pub fn migrate_legacy_dirs(&self) -> Vec<(PathBuf, PathBuf)> {
        let mut moved = Vec::new();
        for (old, new) in [
            (&self.legacy_config_dir, &self.config_dir),
            (&self.legacy_state_dir, &self.state_dir),
        ] {
            let is_dir = std::fs::symlink_metadata(old).is_ok_and(|m| m.is_dir());
            if is_dir
                && std::fs::symlink_metadata(new).is_err()
                && std::fs::rename(old, new).is_ok()
            {
                moved.push((old.clone(), new.clone()));
            }
        }
        moved
    }

    /// OpenMosh directories still present (both names exist).
    pub fn legacy_dirs_left(&self) -> Vec<&Path> {
        [&self.legacy_config_dir, &self.legacy_state_dir]
            .into_iter()
            .filter(|p| p.exists())
            .map(PathBuf::as_path)
            .collect()
    }
}

/// `config.toml` written by `setup` (and `install.sh`) when none exists.
pub const DEFAULT_CONFIG: &str = r#"# falconer-hook configuration. Keep this file mode 0600.
# host_alias = "my-laptop"   # name shown on the phone (default: hostname)
retention_days = 7
poll_interval_ms = 1000
sources = ["herdr", "tmux"]
ssh_port = 22

[push]
# Push transport suggested to the phone. ntfy.sh only sees ciphertext;
# a self-hosted ntfy inside your tailnet also works.
server = "https://ntfy.sh"
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PushConfig {
    /// Default ntfy server suggested to the phone; endpoints are per device.
    pub server: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            server: "https://ntfy.sh".into(),
        }
    }
}

/// External programs. Plain names are resolved through `PATH`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Bins {
    pub herdr: String,
    pub tmux: String,
    pub tailscale: String,
    pub systemctl: String,
    pub launchctl: String,
}

impl Default for Bins {
    fn default() -> Self {
        Self {
            herdr: "herdr".into(),
            tmux: "tmux".into(),
            tailscale: "tailscale".into(),
            systemctl: "systemctl".into(),
            launchctl: "launchctl".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Name shown on the phone. Defaults to the hostname.
    pub host_alias: Option<String>,
    pub retention_days: u32,
    pub poll_interval_ms: u64,
    /// Enabled agent sources: `herdr`, `tmux`.
    pub sources: Vec<String>,
    pub ssh_port: u16,
    pub ssh_host_key_pub: PathBuf,
    pub push: PushConfig,
    pub bins: Bins,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host_alias: None,
            retention_days: 7,
            poll_interval_ms: 1000,
            sources: vec!["herdr".into(), "tmux".into()],
            ssh_port: 22,
            ssh_host_key_pub: PathBuf::from("/etc/ssh/ssh_host_ed25519_key.pub"),
            push: PushConfig::default(),
            bins: Bins::default(),
        }
    }
}

impl Config {
    pub fn source_enabled(&self, name: &str) -> bool {
        self.sources.iter().any(|s| s == name)
    }

    /// Loads `path`; a missing file yields defaults. A file readable by
    /// group/others is refused (control T-8).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        check_private_file_if_exists(path)?;
        let text = std::fs::read_to_string(path)
            .map_err(|e| FsError::Io(path.display().to_string(), e))?;
        let cfg: Config =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(e.message().to_string()))?;
        if cfg.poll_interval_ms < 200 || cfg.retention_days == 0 {
            return Err(ConfigError::Parse(
                "poll_interval_ms must be >= 200 and retention_days >= 1".into(),
            ));
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn default_config_parses() {
        let c: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.retention_days, 7);
        assert_eq!(c.push.server, "https://ntfy.sh");
    }

    #[test]
    fn xdg_paths_follow_env_and_fallbacks() {
        let p = Paths::from_env(|k| match k {
            "HOME" => Some("/h".into()),
            "XDG_STATE_HOME" => Some("/s".into()),
            "XDG_CONFIG_HOME" => Some("relative-is-ignored".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(p.state_dir, PathBuf::from("/s/falconer"));
        assert_eq!(p.config_dir, PathBuf::from("/h/.config/falconer"));
        assert_eq!(
            p.authorized_keys(),
            PathBuf::from("/h/.ssh/authorized_keys")
        );
        assert!(Paths::from_env(|_| None).is_err());
    }

    #[test]
    fn missing_config_is_default_and_loose_config_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("config.toml");
        assert_eq!(Config::load(&f).unwrap(), Config::default());
        std::fs::write(
            &f,
            "host_alias = \"box\"\n[push]\nserver = \"https://n.example\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(Config::load(&f).unwrap_err().code(), "permissions_too_open");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        let c = Config::load(&f).unwrap();
        assert_eq!(c.host_alias.as_deref(), Some("box"));
        assert_eq!(c.retention_days, 7);
        assert_eq!(c.push.server, "https://n.example");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("config.toml");
        std::fs::write(&f, "typo = 1\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Config::load(&f).unwrap_err().code(), "config_error");
    }
}
