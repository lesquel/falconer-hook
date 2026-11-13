//! Prerequisites `setup` can fix on its own: an agent runtime (tmux) and a
//! reachable SSH server. Pure decisions only: which package manager, which
//! argv, whether to ask. Nothing here runs a program.

use serde::Serialize;

pub const TMUX: &str = "tmux";
pub const HERDR: &str = "herdr";
pub const HERDR_URL: &str = "https://herdr.dev";
pub const HOMEBREW_URL: &str = "https://brew.sh";

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
    /// all), and its installer is a `curl | sh` we never run.
    pub fn has_herdr(self) -> bool {
        self == Self::Brew
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
    fn consent_rules() {
        let never = || -> bool { panic!("must not ask") };
        assert_eq!(consent(true, false, never), Consent::Run);
        assert_eq!(consent(false, false, never), Consent::PrintOnly);
        assert_eq!(consent(false, true, || true), Consent::Run);
        assert_eq!(consent(false, true, || false), Consent::Declined);
    }
}
