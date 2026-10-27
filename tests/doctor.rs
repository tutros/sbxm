//! Slice 17: `sbxm doctor` reports every check and fails if any fails
//! (decision 66).

mod common;

use common::{Env, toml_path};
use sbxm::backend::{FakeBackend, Location, SandboxDetail};
use sbxm::commands::{doctor, new};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

const GIB: u64 = 1024 * 1024 * 1024;
/// The clock of [`host`], in 2027.
const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

/// A host with 20 GiB free, a writable base dir and no temp dirs (the test
/// dirs themselves are under `%TEMP%`).
fn host() -> doctor::Host {
    doctor::Host {
        free_space: |_| Ok(20 * GIB),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
        sandbox_state_dir: None,
        now: || NOW,
        current_dir: None,
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
            "ok   20.0 GiB free on {} (sbx's sandbox state)\n",
            sbxm::disk::describe(&state)
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
            "warn only 3.0 GiB free on {} (sbx's sandbox state); free up space to \
             at least 10.0 GiB (a full drive makes sbx answer \"try: sbx login\"), or change \
             min_free_disk in {}\n",
            sbxm::disk::describe(&state),
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

    assert!(text.contains("warn only 20.0 GiB free on "), "{text}");
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
            "ok   20.0 GiB free on {} (sbx's sandbox state)\n",
            sbxm::disk::describe(&state)
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
        text.contains(&format!("on {} ", sbxm::disk::describe(&state))),
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
            "warn free space of {} (sbx's sandbox state): no statvfs; check \
             sandbox_state_dir in {}\n",
            sbxm::disk::describe(&state),
            env.config_dir().join("config.toml").display()
        )),
        "{}",
        report.render()
    );
}

const TEMPLATE_PROFILE: &str = "[[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     allow = []\n";

fn sbx_046() -> FakeBackend {
    FakeBackend::default().with_version("0.46.0")
}

/// Builds the cold template of `default` for claude, saved at `at`, and returns its record.
fn build_template(env: &Env, backend: &FakeBackend, at: u64) -> sbxm::template::Record {
    sbxm::commands::template_build::run(
        &env.config_dir(),
        &sbxm::commands::template_build::Options {
            profile: "default".into(),
            harness: sbxm::harness::Harness::Claude,
            now: Some(at),
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    sbxm::template::Record::load(&env.base_dir(), "default-claude")
        .unwrap()
        .unwrap()
}

#[test]
fn doctor_lists_each_template_with_its_size_and_age_and_the_total() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);
    let backend = sbx_046();
    let record = build_template(&env, &backend, NOW - 2 * DAY);

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    assert!(
        text.contains("ok   sbx 0.46.0 (templates need 0.46.0)\n"),
        "{text}"
    );
    let size = sbxm::disk::gib(record.size_bytes);
    assert!(
        text.contains(&format!(
            "ok   template {} (default, claude, cold): {size}, 2d00h old\n",
            record.tag
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("ok   1 template (1 image), {size} in total\n")),
        "{text}"
    );
    assert!(!text.contains("warn template"), "{text}");
}

#[test]
fn a_template_whose_image_is_gone_warns() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);
    let record = build_template(&env, &sbx_046(), NOW - DAY);

    // A backend that never saved it: `sbx template ls` doesn't list the tag.
    let report = report(&env, &sbx_046());

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn template {}: image missing; rebuild it with `sbxm template build default` or \
             remove the record with `sbxm template rm default`\n",
            record.tag
        )),
        "{}",
        report.render()
    );
}

#[test]
fn a_template_whose_profile_changed_its_build_part_warns_that_it_is_stale() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);
    let backend = sbx_046();
    let record = build_template(&env, &backend, NOW - DAY);
    env.write_profile(
        "default",
        &TEMPLATE_PROFILE.replace("ripgrep", "ripgrep jq"),
    );

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn template {} is stale: profile 'default' changed its install steps, build allow \
             list or harness kit since it was built; rebuild it with `sbxm template build default`\n",
            record.tag
        )),
        "{}",
        report.render()
    );
}

