//! `install.sh --dry-run` changes nothing and prints every step. Run with a
//! temporary HOME and fake `tailscale`/`herdr`/`cargo` so no real tool runs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn dry_run(args: &[&str]) -> (tempfile::TempDir, String) {
    dry_run_with(args, &[("FALCONER_NO_GH", "1")], &[])
}

/// Fake `uname` reporting `os`/`arch`.
fn uname(os: &str, arch: &str) -> (&'static str, String) {
    (
        "uname",
        format!("#!/bin/sh\ncase \"$1\" in -s) echo {os} ;; -m) echo {arch} ;; esac\n"),
    )
}

/// A logged-in GitHub CLI: only `gh auth status` may run in a dry run.
fn gh_logged_in() -> (&'static str, String) {
    (
        "gh",
        "#!/bin/sh\n[ \"$1 $2\" = \"auth status\" ] && exit 0\necho 'gh must not download in dry-run' >&2\nexit 99\n".into(),
    )
}

/// Like `dry_run_output_bare`, with a signature verifier on PATH (it must not
/// run in a dry run): the signature is required by default.
fn dry_run_output(
    args: &[&str],
    env: &[(&str, &str)],
    fakes: &[(&str, String)],
) -> (tempfile::TempDir, Output) {
    let mut all = vec![verifier("minisign")];
    all.extend(fakes.iter().cloned());
    dry_run_output_bare(args, env, &all)
}

