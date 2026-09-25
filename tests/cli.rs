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