#[test]
fn a_template_older_than_14_days_warns_and_does_not_fail() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);
    let backend = sbx_046();
    let record = build_template(&env, &backend, NOW - 15 * DAY);

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    assert!(
        report.render().contains(&format!(
            "warn template {} is 15d00h old, and its agent is frozen at the version it was saved \
             with; rebuild it with `sbxm template build default`\n",
            record.tag
        )),
        "{}",
        report.render()
    );
}

#[test]
fn a_template_of_exactly_14_days_does_not_warn() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);
    let backend = sbx_046();
    build_template(&env, &backend, NOW - 14 * DAY);

    let text = report(&env, &backend).render();

    assert!(!text.contains("warn template"), "{text}");
}

#[test]
fn an_sbx_older_than_the_template_minimum_fails_when_a_profile_has_a_template() {
    let env = Env::new();
    env.write_profile("default", TEMPLATE_PROFILE);

    // The fake's default version, 0.43.0, passes the global minimum.
    let report = report(&env, &FakeBackend::default());

    assert!(report.failed());
    assert!(
        report.render().contains(
            "FAIL sbx 0.43.0 is older than 0.46.0, the version templates need (profile \
             'default' has [template]); upgrade sbx, or remove [template] from the profile\n"
        ),
        "{}",
        report.render()
    );
}

#[test]
fn an_sbx_older_than_the_template_minimum_passes_without_a_template_profile() {
    let env = Env::new();

    let report = report(&env, &FakeBackend::default());

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    assert!(!text.contains("templates need"), "{text}");
    assert!(!text.contains("template"), "{text}");
}

/// Saves the `default-claude` record with `tag` and `previous`, its own size 2 GiB.
fn save_record(env: &Env, tag: &str, previous: &[&str]) {
    env.write_profile("default", TEMPLATE_PROFILE);
    sbxm::template::Record {
        tag: tag.into(),
        template_hash: "0".repeat(64),
        warm: false,
        lock_files: Vec::new(),
        lock_hash: None,
        sbx_version: "0.46.0".into(),
        size_bytes: 2 * GIB,
        created_at: "2026-10-16T00:00:00Z".into(),
        previous: previous.iter().map(|t| (*t).to_owned()).collect(),
    }
    .save(&env.base_dir(), "default-claude")
    .unwrap();
}

/// An image `sbx template ls` reports for `tag` with `size` bytes.
fn image(tag: &str, size: u64) -> sbxm::backend::TemplateImage {
    let (name, version) = tag.split_once(':').unwrap();
    sbxm::backend::TemplateImage {
        id: "fake".into(),
        repository: format!("docker.io/library/{name}"),
        tag: version.into(),
        flavor: "fake".into(),
        created_at: "2026-10-16T00:00:00Z".into(),
        size,
    }
}

const CURRENT: &str = "sbxm-default-claude:aaaaaaaaaaaa-20261016t0000";
const OLD_1: &str = "sbxm-default-claude:bbbbbbbbbbbb-20261010t0000";
const OLD_2: &str = "sbxm-default-claude:cccccccccccc-20261001t0000";

fn total_line(env: &Env, images: Vec<sbxm::backend::TemplateImage>) -> String {
    let text = report(env, &sbx_046().with_templates(images)).render();
    text.lines()
        .find(|l| l.contains("in total"))
        .unwrap_or_else(|| panic!("no total line in {text}"))
        .to_owned()
}

#[test]
fn the_total_of_a_template_without_previous_tags_is_its_image() {
    let env = Env::new();
    save_record(&env, CURRENT, &[]);

    let line = total_line(&env, vec![image(CURRENT, 3 * GIB)]);

    assert_eq!(line, "ok   1 template (1 image), 3.0 GiB in total");
}

