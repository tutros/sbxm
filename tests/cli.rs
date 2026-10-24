use assert_cmd::Command;

fn sbxm() -> Command {
    Command::cargo_bin("sbxm").unwrap()
}

#[test]
fn version_prints_crate_version() {
    sbxm()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("sbxm {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_prints_usage() {
    let output = sbxm().arg("--help").assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Usage: sbxm "),
        "unexpected help output:\n{stdout}"
    );
}

#[test]
fn commands_without_config_point_to_config_init() {
    let tmp = tempfile::TempDir::new().unwrap();
    for args in [
        &["list"][..],
        &["list", "--json"][..],
        &["stop", "demo", "--harness", "claude"][..],
        &["open", "demo", "--rebuild", "--harness", "claude"][..],
    ] {
        let output = sbxm()
            .env("SBXM_CONFIG_DIR", tmp.path())
            .args(args)
            .assert()
            .failure()
            .get_output()
            .clone();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("sbxm config init"), "{args:?}: {stderr}");
    }
}

#[test]
fn rm_yes_requires_purge() {
    let output = sbxm()
        .args(["rm", "demo", "--yes"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--purge"), "{stderr}");
}

/// Issue 202, decision 195 point 3: `--force` is refused together with `--merged`.
#[test]
fn task_rm_force_and_merged_conflict() {
    let output = sbxm()
        .args(["task", "rm", "--merged", "--force"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--force"), "{stderr}");
    assert!(stderr.contains("--merged"), "{stderr}");
}

/// `--dry-run` only makes sense with `--merged`.
#[test]
fn task_rm_dry_run_requires_merged() {
    let output = sbxm()
        .args(["task", "rm", "--issue", "1", "--dry-run"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--dry-run"), "{stderr}");
}

/// `--issue` and `--merged` name different, mutually exclusive ways to pick what `task rm`
/// removes.
#[test]
fn task_rm_issue_and_merged_conflict() {
    let output = sbxm()
        .args(["task", "rm", "--issue", "1", "--merged"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn purge_without_terminal_refuses_and_deletes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(base.join("demo")).unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        tmp.path().join("config.toml"),
        format!("base_dir = {base_toml}\n[resources]\ncpus = 1\nmemory = \"1g\"\n"),
    )
    .unwrap();

    let output = sbxm()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["rm", "demo", "--purge"])
        .assert()
        .failure()
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("pass --yes"), "{stderr}");
    assert!(base.join("demo").is_dir());
}

#[test]
fn purge_with_harness_is_rejected_and_deletes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(base.join("demo")).unwrap();
    let base_toml = toml::Value::String(base.to_str().unwrap().to_owned());
    std::fs::write(
        tmp.path().join("config.toml"),
        format!("base_dir = {base_toml}\n[resources]\ncpus = 1\nmemory = \"1g\"\n"),
    )
    .unwrap();

    let output = sbxm()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["rm", "demo", "--purge", "--yes", "--harness", "codex"])
        .assert()
        .failure()
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("'--purge' cannot be used with '--harness <HARNESS>'"),
        "{stderr}"
    );
    assert!(base.join("demo").is_dir());
}

#[test]
fn commands_on_an_existing_sandbox_require_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    for args in [
        &["rm", "demo"][..],
        &["stop", "demo"][..],
        &["open", "demo"][..],
    ] {
        let output = sbxm()
            .env("SBXM_CONFIG_DIR", tmp.path())
            .args(args)
            .assert()
            .failure()
            .get_output()
            .clone();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("required arguments were not provided") && stderr.contains("--harness"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn rm_purge_does_not_require_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["rm", "demo", "--purge", "--yes"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("required arguments"), "{stderr}");
}

/// Issue 198: `task review --force` only overrides the already-reviewed check of a PR.
#[test]
fn task_review_force_needs_pr() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["task", "review", "--issue", "4", "--force"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot be used with '--force'"), "{stderr}");
}

/// Issue 210: every command that creates a sandbox takes `--ignore-disk`.
#[test]
fn sandbox_creating_commands_take_ignore_disk() {
    for args in [
        &["new", "--help"][..],
        &["open", "--help"],
        &["run", "--help"],
        &["task", "start", "--help"],
        &["task", "review", "--help"],
        &["task", "run", "--help"],
        &["task", "resume", "--help"],
    ] {
        let output = sbxm().args(args).assert().success().get_output().clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("--ignore-disk"), "{args:?}:\n{stdout}");
    }
}

/// Milestone 4 slice 1 (P14, decision 262(k)): `task start` runs here or in the cloud, never both.
#[test]
fn task_start_cloud_and_local_conflict() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .args(["task", "start", "--spec", "idea.md", "--cloud", "--local"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("'--cloud' cannot be used with '--local'"),
        "{stderr}"
    );
}
