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