#[test]
fn the_total_adds_a_present_previous_image() {
    let env = Env::new();
    save_record(&env, CURRENT, &[OLD_1]);

    let line = total_line(&env, vec![image(CURRENT, 2 * GIB), image(OLD_1, 2 * GIB)]);

    assert_eq!(line, "ok   1 template (2 images), 4.0 GiB in total");
}

#[test]
fn the_total_adds_every_present_previous_image() {
    let env = Env::new();
    save_record(&env, CURRENT, &[OLD_1, OLD_2]);

    let line = total_line(
        &env,
        vec![
            image(CURRENT, 2 * GIB),
            image(OLD_1, GIB),
            image(OLD_2, GIB),
        ],
    );

    assert_eq!(line, "ok   1 template (3 images), 4.0 GiB in total");
}

#[test]
fn a_missing_previous_image_is_not_counted() {
    let env = Env::new();
    save_record(&env, CURRENT, &[OLD_1, OLD_2]);

    let line = total_line(&env, vec![image(CURRENT, 2 * GIB), image(OLD_2, GIB)]);

    assert_eq!(line, "ok   1 template (2 images), 3.0 GiB in total");
}

#[test]
fn a_missing_current_image_is_not_counted_and_still_warns() {
    let env = Env::new();
    save_record(&env, CURRENT, &[OLD_1]);

    let text = report(&env, &sbx_046().with_templates(vec![image(OLD_1, GIB)])).render();

    assert!(
        text.contains("ok   1 template (1 image), 1.0 GiB in total\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "warn template {CURRENT}: image missing; rebuild it with `sbxm template build \
             default` or remove the record with `sbxm template rm default`\n"
        )),
        "{text}"
    );
}

#[test]
fn a_failing_template_ls_falls_back_to_the_recorded_sizes() {
    let env = Env::new();
    save_record(&env, CURRENT, &[OLD_1]);

    let text = report(&env, &sbx_046().failing_template_ls()).render();

    assert!(
        text.contains(
            "ok   1 template, 2.0 GiB in total (previous images not counted: sbx template ls \
             failed)\n"
        ),
        "{text}"
    );
    assert!(text.contains("template images aren't checked"), "{text}");
}

#[test]
fn a_tag_both_current_and_previous_is_counted_once() {
    let env = Env::new();
    save_record(&env, CURRENT, &[CURRENT, OLD_1]);

    let line = total_line(&env, vec![image(CURRENT, 2 * GIB), image(OLD_1, GIB)]);

    assert_eq!(line, "ok   1 template (2 images), 3.0 GiB in total");
}

// ---- milestone 4 slice 8: the cloud (decision 259, P12, P13, P17 (a)) ----

/// [`NOW`] is 2027-01-15T08:00:00Z; this expires 1h05m after it.
const IN_1H05M: &str = "2027-01-15T09:05:00.25Z";

fn cloud_box(name: &str, expires_at: &str) -> SandboxDetail {
    SandboxDetail {
        name: name.to_owned(),
        status: "running".to_owned(),
        cpus: Some(2),
        memory_mib: Some(4096),
        expires_at: Some(expires_at.to_owned()),
    }
}

#[test]
fn doctor_lists_the_running_sbxm_cloud_sandboxes() {
    let env = Env::new();
    let backend = FakeBackend::default().with_cloud_sandboxes(vec![
        cloud_box("sbxm-issue-7-claude", IN_1H05M),
        cloud_box("sbxm-pr-3-codex", "2027-01-15T07:59:59Z"),
        cloud_box("someone-else", IN_1H05M),
    ]);

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    let cloud: Vec<&str> = text.lines().filter(|l| l.contains("cloud")).collect();
    assert_eq!(
        cloud,
        [
            "ok   cloud sandbox sbxm-issue-7-claude: running, 2 CPUs, 4 GiB, 1h05m left",
            "ok   cloud sandbox sbxm-pr-3-codex: running, 2 CPUs, 4 GiB, expiring",
        ],
        "{text}"
    );
    assert!(!text.contains("someone-else"), "{text}");
    assert_eq!(backend.locations(), [Location::Cloud]);
}

