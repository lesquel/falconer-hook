//! Adapters for `update`: resolving the latest tag and downloading the
//! tag-pinned install.sh with curl or wget (HTTPS only, the same way
//! install.sh does), a private temp directory, and running the script.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::domain::{latest_release_url, tag_from_release_url, UpdateError};
use crate::shared::domain::command::CommandRunner;
use crate::shared::infrastructure::system::on_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Downloader {
    Curl,
    Wget,
}

impl Downloader {
    /// curl first, as in install.sh.
    pub fn detect(path_env: Option<&str>) -> Option<Self> {
        if on_path("curl", path_env) {
            Some(Self::Curl)
        } else if on_path("wget", path_env) {
            Some(Self::Wget)
        } else {
            None
        }
    }

    /// The download command, as argv.
    pub fn download_argv(self, url: &str, dest: &Path) -> Vec<String> {
        let dest = dest.display().to_string();
        let a: Vec<&str> = match self {
            Self::Curl => vec![
                "curl",
                "-fsSL",
                "--proto",
                "=https",
                "--tlsv1.2",
                "-o",
                &dest,
                url,
            ],
            Self::Wget => vec!["wget", "-q", "--https-only", "-O", &dest, url],
        };
        a.into_iter().map(String::from).collect()
    }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// The latest release tag, from the redirect of `/releases/latest`.
pub fn resolve_latest(runner: &dyn CommandRunner, d: Downloader) -> Result<String, UpdateError> {
    let url = latest_release_url();
    let unresolved = |why: &str| UpdateError::Unresolved(why.to_string());
    let effective = match d {
        Downloader::Curl => {
            let out = runner
                .run(
                    "curl",
                    &args(&[
                        "-fsSLI",
                        "--proto",
                        "=https",
                        "--tlsv1.2",
                        "-o",
                        "/dev/null",
                        "-w",
                        "%{url_effective}",
                        &url,
                    ]),
                )
                .map_err(|e| unresolved(&e.to_string()))?;
            if !out.success() {
                return Err(unresolved(&format!("curl exited with {}", out.status)));
            }
            out.stdout
        }
        Downloader::Wget => {
            // Not following the redirect makes wget exit non-zero; the
            // Location header is what matters.
            let out = runner
                .run(
                    "wget",
                    &args(&[
                        "-q",
                        "--https-only",
                        "--max-redirect=0",
                        "-S",
                        "-O",
                        "/dev/null",
                        &url,
                    ]),
                )
                .map_err(|e| unresolved(&e.to_string()))?;
            out.stderr
                .lines()
                .find_map(|l| {
                    let l = l.trim();
                    l.strip_prefix("Location:")
                        .or_else(|| l.strip_prefix("location:"))
                })
                .unwrap_or_default()
                .trim()
                .to_string()
        }
    };
    tag_from_release_url(&effective).ok_or_else(|| unresolved("no release tag in the redirect"))
}

/// Downloads `url` to `dest` (a file already created with mode 0600).
pub fn download(
    runner: &dyn CommandRunner,
    d: Downloader,
    url: &str,
    dest: &Path,
) -> Result<(), UpdateError> {
    let argv = d.download_argv(url, dest);
    let out = runner
        .run(&argv[0], &argv[1..])
        .map_err(|_| UpdateError::Download(url.to_string()))?;
    let size = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    if !out.success() || size == 0 {
        return Err(UpdateError::Download(url.to_string()));
    }
    // Downloaders may recreate the file; keep it private either way.
    fs::set_permissions(dest, fs::Permissions::from_mode(0o600))
        .map_err(|e| UpdateError::Failed(format!("could not secure the download: {e}")))?;
    Ok(())
}

/// A private (0700) directory under `$TMPDIR`, removed on drop.
pub struct PrivateTemp {
    dir: PathBuf,
}

impl PrivateTemp {
    pub fn new() -> Result<Self, UpdateError> {
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = base.join(format!(
            "falconer-update-{}-{nanos:08x}",
            std::process::id()
        ));
        // `create`, not `create_all`: an existing path (planted by someone
        // else) is an error, never reused.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| UpdateError::Failed(format!("could not create a temp directory: {e}")))?;
        Ok(Self { dir })
    }

    /// Creates an empty file `name` with mode 0600.
    pub fn file(&self, name: &str) -> Result<PathBuf, UpdateError> {
        let p = self.dir.join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
            .map_err(|e| UpdateError::Failed(format!("could not create a temp file: {e}")))?;
        Ok(p)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Runs `sh <script> <args>` on the user's terminal with
/// `FALCONER_BIN_DIR=<bin_dir>`. With `stdout_to_stderr` (JSON output) the
/// script's stdout goes to stderr so the envelope stays the only stdout.
pub fn run_installer(
    script: &Path,
    args: &[String],
    bin_dir: &Path,
    stdout_to_stderr: bool,
) -> Result<i32, UpdateError> {
    let mut c = Command::new("sh");
    c.arg(script)
        .args(args)
        .env("FALCONER_BIN_DIR", bin_dir)
        // An update replaces falconer-hook only: never offer Herdr/Tailscale installs.
        .env("FALCONER_SKIP_PREREQS", "1");
    if stdout_to_stderr {
        use std::os::fd::AsFd;
        let fd = std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| UpdateError::Failed(format!("could not redirect output: {e}")))?;
        c.stdout(Stdio::from(fd));
    }
    let status = c
        .status()
        .map_err(|e| UpdateError::Failed(format!("could not run sh: {e}")))?;
    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;
    use crate::shared::domain::command::CommandOutput;

    #[test]
    fn temp_dir_and_file_are_private_and_removed() {
        let t = PrivateTemp::new().unwrap();
        let dir = t.path().to_path_buf();
        let f = t.file("install.sh").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&f), 0o600);
        assert!(t.file("install.sh").is_err(), "never reuses a file");
        drop(t);
        assert!(!dir.exists());
    }

    #[test]
    fn wget_reads_the_location_header() {
        let r = RecordingRunner::default();
        r.replies.borrow_mut().push_back(Ok(CommandOutput {
            status: 8,
            stdout: String::new(),
            stderr: "  HTTP/1.1 302 Found\n  Location: https://github.com/lesquel/falconer-hook/releases/tag/v0.2.1\r\n".into(),
        }));
        assert_eq!(resolve_latest(&r, Downloader::Wget).unwrap(), "v0.2.1");
        let call = &r.calls.borrow()[0];
        assert!(call.iter().any(|a| a == "--https-only"), "{call:?}");
    }

    #[test]
    fn download_commands_are_https_only() {
        let p = Path::new("/t/install.sh");
        assert_eq!(
            Downloader::Curl
                .download_argv("https://x/i.sh", p)
                .join(" "),
            "curl -fsSL --proto =https --tlsv1.2 -o /t/install.sh https://x/i.sh"
        );
        assert_eq!(
            Downloader::Wget
                .download_argv("https://x/i.sh", p)
                .join(" "),
            "wget -q --https-only -O /t/install.sh https://x/i.sh"
        );
    }
}
