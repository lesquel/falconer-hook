//! Prerequisites `setup` can fix on its own: an agent runtime (Herdr or
//! tmux), a reachable SSH server and Tailscale. Pure decisions only: which
//! package manager, which installer, which argv, whether to ask. Nothing here
//! runs a program.

use serde::Serialize;

pub const TMUX: &str = "tmux";
pub const HERDR: &str = "herdr";
pub const HERDR_URL: &str = "https://herdr.dev";
pub const HOMEBREW_URL: &str = "https://brew.sh";
pub const TAILSCALE: &str = "tailscale";
pub const TAILSCALE_DOWNLOAD_URL: &str = "https://tailscale.com/download";
/// Official installers, which always install the latest release. Used only
/// when no package manager here ships the tool (Homebrew stays preferred).
pub const HERDR_INSTALLER_URL: &str = "https://herdr.dev/install.sh";
pub const TAILSCALE_INSTALLER_URL: &str = "https://tailscale.com/install.sh";
/// What the downloaded installer is called in the commands shown to the user.
pub const INSTALLER_FILE: &str = "install.sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Brew,
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Apk,
}

impl PackageManager {
    /// Probe order. Homebrew first: it is the only manager on macOS and runs
    /// without root on Linux too.
    pub const ALL: [PackageManager; 6] = [
        Self::Brew,
        Self::Apt,
        Self::Dnf,
        Self::Pacman,
        Self::Zypper,
        Self::Apk,
    ];

    pub fn binary(self) -> &'static str {
        match self {
            Self::Brew => "brew",
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
            Self::Apk => "apk",
        }
    }

    /// Whether this manager ships Herdr. Checked against Repology: only
    /// Homebrew has it among the supported managers (Arch has it only in the
    /// AUR, openSUSE only in Tumbleweed, and Debian, Fedora and Alpine not at
    /// all). Elsewhere `setup` offers Herdr's official installer, downloaded
    /// to a private file first and never piped from the network.
    pub fn has_herdr(self) -> bool {
        self == Self::Brew
    }

    /// Whether this manager ships Tailscale in its default repositories.
    /// Only Homebrew is relied on; elsewhere the official installer adds
    /// Tailscale's own repository and picks the right package.
    pub fn has_tailscale(self) -> bool {
        self == Self::Brew
    }

    /// The OpenSSH server package, `None` for Homebrew (macOS ships SSH and
    /// Homebrew does not run system services).
    pub fn ssh_server_package(self) -> Option<&'static str> {
        match self {
            Self::Brew => None,
            Self::Apt | Self::Dnf => Some("openssh-server"),
            Self::Pacman | Self::Zypper | Self::Apk => Some("openssh"),
        }
    }

    pub fn needs_root(self) -> bool {
        self != Self::Brew
    }

    /// The install command without privilege escalation.
    pub fn install_argv(self, package: &str) -> Vec<String> {
        let tail: &[&str] = match self {
            Self::Brew => &["install"],
            Self::Apt | Self::Dnf => &["install", "-y"],
            Self::Pacman => &["-S", "--noconfirm"],
            Self::Zypper => &["--non-interactive", "install"],
            Self::Apk => &["add"],
        };
        std::iter::once(self.binary())
            .chain(tail.iter().copied())
            .chain(std::iter::once(package))
            .map(String::from)
            .collect()
    }
}

/// The first supported package manager for which `has(binary)` is true.
pub fn detect_package_manager(has: impl Fn(&str) -> bool) -> Option<PackageManager> {
    PackageManager::ALL.into_iter().find(|pm| has(pm.binary()))
}

/// The first supported system package manager (Homebrew excluded), for
/// packages Homebrew does not ship, such as the OpenSSH server.
pub fn detect_system_package_manager(has: impl Fn(&str) -> bool) -> Option<PackageManager> {
    PackageManager::ALL
        .into_iter()
        .filter(|pm| *pm != PackageManager::Brew)
        .find(|pm| has(pm.binary()))
}

/// Downloads an official installer to a file, HTTPS and TLS 1.2+ only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Downloader {
    Curl,
    Wget,
}

impl Downloader {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Curl => "curl",
            Self::Wget => "wget",
        }
    }

    /// Fetches `url` into `dest`, refusing anything but HTTPS (redirects
    /// included) and TLS older than 1.2.
    pub fn argv(self, url: &str, dest: &str) -> Vec<String> {
        let parts: Vec<&str> = match self {
            Self::Curl => vec![
                "curl",
                "--proto",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                "-o",
                dest,
                url,
            ],
            Self::Wget => vec![
                "wget",
                "--https-only",
                "--secure-protocol=TLSv1_2",
                "-q",
                "-O",
                dest,
                url,
            ],
        };
        parts.into_iter().map(String::from).collect()
    }
}

/// curl first, then wget.
pub fn detect_downloader(has: impl Fn(&str) -> bool) -> Option<Downloader> {
    [Downloader::Curl, Downloader::Wget]
        .into_iter()
        .find(|d| has(d.binary()))
}

