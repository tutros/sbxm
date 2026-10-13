//! Slice 17: `sbxm doctor` reports every check and fails if any fails
//! (decision 66).

mod common;

use common::{Env, toml_path};
use sbxm::backend::FakeBackend;
use sbxm::commands::{doctor, new};

const GIB: u64 = 1024 * 1024 * 1024;

/// A host with 20 GiB free, a writable base dir and no temp dirs (the test
/// dirs themselves are under `%TEMP%`).
fn host() -> doctor::Host {
    doctor::Host {
        free_space: |_| Ok(20 * GIB),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
        sandbox_state_dir: None,
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
        report.render().contains(
            "FAIL free space for base dir: no statvfs; check that base_dir is on a drive sbx can use"
        ),
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

/// Writes `.sbxm/demo/sandbox.toml`.
fn write_project_config(env: &Env, contents: &str) {
    let dir = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sandbox.toml"), contents).unwrap();
}

#[test]
fn every_profile_is_checked() {
    let env = Env::new();
    env.write_profile("other", "[env]\nA = \"1\"\n");

    let report = report(&env, &FakeBackend::default());

    let text = report.render();
    assert!(text.contains("ok   profile 'default'\n"), "{text}");
    assert!(text.contains("ok   profile 'other'\n"), "{text}");
}

#[test]
fn broken_profile_fails() {
    let env = Env::new();
    env.write_profile("broken", "[netwrk]\n");

    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    let text = report.render();
    assert!(
        text.contains("FAIL profile 'broken': invalid profile "),
        "{text}"
    );
    assert!(text.contains("ok   profile 'default'\n"), "{text}");
}

#[test]
fn missing_secret_fails() {
    let env = Env::new();
    env.write_profile("default", "[secrets]\nservices = [\"github\"]\n");

    let report = report(&env, &FakeBackend::with_secrets(&["anthropic"]));

    assert!(report.failed());
    assert!(
        report
            .render()
            .contains("FAIL profile 'default': secret 'github' (secrets.services) is not stored"),
        "{}",
        report.render()
    );
}

#[test]
fn invalid_kit_fails() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::with_invalid_kit("bad network pattern"));

    assert!(report.failed());
    let profile_toml = env
        .profiles_dir()
        .join("default")
        .join("profile.toml")
        .display()
        .to_string();
    let text = report.render();
    assert!(
        text.contains(&format!(
            "FAIL profile 'default': generated kit common is invalid: bad network pattern; \
             check profile 'default' ({profile_toml})"
        )),
        "{text}"
    );
}

/// Decision from #4/#6: a project's invalid-kit failure names its recorded
/// profile and, when it has one, its `sandbox.toml`, the same way `new` does.
/// Uses a non-default profile so the profile name in the message can only
/// have come from the project's own recorded profile, not the bare-profile
/// check that also runs (and also fails) against `default`.
#[test]
fn invalid_kit_for_a_project_names_its_sandbox_toml() {
    let env = Env::new();
    env.write_profile("custom", "description = \"test custom\"\n");
    env.run_with(
        "demo",
        &new::Options {
            profile: Some("custom".into()),
            ..new::Options::default()
        },
        &FakeBackend::default(),
    )
    .unwrap();
    write_project_config(&env, "");

    let report = report(&env, &FakeBackend::with_invalid_kit("manifest: bad host"));

    assert!(report.failed());
    let profile_toml = env
        .profiles_dir()
        .join("custom")
        .join("profile.toml")
        .display()
        .to_string();
    let sandbox_toml = env
        .base_dir()
        .join(".sbxm")
        .join("demo")
        .join("sandbox.toml")
        .display()
        .to_string();
    let text = report.render();
    // The full line, not just each path's presence anywhere in the report:
    // the bare `profile 'custom'` check also fails and also names
    // profile_toml, so checking each path independently wouldn't catch a
    // regression that drops one of them from the project's own line.
    assert!(
        text.contains(&format!(
            "FAIL project demo (claude, profile 'custom'): generated kit common is invalid: \
             manifest: bad host; check profile 'custom' ({profile_toml}) and the project's \
             sandbox.toml ({sandbox_toml})"
        )),
        "{text}"
    );
}

#[test]
fn kits_are_validated_in_a_temp_dir_that_is_removed() {
    let env = Env::new();
    let backend = FakeBackend::default();

    report(&env, &backend);

    let log = backend.log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].starts_with("validate ") && log[0].ends_with("common"));
    assert!(log[1].starts_with("validate ") && log[1].ends_with("harness-claude"));
    for line in &log {
        let dir = std::path::Path::new(line.strip_prefix("validate ").unwrap());
        assert!(!dir.exists(), "{} was left behind", dir.display());
    }
    assert!(!env.base_dir().join(".sbxm").exists());
}

#[test]
fn projects_are_checked_with_their_recorded_profile() {
    let env = Env::new();
    env.write_profile("other", "[env]\nA = \"1\"\n");
    let options = sbxm::commands::new::Options {
        profile: Some("other".into()),
        ..Default::default()
    };
    env.run_with("demo", &options, &FakeBackend::default())
        .unwrap();

    let text = report(&env, &FakeBackend::default()).render();

    assert!(
        text.contains("ok   project demo (claude, profile 'other')\n"),
        "{text}"
    );
}

#[test]
fn broken_project_config_fails() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    write_project_config(&env, "[env]\n1BAD = \"x\"\n");

    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    assert!(
        report
            .render()
            .contains("FAIL project demo (claude, profile 'default'): invalid env name '1BAD'"),
        "{}",
        report.render()
    );
}

