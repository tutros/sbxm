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
        &["stop", "demo"][..],
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
