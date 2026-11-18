//! Self-update decisions: release tags, semantic version ordering and the
//! signed reinstall through the tag-pinned install.sh. Nothing here runs a
//! program or touches the network.

use serde::Serialize;
use std::cmp::Ordering;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

pub const REPO: &str = "lesquel/falconer-hook";

/// Redirects to `…/releases/tag/<latest tag>`. GitHub never points it at a
/// prerelease, but the comparison below does not rely on that.
pub fn latest_release_url() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// install.sh pinned to the release tag, so the script that verifies the
/// signature is the one published with that release.
pub fn script_url(tag: &str) -> String {
    format!("https://raw.githubusercontent.com/{REPO}/{tag}/install.sh")
}

/// The same rule install.sh applies to `--version`: `v<digit>…` made of
/// `[0-9A-Za-z.+-]`, since the tag goes into a URL and an argv.
pub fn valid_tag(tag: &str) -> bool {
    let mut b = tag.bytes();
    b.next() == Some(b'v')
        && b.next().is_some_and(|c| c.is_ascii_digit())
        && tag
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".+-".contains(&c))
        && tag.len() <= 64
}

/// `https://github.com/<repo>/releases/tag/v1.2.3` → `v1.2.3`.
pub fn tag_from_release_url(url: &str) -> Option<String> {
    let url = url.trim();
    let (prefix, tag) = url.rsplit_once('/')?;
    (prefix.ends_with("/releases/tag") && valid_tag(tag)).then(|| tag.to_string())
}

/// A semantic version (`MAJOR.MINOR.PATCH[-PRE][+BUILD]`); build metadata
/// is ignored, as semver says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    /// Parses `1.2.3`, `v1.2.3-rc.1` or `1.2.3+build`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split_once('+').map_or(s, |(v, _)| v);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut nums = core.split('.').map(|n| {
            (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                .then(|| n.parse::<u64>().ok())
                .flatten()
        });
        let core = [nums.next()??, nums.next()??, nums.next()??];
        if nums.next().is_some() {
            return None;
        }
        let pre: Vec<String> = match pre {
            None => Vec::new(),
            Some(p) => {
                let ids: Vec<String> = p.split('.').map(str::to_string).collect();
                if ids.iter().any(|i| {
                    i.is_empty() || !i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                }) {
                    return None;
                }
                ids
            }
        };
        Some(Self { core, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

fn cmp_ident(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        // Numeric identifiers sort before alphanumeric ones.
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A prerelease sorts before its release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let o = cmp_ident(a, b);
                        if o != Ordering::Equal {
                            return o;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether `latest` should be offered to a host running `current`: it must
/// be newer, and a prerelease is never offered to a stable version.
pub fn update_available(current: &Version, latest: &Version) -> bool {
    latest > current && (current.is_prerelease() || !latest.is_prerelease())
}

/// The result of `update --check` (and the start of `update`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateCheck {
    /// The running version, e.g. `0.2.0`.
    pub current: String,
    /// The latest release's version without the `v`, e.g. `0.2.1`.
    pub latest: String,
    /// The latest release's tag, e.g. `v0.2.1`.
    pub tag: String,
    pub update_available: bool,
}

impl UpdateCheck {
    pub fn new(current: &str, tag: &str) -> Result<Self, UpdateError> {
        let cur = Version::parse(current)
            .ok_or_else(|| UpdateError::Invalid(format!("bad running version {current}")))?;
        let lat = Version::parse(tag).ok_or_else(|| UpdateError::Unresolved(tag.to_string()))?;
        Ok(Self {
            current: current.to_string(),
            latest: tag.trim_start_matches('v').to_string(),
            tag: tag.to_string(),
            update_available: update_available(&cur, &lat),
        })
    }

    /// `Up to date (v0.2.0)` or `Update available: v0.2.0 → v0.2.1`.
    pub fn summary(&self) -> String {
        if self.update_available {
            format!("Update available: v{} → {}", self.current, self.tag)
        } else {
            format!("Up to date (v{})", self.current)
        }
    }
}

/// install.sh arguments for a reinstall of `tag`: the release path (which
/// verifies the signed SHA256SUMS), the offers to install missing Herdr or
/// Tailscale accepted, and the service left alone when none is installed.
pub fn installer_args(tag: &str, has_service: bool) -> Vec<String> {
    let mut v = vec![
        "--version".to_string(),
        tag.to_string(),
        "--yes".to_string(),
    ];
    if !has_service {
        v.push("--no-service".to_string());
    }
    v
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UpdateError {
    #[error("curl or wget is required to check for updates")]
    NoDownloader,
    #[error("could not resolve the latest release tag ({0}); check your connection or pass --version <tag>")]
    Unresolved(String),
    #[error("could not download {0}; check your connection")]
    Download(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

impl ErrorCode for UpdateError {
    fn code(&self) -> &'static str {
        match self {
            Self::NoDownloader | Self::Unresolved(_) | Self::Download(_) => "unavailable",
            Self::Invalid(_) => "invalid_input",
            Self::Failed(_) => "update_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn parses_versions_and_rejects_garbage() {
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert!(v("1.0.0-rc.1").is_prerelease());
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.x.3",
            "1.2.3-",
            "1.2.3-a..b",
            "-1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn orders_by_semver_precedence() {
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "1.10.0",
            "2.0.0",
        ];
        for w in chain.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
    }

    #[test]
    fn prereleases_are_offered_only_to_prereleases() {
        assert!(update_available(&v("0.2.0"), &v("0.2.1")));
        assert!(!update_available(&v("0.2.0"), &v("0.2.0")));
        assert!(!update_available(&v("0.2.1"), &v("0.2.0")));
        assert!(!update_available(&v("0.2.0"), &v("0.3.0-rc.1")));
        assert!(update_available(&v("0.3.0-rc.1"), &v("0.3.0-rc.2")));
        assert!(update_available(&v("0.3.0-rc.1"), &v("0.3.0")));
    }

    #[test]
    fn tags_follow_the_install_sh_rule() {
        assert!(valid_tag("v0.2.1"));
        assert!(valid_tag("v1.0.0-rc.1+b"));
        for bad in ["0.2.1", "v", "vx", "v1;rm", "v1 2", "v1/../x", "latest"] {
            assert!(!valid_tag(bad), "{bad}");
        }
        assert_eq!(
            tag_from_release_url("https://github.com/lesquel/falconer-hook/releases/tag/v0.2.1\n"),
            Some("v0.2.1".into())
        );
        assert_eq!(
            tag_from_release_url("https://github.com/lesquel/falconer-hook/releases/latest"),
            None
        );
    }

    #[test]
    fn summaries_and_installer_args() {
        let c = UpdateCheck::new("0.2.0", "v0.2.1").unwrap();
        assert_eq!(c.summary(), "Update available: v0.2.0 → v0.2.1");
        assert_eq!(c.latest, "0.2.1");
        let c = UpdateCheck::new("0.2.0", "v0.2.0").unwrap();
        assert_eq!(c.summary(), "Up to date (v0.2.0)");
        assert_eq!(
            installer_args("v0.2.1", false),
            ["--version", "v0.2.1", "--yes", "--no-service"]
        );
        assert_eq!(
            installer_args("v0.2.1", true),
            ["--version", "v0.2.1", "--yes"]
        );
    }
}