/// Runs `install.sh --dry-run` and returns the raw output; the HOME must stay empty.
fn dry_run_output_bare(
    args: &[&str],
    env: &[(&str, &str)],
    fakes: &[(&str, String)],
) -> (tempfile::TempDir, Output) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let bin = t.path().join("bin");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&bin).unwrap();
    for name in ["tailscale", "herdr", "cargo"] {
        let p = bin.join(name);
        fs::write(
            &p,
            "#!/bin/sh\necho 'fake must not run in dry-run' >&2\nexit 99\n",
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (name, script) in fakes {
        let p = bin.join(name);
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
        .arg("--dry-run")
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert_eq!(
        fs::read_dir(&home).unwrap().count(),
        0,
        "dry run must not create files"
    );
    (t, out)
}

fn dry_run_with(
    args: &[&str],
    env: &[(&str, &str)],
    fakes: &[(&str, String)],
) -> (tempfile::TempDir, String) {
    let (t, out) = dry_run_output(args, env, fakes);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (t, stdout)
}

#[test]
fn release_dry_run_prints_every_step() {
    let (_t, out) = dry_run(&[]);
    assert!(out.contains("(dry run: nothing will be changed)"));
    assert!(out.contains("(dry run: would resolve the latest release to its tag)"));
    assert!(out.contains("releases/download/<latest>/falconer-hook-"));
    assert!(out.contains("releases/download/<latest>/SHA256SUMS"));
    assert!(out.contains("would verify falconer-hook-"));
    assert!(out.contains("falconer-hook service install"));
    assert!(out.contains("falconer-hook doctor"));
    assert!(out.contains("config.toml (mode 0600)"));
    assert!(out.contains("Next: open Falconer on your phone"));
}

#[test]
fn from_source_and_binary_modes() {
    let (_t, out) = dry_run(&["--from-source", "--no-service"]);
    assert!(out.contains(
        "+ cargo install --git https://github.com/lesquel/falconer-hook --locked --root"
    ));
    assert!(!out.contains("service install"));
    let (_t, out) = dry_run(&["--from-source", "--version", "v0.2.0", "--no-service"]);
    assert!(
        out.contains(
            "+ cargo install --git https://github.com/lesquel/falconer-hook --tag v0.2.0 --locked --root"
        ),
        "{out}"
    );
    let (_t, out) = dry_run(&["--binary", env!("CARGO_BIN_EXE_falconer-hook")]);
    assert!(out.contains(&format!(
        "+ install -m 0755 {}",
        env!("CARGO_BIN_EXE_falconer-hook")
    )));
}

#[test]
fn yes_never_runs_installers_in_dry_run() {
    let (_t, out) = dry_run(&["--yes", "--no-service"]);
    assert!(!out.contains("+ curl -fsSL https://herdr.dev/install.sh | sh"));
    assert!(!out.contains("+ sh "), "{out}");
    assert!(!out.contains("| sh"), "an installer is never piped: {out}");
}

#[test]
fn release_asset_matches_the_platform() {
    for (os, arch, target) in [
        ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
        ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
        ("Darwin", "x86_64", "x86_64-apple-darwin"),
        ("Darwin", "arm64", "aarch64-apple-darwin"),
    ] {
        let (_t, out) = dry_run_with(
            &["--no-service"],
            &[("FALCONER_NO_GH", "1")],
            &[uname(os, arch)],
        );
        assert!(
            out.contains(&format!(
                "releases/download/<latest>/falconer-hook-{target}.tar.gz"
            )),
            "{os}/{arch}: {out}"
        );
    }
}

#[test]
fn version_tag_selects_the_release() {
    let (_t, out) = dry_run_with(
        &["--version", "v0.1.0", "--no-service"],
        &[("FALCONER_NO_GH", "1")],
        &[uname("Linux", "x86_64")],
    );
    assert!(out.contains("releases/download/v0.1.0/falconer-hook-x86_64-unknown-linux-musl.tar.gz"));
    assert!(out.contains("releases/download/v0.1.0/SHA256SUMS"));
}

#[test]
fn logged_in_gh_downloads_the_private_release() {
    let (_t, out) = dry_run_with(
        &["--no-service"],
        &[],
        &[uname("Darwin", "arm64"), gh_logged_in()],
    );
    assert!(out.contains("Using the GitHub CLI"), "{out}");
    assert!(out.contains(
        "+ gh release download <latest> --repo lesquel/falconer-hook --pattern falconer-hook-aarch64-apple-darwin.tar.gz --pattern SHA256SUMS --pattern SHA256SUMS.minisig --dir <tmpdir>"
    ), "{out}");
    assert!(!out.contains("curl"));
    let (_t, out) = dry_run_with(
        &["--version", "v0.2.0", "--no-service"],
        &[],
        &[uname("Linux", "x86_64"), gh_logged_in()],
    );
    assert!(out.contains("+ gh release download v0.2.0 --repo lesquel/falconer-hook"));
    let (_t, out) = dry_run_with(
        &["--no-service"],
        &[("FALCONER_NO_GH", "1")],
        &[uname("Linux", "x86_64"), gh_logged_in()],
    );
    assert!(
        !out.contains("gh release download"),
        "FALCONER_NO_GH opts out"
    );
}

const PUBKEY: &str = "RWTiZ1vxHn/jWfqAEjobeqloI9cL9pOA0/in+ayyCgPqT0gYDRKR1Ox5";

/// A signature verifier that must not run in a dry run.
fn verifier(name: &'static str) -> (&'static str, String) {
    (
        name,
        "#!/bin/sh\necho 'verifier must not run in dry-run' >&2\nexit 99\n".into(),
    )
}

/// Whether a real verifier outside the fake bin dir would be found.
fn system_verifier() -> bool {
    ["/usr/bin", "/bin"]
        .iter()
        .any(|d| Path::new(d).join("minisign").exists() || Path::new(d).join("rsign").exists())
}

#[test]
fn the_pinned_key_matches_minisign_pub() {
    let pub_file =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/minisign.pub")).unwrap();
    let lines: Vec<&str> = pub_file.lines().collect();
    assert_eq!(
        lines,
        [
            "untrusted comment: minisign public key 59E37F1EF15B67E2",
            PUBKEY
        ]
    );
    let script = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh")).unwrap();
    assert!(script.contains(&format!("PUBKEY=\"{PUBKEY}\"")));
}

#[test]
fn without_a_verifier_it_refuses_with_install_instructions() {
    if system_verifier() {
        return;
    }
    for extra in [&[][..], &["--require-signature"][..]] {
        let mut args = vec!["--no-service"];
        args.extend_from_slice(extra);
        let (_t, out) = dry_run_output_bare(
            &args,
            &[("FALCONER_NO_GH", "1")],
            &[uname("Linux", "x86_64")],
        );
        assert_eq!(out.status.code(), Some(1));
        let stdout = String::from_utf8(out.stdout).unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr
                .contains("error: minisign or rsign is required to verify the release signature."),
            "{stderr}"
        );
        assert!(stderr.contains("sudo apt install minisign"), "{stderr}");
        assert!(stderr.contains("cargo install rsign2 --locked"), "{stderr}");
        assert!(stderr.contains("--insecure-no-signature"), "{stderr}");
        assert!(
            !stdout.contains("download"),
            "aborts before downloading: {stdout}"
        );
    }
}

#[test]
fn insecure_no_signature_checks_checksums_only_and_says_so() {
    if system_verifier() {
        return;
    }
    let (_t, out) = dry_run_output_bare(
        &["--insecure-no-signature", "--no-service"],
        &[("FALCONER_NO_GH", "1")],
        &[uname("Linux", "x86_64")],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains(
            "warning: --insecure-no-signature: checking SHA256SUMS only, without its signature."
        ),
        "{stderr}"
    );
    assert!(!stdout.contains("SHA256SUMS.minisig"), "{stdout}");
    assert!(stdout.contains(
        "would verify falconer-hook-x86_64-unknown-linux-musl.tar.gz against SHA256SUMS"
    ));
}

#[test]
fn a_verifier_checks_the_signature_with_the_pinned_key() {
    for tool in ["minisign", "rsign"] {
        for extra in [&[][..], &["--require-signature"][..]] {
            let mut args = vec!["--no-service"];
            args.extend_from_slice(extra);
            let (_t, out) = dry_run_output_bare(
                &args,
                &[("FALCONER_NO_GH", "1")],
                &[uname("Linux", "x86_64"), verifier(tool)],
            );
            let stdout = String::from_utf8(out.stdout).unwrap();
            let stderr = String::from_utf8(out.stderr).unwrap();
            assert!(out.status.success(), "{tool}: {stdout}\n{stderr}");
            if !system_verifier() || tool == "minisign" {
                assert!(
                    stdout.contains(&format!(
                        "(dry run: would verify SHA256SUMS.minisig with {tool} and key {PUBKEY}, trusted comment \"falconer-hook <latest>\")"
                    )),
                    "{tool}: {stdout}"
                );
            }
            assert!(
                stdout.contains("releases/download/<latest>/SHA256SUMS.minisig"),
                "{stdout}"
            );
            assert!(!stderr.contains("checking SHA256SUMS only"), "{stderr}");
        }
    }
}

#[test]
fn require_signature_is_listed_in_help() {
    let out = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
        .arg("--help")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("--require-signature"), "{stdout}");
    assert!(stdout.contains("--insecure-no-signature"), "{stdout}");
}