#[test]
fn doctor_with_an_empty_cloud_says_nothing_about_it() {
    let env = Env::new();

    let text = report(&env, &FakeBackend::default()).render();

    assert!(!text.to_lowercase().contains("cloud"), "{text}");
}

#[test]
fn an_unreadable_cloud_is_a_warning_and_doctor_still_passes() {
    let env = Env::new();
    let backend =
        FakeBackend::default().failing_cloud_list("sbxcloud: unauthorized (401)\nsecond line");

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    let cloud: Vec<&str> = text.lines().filter(|l| l.contains("cloud")).collect();
    assert_eq!(
        cloud,
        [
            "warn cloud sandboxes couldn't be read (`sbx --cloud ls --json`): sbxcloud: \
             unauthorized (401); if you use the cloud, log in with `sbx login`"
        ],
        "{text}"
    );
}

#[test]
fn an_unreadable_cloud_leaves_a_failing_doctor_failing() {
    let env = Env::new();
    let backend = FakeBackend::default()
        .with_version("0.1.0")
        .failing_cloud_list("sbxcloud: unauthorized (401)");

    let report = report(&env, &backend);

    assert!(report.failed(), "{}", report.render());
    assert!(
        report
            .render()
            .contains("warn cloud sandboxes couldn't be read"),
        "{}",
        report.render()
    );
}

// P13 (a) with (i), decision 262: a cloud repo's gates naming `/tmp`; P12: the cloud version.

const CLOUD_MIN: &str = "0.46.0";

/// A repo folder holding `toml` as its `sbxm-task.toml`, and a host whose current directory it is.
fn repo_with(env: &Env, toml: &str) -> (std::path::PathBuf, doctor::Host) {
    let repo = env.tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("sbxm-task.toml"), toml).unwrap();
    let host = doctor::Host {
        current_dir: Some(repo.clone()),
        ..host()
    };
    (repo, host)
}

fn gates_toml(location: &str, gates: &[&str]) -> String {
    let quoted: Vec<String> = gates.iter().map(|g| format!("{g:?}")).collect();
    format!(
        "[sandbox]\nprofile = \"default\"\nlocation = \"{location}\"\n\n[gates]\nsandbox = [{}]\n",
        quoted.join(", ")
    )
}

const BOUNDARY_GATES: [&str; 6] = [
    "CARGO_TARGET_DIR=/tmp/target cargo test",
    "cd /tmp",
    "ls /tmpfoo",
    "ls /var/tmp2",
    "CARGO_TARGET_DIR=$HOME/target cargo build",
    "cd /tmp && cargo build",
];

fn tmp_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains("tmpfs")).collect()
}

#[test]
fn a_cloud_repos_gates_naming_tmp_warn_each_and_doctor_still_passes() {
    let env = Env::new();
    let (repo, host) = repo_with(&env, &gates_toml("cloud", &BOUNDARY_GATES));
    let backend = FakeBackend::default().with_version(CLOUD_MIN);

    let report = doctor::run(&env.config_dir(), &backend, &host);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    let file = repo.join("sbxm-task.toml").display().to_string();
    let warn = |gate: &str| {
        format!(
            "warn gate `{gate}` in {file} uses /tmp, which is a small tmpfs in a cloud sandbox; \
             build under $HOME instead (e.g. $HOME/target)"
        )
    };
    assert_eq!(
        tmp_lines(&text),
        [
            warn("CARGO_TARGET_DIR=/tmp/target cargo test"),
            warn("cd /tmp"),
            warn("cd /tmp && cargo build"),
        ],
        "{text}"
    );
}

