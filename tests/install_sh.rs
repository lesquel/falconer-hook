//! `install.sh --dry-run` changes nothing and prints every step. Run with a
//! temporary HOME and fake `tailscale`/`herdr`/`cargo` so no real tool runs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn dry_run(args: &[&str]) -> (tempfile::TempDir, String) {
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
    let out = Command::new("sh")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh"))
        .arg("--dry-run")
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_dir(&home).unwrap().count(),
        0,
        "dry run must not create files"
    );
    (t, stdout)
}

#[test]
fn release_dry_run_prints_every_step() {
    let (_t, out) = dry_run(&[]);
    assert!(out.contains("(dry run: nothing will be changed)"));
    assert!(out.contains("releases/latest/download/openmosh-hook-"));
    assert!(out.contains(".tar.gz.sha256"));
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
