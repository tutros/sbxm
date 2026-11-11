//! Issue 575: the built-in warm-up and this repo's gates build under `$HOME/target`, never
//! `/tmp`, which is a small tmpfs in a cloud sandbox (decision 257).

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::doctor;
use sbxm::commands::template_build::warm_steps;
use sbxm::task::config::TaskConfig;

const HOME_ENV: &str = "CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=$HOME/target";

#[test]
fn the_built_in_warm_up_names_home_target_unexpanded_and_never_tmp() {
    let steps = warm_steps(None);

    assert_eq!(steps.len(), 3);
    for step in steps {
        assert_eq!(step.command, format!("{HOME_ENV} {}", step.name));
        assert!(!step.command.contains("/tmp"), "{}", step.command);
    }
}

#[test]
fn a_repos_own_warm_commands_run_as_written_with_no_env_added() {
    let commands = vec!["make warm".to_owned(), "cargo fetch".to_owned()];

    let steps = warm_steps(Some(&commands));

    let run: Vec<_> = steps.iter().map(|s| s.command.as_str()).collect();
    assert_eq!(run, ["make warm", "cargo fetch"]);
}

fn host(dir: std::path::PathBuf) -> doctor::Host {
    doctor::Host {
        free_space: |_| Ok(20 * 1024 * 1024 * 1024),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
        sandbox_state_dir: None,
        now: || 1_800_000_000,
        current_dir: Some(dir),
    }
}

/// The doctor's `/tmp` warnings for a repo holding `toml` as its `sbxm-task.toml`, which must
/// run in the cloud, or the warnings it checks are never looked for.
fn tmp_warnings(env: &Env, toml: &str) -> Vec<String> {
    let repo = env.tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("sbxm-task.toml"), toml).unwrap();
    let config = TaskConfig::load(&repo).unwrap();
    assert_eq!(config.sandbox.location, Location::Cloud, "{toml}");
    let backend = FakeBackend::default().with_version("0.46.0");
    doctor::run(&env.config_dir(), &backend, &host(repo))
        .render()
        .lines()
        .filter(|l| l.contains("tmpfs"))
        .map(str::to_owned)
        .collect()
}

/// This repo's own `sbxm-task.toml` as a checkout with `newline` line endings holds it (a
/// Windows checkout can have CRLF).
fn own_toml(newline: &str) -> String {
    let own = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sbxm-task.toml"),
    )
    .unwrap();
    own.replace("\r\n", "\n").replace('\n', newline)
}

/// `toml` moved to the cloud. Matches no line ending, so LF and CRLF files both work.
fn in_the_cloud(toml: &str) -> String {
    assert!(!toml.contains("location ="), "{toml}");
    toml.replacen("[sandbox]", "[sandbox]\nlocation = \"cloud\"", 1)
}

const NEWLINES: [&str; 2] = ["\n", "\r\n"];

#[test]
fn doctor_gives_no_tmp_warning_for_this_repos_own_gates_in_the_cloud() {
    for newline in NEWLINES {
        let env = Env::new();

        let warnings = tmp_warnings(&env, &in_the_cloud(&own_toml(newline)));

        assert!(warnings.is_empty(), "{newline:?}: {warnings:?}");
    }
}

#[test]
fn a_gate_naming_tmp_target_still_warns_in_the_cloud() {
    for newline in NEWLINES {
        let env = Env::new();
        let toml = in_the_cloud(&own_toml(newline)).replacen(
            "sandbox = [",
            "sandbox = [\"CARGO_TARGET_DIR=/tmp/target cargo test\", ",
            1,
        );

        let warnings = tmp_warnings(&env, &toml);

        assert_eq!(warnings.len(), 1, "{newline:?}: {warnings:?}");
        assert!(
            warnings[0].contains("gate `CARGO_TARGET_DIR=/tmp/target cargo test`"),
            "{newline:?}: {warnings:?}"
        );
    }
}