#[test]
fn a_local_repo_or_no_sbxm_task_toml_gets_no_tmp_warning() {
    let env = Env::new();
    let (_repo, local) = repo_with(&env, &gates_toml("local", &BOUNDARY_GATES));
    let no_location = doctor::Host {
        current_dir: local.current_dir.clone(),
        ..host()
    };
    let without_file = env.tmp.path().join("plain");
    std::fs::create_dir_all(&without_file).unwrap();
    let no_file = doctor::Host {
        current_dir: Some(without_file),
        ..host()
    };

    for host in [&local, &no_file, &host()] {
        let text = doctor::run(&env.config_dir(), &FakeBackend::default(), host).render();
        assert!(tmp_lines(&text).is_empty(), "{text}");
    }
    std::fs::write(
        no_location
            .current_dir
            .as_ref()
            .unwrap()
            .join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cd /tmp\"]\n",
    )
    .unwrap();
    let text = doctor::run(&env.config_dir(), &FakeBackend::default(), &no_location).render();
    assert!(tmp_lines(&text).is_empty(), "{text}");
}

fn cloud_version_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains(CLOUD_MIN)).collect()
}

#[test]
fn an_old_sbx_warns_when_the_current_repo_is_a_cloud_repo() {
    let env = Env::new();
    let (repo, host) = repo_with(&env, &gates_toml("cloud", &["cargo test"]));
    let backend = FakeBackend::default().with_version("0.45.9");

    let report = doctor::run(&env.config_dir(), &backend, &host);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    assert_eq!(
        cloud_version_lines(&text),
        [format!(
            "warn sbx 0.45.9 is older than {CLOUD_MIN}, the version a cloud task needs (the \
             cloud is in use: {} has location = \"cloud\"); upgrade sbx",
            repo.join("sbxm-task.toml").display()
        )],
        "{text}"
    );
}

#[test]
fn an_old_sbx_warns_when_a_task_record_says_cloud() {
    let env = Env::new();
    let mut task = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        NOW,
        Process::new(1, NOW),
    );
    task.location = Location::Cloud;
    record::write(&record::task_dir(&env.base_dir(), &task.id), &task).unwrap();
    let backend = FakeBackend::default().with_version("0.45.0");

    let report = report(&env, &backend);

    assert!(!report.failed(), "{}", report.render());
    let text = report.render();
    assert_eq!(
        cloud_version_lines(&text),
        [format!(
            "warn sbx 0.45.0 is older than {CLOUD_MIN}, the version a cloud task needs (the \
             cloud is in use: task issue-7 runs in the cloud); upgrade sbx"
        )],
        "{text}"
    );
}

#[test]
fn a_malformed_sibling_record_does_not_hide_a_cloud_task_from_the_version_check() {
    let env = Env::new();
    let mut task = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        NOW,
        Process::new(1, NOW),
    );
    task.location = Location::Cloud;
    record::write(&record::task_dir(&env.base_dir(), &task.id), &task).unwrap();
    // Sorted before and after `issue-7`, so the scan meets a bad record whichever order it reads in.
    for id in ["aaa-bad", "zzz-bad"] {
        let dir = record::task_dir(&env.base_dir(), id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task.json"), "{ not json").unwrap();
    }
    let backend = FakeBackend::default().with_version("0.45.0");

    let report = report(&env, &backend);

    let text = report.render();
    assert_eq!(
        cloud_version_lines(&text),
        [format!(
            "warn sbx 0.45.0 is older than {CLOUD_MIN}, the version a cloud task needs (the \
             cloud is in use: task issue-7 runs in the cloud); upgrade sbx"
        )],
        "{text}"
    );
}

#[test]
fn the_cloud_version_says_nothing_at_the_minimum_or_when_the_cloud_is_not_in_use() {
    let env = Env::new();
    let (_repo, cloud_host) = repo_with(&env, &gates_toml("cloud", &["cargo test"]));

    let at_min = doctor::run(
        &env.config_dir(),
        &FakeBackend::default().with_version(CLOUD_MIN),
        &cloud_host,
    )
    .render();
    assert!(
        !at_min.contains("the version a cloud task needs"),
        "{at_min}"
    );

    let not_in_use = report(&env, &FakeBackend::default().with_version("0.45.0")).render();
    assert!(
        !not_in_use.contains("the version a cloud task needs"),
        "{not_in_use}"
    );
}
