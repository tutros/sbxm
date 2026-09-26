//! Slice 17: `sbxm doctor` reports every check and fails if any fails
//! (decision 66).

mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::doctor;

const GIB: u64 = 1024 * 1024 * 1024;

/// A host with 20 GiB free, a writable base dir and no temp dirs (the test
/// dirs themselves are under `%TEMP%`).
fn host() -> doctor::Host {
    doctor::Host {
        free_space: |_| Ok(20 * GIB),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
    }
}

fn report(env: &Env, backend: &FakeBackend) -> doctor::Report {
    doctor::run(&env.config_dir(), backend, &host())
}

fn report_with(env: &Env, host: &doctor::Host) -> doctor::Report {
    doctor::run(&env.config_dir(), &FakeBackend::default(), host)
}

#[test]
fn healthy_base_dir_passes() {
    let env = Env::new();

    let text = report(&env, &FakeBackend::default()).render();

    let base = env.base_dir().display().to_string();
    assert!(text.contains(&format!("ok   base dir {base}\n")), "{text}");
    assert!(
        text.contains(&format!("ok   20.0 GiB free for base dir {base}\n")),
        "{text}"
    );
}

#[test]
fn missing_base_dir_fails_and_skips_its_other_checks() {
    let env = Env::new();
    std::fs::remove_dir(env.base_dir()).unwrap();

    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    let text = report.render();
    assert!(
        text.contains(&format!(
            "FAIL base dir {} does not exist; create it or change base_dir in {}",
            env.base_dir().display(),
            env.config_dir().join("config.toml").display()
        )),
        "{text}"
    );
    assert!(!text.contains("free for base dir"), "{text}");
}

#[test]
fn unwritable_base_dir_fails() {
    let env = Env::new();
    let host = doctor::Host {
        writable: |_| Err(std::io::Error::other("access denied")),
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(report.failed());
    assert!(
        report.render().contains(&format!(
            "FAIL base dir {} is not writable (access denied); fix its permissions or change base_dir",
            env.base_dir().display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn base_dir_under_a_temp_dir_fails() {
    let env = Env::new();
    let host = doctor::Host {
        temp_dirs: vec![env.tmp.path().to_owned()],
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(report.failed());
    assert!(
        report.render().contains(&format!(
            "FAIL base dir {} is under {}, where sbx can't mount workspaces; change base_dir",
            env.base_dir().display(),
            env.tmp.path().display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn low_free_space_fails() {
    let env = Env::new();
    let host = doctor::Host {
        free_space: |_| Ok(5 * GIB / 2),
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(report.failed());
    assert!(
        report.render().contains(&format!(
            "FAIL only 2.5 GiB free for base dir {}; free up space to at least 10 GiB",
            env.base_dir().display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn unknown_free_space_fails() {
    let env = Env::new();
    let host = doctor::Host {
        free_space: |_| Err(std::io::Error::other("no statvfs")),
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(report.failed());
    assert!(
        report
            .render()
            .contains("FAIL free space for base dir: no statvfs"),
        "{}",
        report.render()
    );
}

/// Appends `line` to the test config.
fn add_to_config(env: &Env, line: &str) {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{line}\n{text}")).unwrap();
}

#[test]
fn healthy_setup_passes() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::default());

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    assert!(text.contains("ok   config "), "{text}");
    assert!(
        text.contains("ok   sbx 0.43.0 (min_sbx_version 0.43.0)"),
        "{text}"
    );
    assert!(text.contains("ok   sbx daemon answers"), "{text}");
}

#[test]
fn old_sbx_fails_with_the_fix() {
    let env = Env::new();
    add_to_config(&env, "min_sbx_version = \"0.44.0\"");

    let report = report(&env, &FakeBackend::default().with_version("0.43.0"));

    assert!(report.failed());
    let text = report.render();
    assert!(
        text.contains(
            "FAIL sbx 0.43.0 is older than min_sbx_version 0.44.0; upgrade sbx or lower \
             min_sbx_version in "
        ),
        "{text}"
    );
}

#[test]
fn versions_compare_numerically() {
    let env = Env::new();
    add_to_config(&env, "min_sbx_version = \"0.9.0\"");

    let report = report(&env, &FakeBackend::default().with_version("0.43.0"));

    assert!(!report.failed(), "{}", report.render());
}

#[test]
fn missing_sbx_fails() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::default().without_sbx());

    assert!(report.failed());
    assert!(
        report
            .render()
            .contains("FAIL sbx version: fake: sbx not found"),
        "{}",
        report.render()
    );
}

#[test]
fn unreachable_daemon_fails() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::default().failing_list());

    assert!(report.failed());
    assert!(
        report
            .render()
            .contains("FAIL sbx daemon: fake: daemon not running"),
        "{}",
        report.render()
    );
}

#[test]
fn missing_config_fails_without_calling_sbx() {
    let env = Env::new();
    std::fs::remove_file(env.config_dir().join("config.toml")).unwrap();
    let backend = FakeBackend::default().without_sbx();

    let report = report(&env, &backend);

    assert!(report.failed());
    let text = report.render();
    assert!(text.contains("FAIL config: no config at"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}

#[test]
fn cli_exits_non_zero_on_failure() {
    let tmp = tempfile::TempDir::new().unwrap();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", tmp.path())
        .arg("doctor")
        .assert()
        .failure()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("FAIL config: no config at"), "{stdout}");
}