/// Runs a downloaded installer. The installers ask for `sudo` themselves
/// when they need it.
pub fn run_installer_argv(file: &str) -> Vec<String> {
    vec!["sh".to_string(), file.to_string()]
}

/// The whole installer step as the user would type it, with `install.sh`
/// standing for the private temp file.
pub fn installer_display(d: Downloader, url: &str) -> String {
    format!(
        "{} && {}",
        display(&d.argv(url, INSTALLER_FILE)),
        display(&run_installer_argv(INSTALLER_FILE))
    )
}

/// Brings Tailscale up (it prints a login URL when the node is logged out).
/// Linux needs root; on macOS the app's CLI talks to the user's daemon.
pub fn tailscale_up_argv(program: &str, os: Os, is_root: bool) -> Vec<String> {
    privileged(
        vec![program.to_string(), "up".to_string()],
        os == Os::Linux,
        is_root,
    )
}

/// Where Herdr or Tailscale comes from on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Package(PackageManager),
    Installer(Downloader),
}

/// Homebrew when it ships the tool, else the official installer when a
/// downloader exists, else nothing.
pub fn install_source(
    pm: Option<PackageManager>,
    ships: impl Fn(PackageManager) -> bool,
    downloader: Option<Downloader>,
) -> Option<Source> {
    pm.filter(|pm| ships(*pm))
        .map(Source::Package)
        .or(downloader.map(Source::Installer))
}

/// Prefixes `sudo` when the command needs root and we are not root.
pub fn privileged(argv: Vec<String>, needs_root: bool, is_root: bool) -> Vec<String> {
    if needs_root && !is_root {
        std::iter::once("sudo".to_string()).chain(argv).collect()
    } else {
        argv
    }
}

/// The command as the user would type it.
pub fn display(argv: &[String]) -> String {
    argv.join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// The command that turns the SSH server on. On Debian-family systems the
/// unit is `ssh`, elsewhere `sshd`.
pub fn enable_ssh_argv(os: Os, pm: Option<PackageManager>, is_root: bool) -> Vec<String> {
    let argv: Vec<&str> = match os {
        Os::MacOs => vec!["systemsetup", "-setremotelogin", "on"],
        Os::Linux => {
            let unit = if pm == Some(PackageManager::Apt) {
                "ssh"
            } else {
                "sshd"
            };
            vec!["systemctl", "enable", "--now", unit]
        }
    };
    privileged(argv.into_iter().map(String::from).collect(), true, is_root)
}

/// What to tell the user when nothing listens on the SSH port.
pub fn ssh_fix_hint(os: Os, pm: Option<PackageManager>, is_root: bool) -> String {
    let cmd = display(&enable_ssh_argv(os, pm, is_root));
    match os {
        Os::MacOs => {
            format!("turn on System Settings > General > Sharing > Remote Login, or run `{cmd}`")
        }
        Os::Linux => format!("run `{cmd}` (install openssh-server first if it is missing)"),
    }
}

/// What the user decided, or what the environment allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    Run,
    Declined,
    /// No terminal and no `--yes`: only print the command.
    PrintOnly,
}

