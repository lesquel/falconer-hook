//! `install.sh --dry-run` changes nothing and prints every step. Run with a
//! temporary HOME and fake `tailscale`/`herdr`/`cargo` so no real tool runs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn dry_run(args: &[&str]) -> (tempfile::TempDir, String) {
    dry_run_with(args, &[("OPENMOSH_NO_GH", "1")], &[])
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

/// Runs `install.sh --dry-run` and returns the raw output; the HOME must stay empty.
fn dry_run_output(
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
    assert!(out.contains("releases/latest/download/openmosh-hook-"));
    assert!(out.contains("releases/latest/download/SHA256SUMS"));
    assert!(out.contains("would verify openmosh-hook-"));
    assert!(out.contains("openmosh-hook service install"));
    assert!(out.contains("openmosh-hook doctor"));
    assert!(out.contains("config.toml (mode 0600)"));
    assert!(out.contains("Next: open OpenMosh on your phone"));
}

#[test]
fn from_source_and_binary_modes() {
    let (_t, out) = dry_run(&["--from-source", "--no-service"]);
    assert!(out.contains(
        "+ cargo install --git https://github.com/lesquel/openmosh-hook --locked --root"
    ));
    assert!(!out.contains("service install"));
    let (_t, out) = dry_run(&["--binary", env!("CARGO_BIN_EXE_openmosh-hook")]);
    assert!(out.contains(&format!(
        "+ install -m 0755 {}",
        env!("CARGO_BIN_EXE_openmosh-hook")
    )));
}

#[test]
fn yes_never_runs_installers_in_dry_run() {
    let (_t, out) = dry_run(&["--yes", "--no-service"]);
    assert!(!out.contains("+ curl -fsSL https://herdr.dev/install.sh | sh"));
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
            &[("OPENMOSH_NO_GH", "1")],
            &[uname(os, arch)],
        );
        assert!(
            out.contains(&format!(
                "releases/latest/download/openmosh-hook-{target}.tar.gz"
            )),
            "{os}/{arch}: {out}"
        );
    }
}

#[test]
fn version_tag_selects_the_release() {
    let (_t, out) = dry_run_with(
        &["--version", "v0.1.0", "--no-service"],
        &[("OPENMOSH_NO_GH", "1")],
        &[uname("Linux", "x86_64")],
    );
    assert!(out.contains("releases/download/v0.1.0/openmosh-hook-x86_64-unknown-linux-musl.tar.gz"));
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
        "+ gh release download --repo lesquel/openmosh-hook --pattern openmosh-hook-aarch64-apple-darwin.tar.gz --pattern SHA256SUMS --pattern SHA256SUMS.minisig --dir <tmpdir>"
    ), "{out}");
    assert!(!out.contains("curl"));
    let (_t, out) = dry_run_with(
        &["--version", "v0.2.0", "--no-service"],
        &[],
        &[uname("Linux", "x86_64"), gh_logged_in()],
    );
    assert!(out.contains("+ gh release download v0.2.0 --repo lesquel/openmosh-hook"));
    let (_t, out) = dry_run_with(
        &["--no-service"],
        &[("OPENMOSH_NO_GH", "1")],
        &[uname("Linux", "x86_64"), gh_logged_in()],
    );
    assert!(
        !out.contains("gh release download"),
        "OPENMOSH_NO_GH opts out"
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
fn without_a_verifier_it_warns_checksum_only() {
    if system_verifier() {
        return;
    }
    let (_t, out) = dry_run_output(
        &["--no-service"],
        &[("OPENMOSH_NO_GH", "1")],
        &[uname("Linux", "x86_64")],
    );
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains(
            "warning: minisign/rsign not found: checking SHA256SUMS only, without its signature."
        ),
        "{stderr}"
    );
    assert!(stderr.contains("--require-signature"), "{stderr}");
    assert!(
        !stdout.contains("SHA256SUMS.minisig"),
        "no signature to fetch: {stdout}"
    );
    assert!(stdout.contains(
        "would verify openmosh-hook-x86_64-unknown-linux-musl.tar.gz against SHA256SUMS"
    ));
}

#[test]
fn require_signature_aborts_without_a_verifier() {
    if system_verifier() {
        return;
    }
    let (_t, out) = dry_run_output(
        &["--require-signature", "--no-service"],
        &[("OPENMOSH_NO_GH", "1")],
        &[uname("Linux", "x86_64")],
    );
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("error: --require-signature: install minisign (https://jedisct1.github.io/minisign/) or rsign2 first"),
        "{stderr}"
    );
    assert!(
        !stdout.contains("download"),
        "aborts before downloading: {stdout}"
    );
}

#[test]
fn a_verifier_checks_the_signature_with_the_pinned_key() {
    for tool in ["minisign", "rsign"] {
        for extra in [&[][..], &["--require-signature"][..]] {
            let mut args = vec!["--no-service"];
            args.extend_from_slice(extra);
            let (_t, out) = dry_run_output(
                &args,
                &[("OPENMOSH_NO_GH", "1")],
                &[uname("Linux", "x86_64"), verifier(tool)],
            );
            let stdout = String::from_utf8(out.stdout).unwrap();
            let stderr = String::from_utf8(out.stderr).unwrap();
            assert!(out.status.success(), "{tool}: {stdout}\n{stderr}");
            if !system_verifier() || tool == "minisign" {
                assert!(
                    stdout.contains(&format!(
                        "(dry run: would verify SHA256SUMS.minisig with {tool} and key {PUBKEY})"
                    )),
                    "{tool}: {stdout}"
                );
            }
            assert!(
                stdout.contains("releases/latest/download/SHA256SUMS.minisig"),
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
}

#[test]
fn version_must_be_a_plain_release_tag() {
    for bad in [
        "v1 --repo evil/x",
        "../../../../evil/repo/releases/download/v1",
        "latest",
        "v1/../x",
    ] {
        let (_t, out) = dry_run_output(&["--version", bad], &[], &[]);
        assert!(!out.status.success(), "{bad} was accepted");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("release tag"),
            "{bad}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let (_t, out) = dry_run_output(&["--version", "v0.1.0"], &[], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
