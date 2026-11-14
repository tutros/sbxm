//! Issue 427 (G73): text after the JSON of an `sbx ... --json` output is ignored with one warning,
//! also on the paths that never return to `main` (a failing `doctor`, `status --watch`).
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// A temporary `sbx` that prints valid JSON and then `trailer`, on every call, and a config dir
/// whose `min_sbx_version` makes `doctor` fail on its own.
fn setup(dir: &Path, trailer: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = dir.join("bin");
    let config = dir.join("config");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n*version*) echo '{{\"client\":{{\"version\":\"v0.46.0\"}}}}';;\n*) echo '{{\"sandboxes\":[]}}';;\nesac\nprintf '%s\\n' '{trailer}'\n"
    );
    let sbx = bin.join("sbx");
    std::fs::write(&sbx, script).unwrap();
    std::fs::set_permissions(&sbx, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "base_dir = \"{}\"\nprofiles_dir = \"{}\"\ndefault_profile = \"default\"\nmin_sbx_version = \"99.0.0\"\n\n[resources]\ncpus = 4\nmemory = \"8g\"\n",
            dir.join("base").display(),
            dir.join("profiles").display()
        ),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("base")).unwrap();
    std::fs::create_dir_all(dir.join("profiles/default")).unwrap();
    std::fs::write(
        dir.join("profiles/default/profile.toml"),
        "description = \"d\"\n",
    )
    .unwrap();
    (bin, config)
}

fn sbxm(dir: &Path, trailer: &str) -> Command {
    let (bin, config) = setup(dir, trailer);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sbxm"));
    cmd.env("SBXM_CONFIG_DIR", config).env(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    cmd
}

#[test]
fn a_failing_doctor_warns_about_the_trailer_before_exiting() {
    let dir = tempfile::tempdir().unwrap();
    let out = sbxm(dir.path(), "sbx: review427 trailer")
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("sbx: review427 trailer"), "{stderr}");
    assert!(stderr.contains("warning:"), "{stderr}");
}

#[test]
fn a_secret_shaped_or_blank_trailer_shows_no_text_in_doctor() {
    let dir = tempfile::tempdir().unwrap();
    let out = sbxm(dir.path(), "sk-abcdefghijklmnopqrstuvwx")
        .arg("doctor")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("sk-abc"), "{stderr}");
    assert!(stderr.contains("warning:"), "{stderr}");
    let dir = tempfile::tempdir().unwrap();
    let out = sbxm(dir.path(), "  ").arg("doctor").output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stderr).contains("warning:"));
}

#[test]
fn status_watch_warns_while_it_is_still_running() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = sbxm(dir.path(), "sbx: review427 watch");
    let (bin, config) = (
        cmd.get_envs()
            .find(|(k, _)| *k == "PATH")
            .unwrap()
            .1
            .unwrap()
            .to_owned(),
        dir.path().join("config"),
    );
    // `script` gives the command a terminal; `timeout -s INT` stands for Ctrl-C.
    let line = format!(
        "timeout -s INT 3 {} status --watch --interval 1 2>{}",
        env!("CARGO_BIN_EXE_sbxm"),
        dir.path().join("stderr").display()
    );
    let status = Command::new("script")
        .args(["-qec", &line, "/dev/null"])
        .env("PATH", bin)
        .env("SBXM_CONFIG_DIR", config)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    let _ = status;
    let stderr = std::fs::read_to_string(dir.path().join("stderr")).unwrap();
    assert!(stderr.contains("sbx: review427 watch"), "{stderr}");
}