#[test]
fn missing_profiles_dir_fails() {
    let env = Env::new();
    std::fs::remove_dir_all(env.profiles_dir()).unwrap();

    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    assert!(
        report.render().contains(&format!(
            "FAIL profiles dir {} does not exist; run `sbxm config init` or change profiles_dir in {}",
            env.profiles_dir().display(),
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}

/// Decision 71: every harness recorded for a project is checked with its own kits.
#[test]
fn every_recorded_harness_of_a_project_is_checked() {
    let env = Env::new();
    let codex = sbxm::commands::new::Options {
        harness: sbxm::harness::Harness::Codex,
        ..Default::default()
    };
    env.run_with("demo", &codex, &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::default();

    let text = report(&env, &backend).render();

    assert!(
        text.contains("ok   project demo (codex, profile 'default')\n"),
        "{text}"
    );
    assert!(
        backend.log().iter().any(|l| l.ends_with("harness-codex")),
        "{:?}",
        backend.log()
    );
}

#[test]
fn unknown_harness_in_state_fails() {
    let env = Env::new();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(
        metadata.join("state.json"),
        r#"{"sandboxes": {"opencode": {"sandbox": "sbxm-demo-opencode", "workspace": "unused", "created_at": 0}}}"#,
    )
    .unwrap();

    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    assert!(
        report.render().contains(&format!(
            "FAIL project demo (opencode, profile 'default'): sbxm doesn't know this harness; \
             use an sbxm version that does, or remove its entry from {}",
            metadata.join("state.json").display()
        )),
        "{}",
        report.render()
    );
}

// Issue 210 (decision 194): the drive of sbx's sandbox state.

/// An env whose `sandbox_state_dir` is an existing folder.
fn env_with_state() -> (Env, std::path::PathBuf) {
    let env = Env::new();
    let state = env.tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    env.set_config(&format!("sandbox_state_dir = {}", toml_path(&state)));
    (env, state)
}

#[test]
fn the_sandbox_drive_free_space_is_reported() {
    let (env, state) = env_with_state();

    let report = report(&env, &FakeBackend::default());

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "ok   20.0 GiB free on the drive of {} (sbx's sandbox state)\n",
            state.display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn a_low_sandbox_drive_warns_without_failing() {
    let (env, state) = env_with_state();
    // Only the state folder is low; the base dir has room.
    let host = doctor::Host {
        free_space: |path| {
            Ok(if path.ends_with("state") {
                3 * GIB
            } else {
                20 * GIB
            })
        },
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn only 3.0 GiB free on the drive of {} (sbx's sandbox state); free up space to \
             at least 10.0 GiB (a full drive makes sbx answer \"try: sbx login\"), or change \
             min_free_disk in {}\n",
            state.display(),
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn the_threshold_comes_from_min_free_disk() {
    let (env, _) = env_with_state();
    env.set_config("min_free_disk = \"30g\"");

    let text = report(&env, &FakeBackend::default()).render();

    assert!(
        text.contains("warn only 20.0 GiB free on the drive of "),
        "{text}"
    );
    assert!(text.contains("at least 30.0 GiB"), "{text}");
}

#[test]
fn a_zero_threshold_says_the_check_is_off() {
    let (env, _) = env_with_state();
    env.set_config("min_free_disk = \"0\"");

    let text = report(&env, &FakeBackend::default()).render();

    assert!(
        text.contains(&format!(
            "ok   sandbox drive not checked: min_free_disk is \"0\" in {}\n",
            env.config_dir().join("config.toml").display()
        )),
        "{text}"
    );
}

#[test]
fn without_a_known_state_folder_doctor_says_so_and_names_the_key() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::default());

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "skip sandbox drive not checked: sbxm knows where sbx keeps its sandboxes only on \
             Windows; set sandbox_state_dir in {} to check it\n",
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn the_platform_default_folder_is_checked() {
    let env = Env::new();
    let state = env.tmp.path().join("DockerSandboxes");
    std::fs::create_dir(&state).unwrap();
    let host = doctor::Host {
        sandbox_state_dir: Some(state.clone()),
        ..host()
    };

    let text = report_with(&env, &host).render();

    assert!(
        text.contains(&format!(
            "ok   20.0 GiB free on the drive of {} (sbx's sandbox state)\n",
            state.display()
        )),
        "{text}"
    );
}

#[test]
fn sandbox_state_dir_overrides_the_platform_default() {
    let (env, state) = env_with_state();
    let host = doctor::Host {
        sandbox_state_dir: Some(env.tmp.path().join("elsewhere")),
        ..host()
    };

    let text = report_with(&env, &host).render();

    assert!(
        text.contains(&format!("on the drive of {} ", state.display())),
        "{text}"
    );
}

#[test]
fn a_missing_state_folder_warns() {
    let env = Env::new();
    let missing = env.tmp.path().join("missing");
    env.set_config(&format!("sandbox_state_dir = {}", toml_path(&missing)));

    let report = report(&env, &FakeBackend::default());

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn sbx's sandbox state folder {} does not exist, so its drive isn't checked; \
             check sandbox_state_dir in {}, or that sbx has run on this machine\n",
            missing.display(),
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}

#[test]
fn an_unreadable_sandbox_drive_warns() {
    let (env, state) = env_with_state();
    let host = doctor::Host {
        free_space: |path| {
            if path.ends_with("state") {
                Err(std::io::Error::other("no statvfs"))
            } else {
                Ok(20 * GIB)
            }
        },
        ..host()
    };

    let report = report_with(&env, &host);

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn free space of the drive of {} (sbx's sandbox state): no statvfs; check \
             sandbox_state_dir in {}\n",
            state.display(),
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}