#[test]
fn version_must_be_a_plain_release_tag() {
    for bad in [
        "v1 --repo evil/x",
        "../../../../evil/repo/releases/download/v1",
        "latest",
        "v1/../x",
    ] {
        let (_t, out) = dry_run_output(&["--version", bad], &[("FALCONER_NO_GH", "1")], &[]);
        assert!(!out.status.success(), "{bad} was accepted");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("release tag"),
            "{bad}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // gh is turned off: on CI runners its auth check writes a config file into HOME.
    let (_t, out) = dry_run_output(&["--version", "v0.1.0"], &[("FALCONER_NO_GH", "1")], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A real (not dry) install against fake downloads: `curl` serves files from
/// a fixture directory, `minisign` accepts every signature, so what is
/// tested is the installer's own logic (checksums, trusted comment, tag).
struct Release {
    t: tempfile::TempDir,
}

impl Release {
    /// A release `served_tag` whose signature's trusted comment is `comment`.
    fn new(served_tag: &str, comment: &str) -> Self {
        let t = tempfile::tempdir().unwrap();
        let fix = t.path().join("fix");
        let pkg = t.path().join("pkg");
        let bin = t.path().join("bin");
        for d in [&fix, &pkg, &bin, &t.path().join("home")] {
            fs::create_dir_all(d).unwrap();
        }
        let exe = pkg.join("falconer-hook");
        fs::write(&exe, "#!/bin/sh\necho fake-hook \"$@\"\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let asset = "falconer-hook-x86_64-unknown-linux-musl.tar.gz";
        assert!(Command::new("tar")
            .args(["-czf"])
            .arg(fix.join(asset))
            .arg("-C")
            .arg(&pkg)
            .arg("falconer-hook")
            .status()
            .unwrap()
            .success());
        let sums = Command::new("sha256sum")
            .arg(asset)
            .current_dir(&fix)
            .output()
            .unwrap();
        fs::write(fix.join("SHA256SUMS"), sums.stdout).unwrap();
        fs::write(
            fix.join("SHA256SUMS.minisig"),
            format!("untrusted comment: x\nRUQ=\ntrusted comment: {comment}\nAAA=\n"),
        )
        .unwrap();
        let fakes = [
            uname("Linux", "x86_64"),
            (
                "curl",
                format!(
                    "#!/bin/sh\nout=''; w=''; for a in \"$@\"; do case \"$prev\" in -o) out=$a ;; -w) w=$a ;; esac; prev=$a; url=$a; done\n\
                     echo \"$url\" >> \"{log}\"\n\
                     if [ -n \"$w\" ]; then printf 'https://github.com/lesquel/falconer-hook/releases/tag/{served_tag}'; exit 0; fi\n\
                     case \"$url\" in */download/{served_tag}/*) cp \"{fix}/${{url##*/}}\" \"$out\" ;; https://herdr.dev/install.sh) printf 'echo herdr-installer-ran\\n' > \"$out\" ;; *) exit 22 ;; esac\n",
                    log = t.path().join("curl.log").display(),
                    fix = fix.display(),
                ),
            ),
            ("minisign", "#!/bin/sh\nexit 0\n".to_string()),
            ("tailscale", "#!/bin/sh\nexit 1\n".to_string()),
            // A runtime is "installed", so no installer is ever offered.
            ("tmux", "#!/bin/sh\nexit 0\n".to_string()),
        ];
        for (name, script) in fakes {
            let p = bin.join(name);
            fs::write(&p, script).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { t }
    }

    fn install(&self, args: &[&str]) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.t.path().join("bin").display());
        self.install_with_path(args, &path)
    }

    fn install_with_path(&self, args: &[&str], path: &str) -> Output {
        self.install_with_env(args, path, &[])
    }

    fn install_with_env(&self, args: &[&str], path: &str, env: &[(&str, &str)]) -> Output {
        Command::new("sh")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
            .args(args)
            .arg("--no-service")
            .env_clear()
            .env("HOME", self.t.path().join("home"))
            .env("FALCONER_NO_GH", "1")
            .env("PATH", path)
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    fn remove_fake(&self, name: &str) {
        fs::remove_file(self.t.path().join("bin").join(name)).unwrap();
    }

    fn installed(&self) -> bool {
        self.t.path().join("home/.local/bin/falconer-hook").exists()
    }

    fn fetched(&self) -> String {
        fs::read_to_string(self.t.path().join("curl.log")).unwrap_or_default()
    }
}

#[test]
fn a_signed_release_installs_and_latest_is_resolved_to_its_tag() {
    let r = Release::new("v9.9.9", "falconer-hook v9.9.9");
    let out = r.install(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("ok: SHA256SUMS signature verified (key 59E37F1EF15B67E2, release v9.9.9)"),
        "{stdout}"
    );
    assert!(r.installed());
    let fetched = r.fetched();
    assert!(fetched.contains("https://github.com/lesquel/falconer-hook/releases/latest\n"));
    assert!(
        fetched.contains("/releases/download/v9.9.9/SHA256SUMS.minisig"),
        "downloads use the resolved tag: {fetched}"
    );
    assert!(!fetched.contains("/latest/download/"), "{fetched}");
}

#[test]
fn a_signature_for_another_release_is_refused() {
    // The server answers for v9.9.9 with files (validly) signed for v0.1.0.
    let r = Release::new("v9.9.9", "falconer-hook v0.1.0");
    let out = r.install(&["--version", "v9.9.9"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "the signature is for \"falconer-hook v0.1.0\", not \"falconer-hook v9.9.9\""
        ),
        "{stderr}"
    );
    assert!(!r.installed(), "nothing is installed");
    let r = Release::new("v9.9.9", "something else");
    assert_eq!(r.install(&[]).status.code(), Some(1));
    assert!(!r.installed());
}

#[test]
fn a_tampered_download_is_refused_even_with_a_valid_signature() {
    let r = Release::new("v9.9.9", "falconer-hook v9.9.9");
    fs::write(
        r.t.path()
            .join("fix/falconer-hook-x86_64-unknown-linux-musl.tar.gz"),
        b"evil",
    )
    .unwrap();
    let out = r.install(&["--version", "v9.9.9"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"));
    assert!(!r.installed());
}

#[test]
fn an_old_openmosh_binary_becomes_a_compatibility_symlink() {
    let r = Release::new("v9.9.9", "falconer-hook v9.9.9");
    let bin = r.t.path().join("home/.local/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("openmosh-hook"), "#!/bin/sh\necho old\n").unwrap();
    fs::set_permissions(bin.join("openmosh-hook"), fs::Permissions::from_mode(0o755)).unwrap();
    let old_cfg = r.t.path().join("home/.config/openmosh/config.toml");
    fs::create_dir_all(old_cfg.parent().unwrap()).unwrap();
    fs::write(&old_cfg, "# old\n").unwrap();

    let out = r.install(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(r.installed());
    let link = bin.join("openmosh-hook");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the old binary is replaced by a symlink"
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("falconer-hook"),
        "relative link to the new binary in the same directory"
    );
    assert!(fs::read_to_string(&link).unwrap().contains("fake-hook"));
    assert_eq!(fs::read_to_string(&old_cfg).unwrap(), "# old\n");
    assert!(
        !r.t.path().join("home/.config/falconer").exists(),
        "no fresh config next to the OpenMosh one: falconer-hook moves it"
    );

    // A second run keeps the link and does not loop.
    let out = r.install(&[]);
    assert!(out.status.success());
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("falconer-hook"));
}

#[test]
fn no_compatibility_symlink_without_an_old_binary() {
    let r = Release::new("v9.9.9", "falconer-hook v9.9.9");
    let out = r.install(&[]);
    assert!(out.status.success());
    assert!(r.installed());
    assert!(fs::symlink_metadata(r.t.path().join("home/.local/bin/openmosh-hook")).is_err());
}

/// A release tree with no runtime on PATH, so install.sh would offer Herdr.
fn release_without_runtime() -> (Release, String) {
    let r = Release::new("v9.9.9", "falconer-hook v9.9.9");
    r.remove_fake("tmux");
    // The system tools, minus any real runtime, so the installer is offered.
    let sys = r.t.path().join("sys");
    fs::create_dir_all(&sys).unwrap();
    for dir in ["/usr/bin", "/bin"] {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name();
            if ["herdr", "tmux"].iter().any(|t| name == *t) {
                continue;
            }
            let _ = std::os::unix::fs::symlink(entry.path(), sys.join(&name));
        }
    }
    let path = format!("{}:{}", r.t.path().join("bin").display(), sys.display());
    (r, path)
}

#[test]
fn yes_downloads_the_herdr_installer_to_a_private_file_before_running_it() {
    let (r, path) = release_without_runtime();
    let out = r.install_with_path(&["--yes"], &path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let fetch = stdout
        .lines()
        .find(|l| l.starts_with("+ curl --proto =https --tlsv1.2 -fsSL -o "))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(fetch.ends_with(" https://herdr.dev/install.sh"), "{fetch}");
    let file = fetch.split(' ').nth(7).unwrap();
    assert!(stdout.contains(&format!("+ sh {file}")), "{stdout}");
    assert!(stdout.contains("herdr-installer-ran"), "{stdout}");
    assert!(!Path::new(file).exists(), "the installer is deleted");
    assert!(!stdout.contains("| sh"), "{stdout}");
}

#[test]
fn skip_prereqs_never_offers_or_runs_an_installer_even_with_yes() {
    let (r, path) = release_without_runtime();
    let out = r.install_with_env(&["--yes"], &path, &[("FALCONER_SKIP_PREREQS", "1")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(r.installed());
    assert!(
        stdout.contains("Skipping the prerequisite checks"),
        "{stdout}"
    );
    assert!(!stdout.contains("herdr.dev"), "{stdout}");
    assert!(!stdout.contains("tailscale.com"), "{stdout}");
    assert!(!stdout.contains("herdr-installer-ran"), "{stdout}");
}