/// `--yes` runs without asking, no terminal only prints, otherwise ask.
pub fn consent(yes: bool, interactive: bool, ask: impl FnOnce() -> bool) -> Consent {
    if yes {
        Consent::Run
    } else if !interactive {
        Consent::PrintOnly
    } else if ask() {
        Consent::Run
    } else {
        Consent::Declined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_the_first_manager_present() {
        assert_eq!(detect_package_manager(|_| false), None);
        assert_eq!(
            detect_package_manager(|b| b == "dnf" || b == "apk"),
            Some(PackageManager::Dnf)
        );
        assert_eq!(
            detect_package_manager(|b| b == "brew" || b == "apt-get"),
            Some(PackageManager::Brew)
        );
        assert_eq!(
            detect_package_manager(|b| b == "apk"),
            Some(PackageManager::Apk)
        );
    }

    #[test]
    fn install_commands_per_manager() {
        let cases = [
            (PackageManager::Brew, "brew install tmux"),
            (PackageManager::Apt, "apt-get install -y tmux"),
            (PackageManager::Dnf, "dnf install -y tmux"),
            (PackageManager::Pacman, "pacman -S --noconfirm tmux"),
            (
                PackageManager::Zypper,
                "zypper --non-interactive install tmux",
            ),
            (PackageManager::Apk, "apk add tmux"),
        ];
        for (pm, want) in cases {
            assert_eq!(display(&pm.install_argv(TMUX)), want);
            assert_eq!(pm.needs_root(), pm != PackageManager::Brew);
        }
    }

    #[test]
    fn only_homebrew_ships_herdr() {
        for pm in PackageManager::ALL {
            assert_eq!(pm.has_herdr(), pm == PackageManager::Brew);
        }
        assert_eq!(
            display(&PackageManager::Brew.install_argv(HERDR)),
            "brew install herdr"
        );
    }

    #[test]
    fn sudo_only_when_needed_and_not_root() {
        let apt = PackageManager::Apt.install_argv(TMUX);
        assert_eq!(
            display(&privileged(apt.clone(), true, false)),
            "sudo apt-get install -y tmux"
        );
        assert_eq!(privileged(apt.clone(), true, true), apt);
        let brew = PackageManager::Brew.install_argv(TMUX);
        assert_eq!(privileged(brew.clone(), false, false), brew);
    }

    #[test]
    fn enabling_ssh_depends_on_the_platform() {
        assert_eq!(
            enable_ssh_argv(Os::MacOs, None, false),
            argv(&["sudo", "systemsetup", "-setremotelogin", "on"])
        );
        assert_eq!(
            enable_ssh_argv(Os::Linux, Some(PackageManager::Apt), false),
            argv(&["sudo", "systemctl", "enable", "--now", "ssh"])
        );
        assert_eq!(
            enable_ssh_argv(Os::Linux, Some(PackageManager::Dnf), true),
            argv(&["systemctl", "enable", "--now", "sshd"])
        );
        assert!(ssh_fix_hint(Os::MacOs, None, false).contains("Remote Login"));
        assert!(ssh_fix_hint(Os::Linux, None, false).contains("sudo systemctl enable --now sshd"));
    }

    #[test]
    fn ssh_server_packages_per_manager() {
        let cases = [
            (PackageManager::Brew, None),
            (PackageManager::Apt, Some("openssh-server")),
            (PackageManager::Dnf, Some("openssh-server")),
            (PackageManager::Pacman, Some("openssh")),
            (PackageManager::Zypper, Some("openssh")),
            (PackageManager::Apk, Some("openssh")),
        ];
        for (pm, want) in cases {
            assert_eq!(pm.ssh_server_package(), want, "{pm:?}");
        }
        assert_eq!(
            detect_system_package_manager(|b| b == "brew" || b == "pacman"),
            Some(PackageManager::Pacman)
        );
        assert_eq!(detect_system_package_manager(|b| b == "brew"), None);
    }

    #[test]
    fn downloads_are_https_only_into_a_file() {
        assert_eq!(
            Downloader::Curl.argv(HERDR_INSTALLER_URL, "/t/i.sh"),
            argv(&[
                "curl",
                "--proto",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                "-o",
                "/t/i.sh",
                HERDR_INSTALLER_URL
            ])
        );
        assert_eq!(
            Downloader::Wget.argv(TAILSCALE_INSTALLER_URL, "/t/i.sh"),
            argv(&[
                "wget",
                "--https-only",
                "--secure-protocol=TLSv1_2",
                "-q",
                "-O",
                "/t/i.sh",
                TAILSCALE_INSTALLER_URL
            ])
        );
        assert_eq!(detect_downloader(|_| false), None);
        assert_eq!(detect_downloader(|_| true), Some(Downloader::Curl));
        assert_eq!(detect_downloader(|b| b == "wget"), Some(Downloader::Wget));
        assert_eq!(
            installer_display(Downloader::Curl, HERDR_INSTALLER_URL),
            "curl --proto =https --tlsv1.2 -fsSL -o install.sh https://herdr.dev/install.sh && sh install.sh"
        );
        assert!(!installer_display(Downloader::Curl, HERDR_INSTALLER_URL).contains('|'));
    }

    #[test]
    fn homebrew_is_preferred_then_the_installer() {
        let brew = Some(PackageManager::Brew);
        let apt = Some(PackageManager::Apt);
        let curl = Some(Downloader::Curl);
        let herdr = |pm: PackageManager| pm.has_herdr();
        assert_eq!(
            install_source(brew, herdr, curl),
            Some(Source::Package(PackageManager::Brew))
        );
        assert_eq!(
            install_source(apt, herdr, curl),
            Some(Source::Installer(Downloader::Curl))
        );
        assert_eq!(install_source(apt, herdr, None), None);
        assert_eq!(install_source(None, herdr, None), None);
        for pm in PackageManager::ALL {
            assert_eq!(pm.has_tailscale(), pm == PackageManager::Brew);
        }
    }

    #[test]
    fn tailscale_up_needs_root_on_linux_only() {
        assert_eq!(
            tailscale_up_argv("tailscale", Os::Linux, false),
            argv(&["sudo", "tailscale", "up"])
        );
        assert_eq!(
            tailscale_up_argv("tailscale", Os::Linux, true),
            argv(&["tailscale", "up"])
        );
        assert_eq!(
            tailscale_up_argv("tailscale", Os::MacOs, false),
            argv(&["tailscale", "up"])
        );
    }

    #[test]
    fn consent_rules() {
        let never = || -> bool { panic!("must not ask") };
        assert_eq!(consent(true, false, never), Consent::Run);
        assert_eq!(consent(false, false, never), Consent::PrintOnly);
        assert_eq!(consent(false, true, || true), Consent::Run);
        assert_eq!(consent(false, true, || false), Consent::Declined);
    }
}
