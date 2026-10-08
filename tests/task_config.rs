//! M2b slice 1: `sbxm-task.toml` loading (spec §2, decisions 148, 151, 155, 160).

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use sbxm::harness::Harness;
use sbxm::task::config::{FILE_NAME, Sink, TaskConfig};
use tempfile::TempDir;

const MINIMAL: &str = "[sandbox]\nprofile = \"sbxm-dev\"\n";

/// A repo root holding `toml` as its `sbxm-task.toml`, and a `Cargo.toml` when `rust`.
fn repo(toml: &str, rust: bool) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(FILE_NAME), toml).unwrap();
    if rust {
        fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    }
    dir
}

fn load_err(toml: &str, rust: bool) -> String {
    let dir = repo(toml, rust);
    format!("{:#}", TaskConfig::load(dir.path()).unwrap_err())
}

#[test]
fn a_minimal_file_gets_the_code_defaults() {
    let dir = repo(MINIMAL, true);
    let config = TaskConfig::load(dir.path()).unwrap();

    assert_eq!(config.worker.harness, Harness::Claude);
    assert_eq!(config.worker.model, None);
    assert_eq!(config.worker.time_limit, Duration::from_secs(2 * 3600));
    assert_eq!(config.fix_rounds, 3);
    assert_eq!(config.reviewer.harness, Harness::Codex);
    assert_eq!(config.reviewer.model, None);
    assert_eq!(config.reviewer.time_limit, Duration::from_secs(45 * 60));
    assert_eq!(config.sandbox.profile, "sbxm-dev");
    assert_eq!(config.sandbox.cpus, None);
    assert_eq!(config.sandbox.memory, None);
    assert_eq!(
        config.gates.sandbox,
        [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test"
        ]
    );
    assert!(
        config.gates.host.is_empty(),
        "the host tier is off by default"
    );
    assert_eq!(config.gates.timeout, Duration::from_secs(20 * 60));
    assert_eq!(config.prompts.worker, None);
    assert!(config.warnings.is_empty());
}

#[test]
fn every_key_is_read() {
    let dir = repo(
        r#"
[worker]
harness = "codex"
model = "gpt-5.6-sol"
time_limit = "90m"
fix_rounds = 5

[reviewer]
harness = "antigravity"
model = "gemini-3.1-pro-high"
time_limit = "30s"

[sandbox]
profile = "p"
cpus = 4
memory = "8g"

[gates]
sandbox = ["make test"]
host = ["make host"]
timeout = "5m"

[prompts]
worker = "prompts/worker.md"
"#,
        false,
    );
    fs::create_dir(dir.path().join("prompts")).unwrap();
    fs::write(dir.path().join("prompts").join("worker.md"), "x").unwrap();

    let config = TaskConfig::load(dir.path()).unwrap();

    assert_eq!(config.worker.harness, Harness::Codex);
    assert_eq!(config.worker.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(config.worker.time_limit, Duration::from_secs(5400));
    assert_eq!(config.fix_rounds, 5);
    assert_eq!(config.reviewer.harness, Harness::Antigravity);
    assert_eq!(
        config.reviewer.model.as_deref(),
        Some("gemini-3.1-pro-high")
    );
    assert_eq!(config.reviewer.time_limit, Duration::from_secs(30));
    assert_eq!(
        (config.sandbox.cpus, config.sandbox.memory.as_deref()),
        (Some(4), Some("8g"))
    );
    assert_eq!(config.gates.sandbox, ["make test"]);
    assert_eq!(config.gates.host, ["make host"]);
    assert_eq!(config.gates.timeout, Duration::from_secs(300));
    assert_eq!(
        config.prompts.worker,
        Some(dir.path().join("prompts").join("worker.md"))
    );
    assert_eq!(config.prompts.reviewer, None);
}

fn same_harness_warnings(config: &TaskConfig) -> usize {
    config
        .warnings
        .iter()
        .filter(|w| w.contains("same harness"))
        .count()
}

#[test]
fn changing_the_worker_harness_re_defaults_a_reviewer_nobody_chose() {
    let dir = repo(MINIMAL, true);
    let mut config = TaskConfig::load(dir.path()).unwrap();
    assert_eq!(
        (config.worker.harness, config.reviewer.harness),
        (Harness::Claude, Harness::Codex)
    );

    config.set_worker_harness(Harness::Codex);

    // Codex can't review itself by default: the next harness in the order that differs.
    assert_eq!(
        (config.worker.harness, config.reviewer.harness),
        (Harness::Codex, Harness::Claude)
    );
    assert_eq!(same_harness_warnings(&config), 0);
}

#[test]
fn a_reviewer_set_in_the_file_stays_when_the_worker_changes_and_warns_if_they_match() {
    let dir = repo(&format!("{MINIMAL}[reviewer]\nharness = \"codex\"\n"), true);
    let mut config = TaskConfig::load(dir.path()).unwrap();
    assert_eq!(same_harness_warnings(&config), 0);

    config.set_worker_harness(Harness::Codex);

    assert_eq!(config.reviewer.harness, Harness::Codex);
    assert_eq!(same_harness_warnings(&config), 1);
    // Setting the worker again to the same value doesn't repeat the warning.
    config.set_worker_harness(Harness::Codex);
    assert_eq!(same_harness_warnings(&config), 1);
}

#[test]
fn choosing_a_reviewer_harness_warns_once_when_it_equals_the_worker_and_clears_when_it_differs() {
    let dir = repo(MINIMAL, true);
    let mut config = TaskConfig::load(dir.path()).unwrap();

    config.set_reviewer_harness(Harness::Claude);
    assert_eq!(same_harness_warnings(&config), 1);
    config.set_reviewer_harness(Harness::Claude);
    assert_eq!(same_harness_warnings(&config), 1);

    config.set_reviewer_harness(Harness::Antigravity);
    assert_eq!(same_harness_warnings(&config), 0);
    // A chosen reviewer is kept when the worker changes later.
    config.set_worker_harness(Harness::Codex);
    assert_eq!(config.reviewer.harness, Harness::Antigravity);
}

#[test]
fn a_gate_command_with_a_line_break_is_refused_naming_the_key() {
    // `sh -c` would run both lines but `cmd /C` stops at the first: never a silent drop.
    for (key, toml) in [
        (
            "gates.sandbox",
            "[gates]\nsandbox = [\"cargo fmt --check\", \"cargo test\\ncargo build\"]\n",
        ),
        (
            "gates.host",
            "[gates]\nsandbox = []\nhost = [\"cargo build\\r\\ncargo doc\"]\n",
        ),
    ] {
        let message = load_err(&format!("{MINIMAL}{toml}"), false);
        assert!(
            message.contains(key) && message.contains("line break"),
            "{message}"
        );
        assert!(message.contains(FILE_NAME), "{message}");
    }
}

#[test]
fn an_empty_gate_command_is_refused_too() {
    let message = load_err(
        &format!("{MINIMAL}[gates]\nsandbox = [\"cargo test\", \"  \"]\n"),
        false,
    );
    assert!(
        message.contains("gates.sandbox") && message.contains("empty"),
        "{message}"
    );
}

#[test]
fn a_missing_file_points_at_task_init() {
    let dir = TempDir::new().unwrap();
    let message = format!("{:#}", TaskConfig::load(dir.path()).unwrap_err());
    assert!(message.contains(FILE_NAME), "{message}");
    assert!(message.contains("sbxm task init"), "{message}");
}

#[test]
fn unknown_keys_and_tables_are_errors_naming_the_file_and_key() {
    let key = load_err(&format!("{MINIMAL}[worker]\nspeed = 1\n"), true);
    assert!(key.contains(FILE_NAME) && key.contains("speed"), "{key}");
    let table = load_err(&format!("{MINIMAL}[extras]\n"), true);
    assert!(
        table.contains(FILE_NAME) && table.contains("extras"),
        "{table}"
    );
}

#[test]
fn fix_rounds_belongs_to_the_worker_only() {
    let message = load_err(&format!("{MINIMAL}[reviewer]\nfix_rounds = 1\n"), true);
    assert!(
        message.contains(FILE_NAME) && message.contains("fix_rounds"),
        "{message}"
    );
}

#[test]
fn a_zero_fix_rounds_budget_is_allowed() {
    let dir = repo(&format!("{MINIMAL}[worker]\nfix_rounds = 0\n"), true);
    assert_eq!(TaskConfig::load(dir.path()).unwrap().fix_rounds, 0);
}

#[test]
fn the_profile_is_required() {
    for toml in ["", "[sandbox]\n", "[sandbox]\nprofile = \"  \"\n"] {
        let message = load_err(toml, true);
        assert!(
            message.contains("sandbox") && message.contains("profile"),
            "{message}"
        );
    }
}

#[test]
fn gates_without_a_cargo_toml_must_be_listed() {
    let message = load_err(MINIMAL, false);
    assert!(
        message.contains("no gates configured for this repo; list the commands in [gates] sandbox"),
        "{message}"
    );
}

#[test]
fn an_explicit_empty_gate_list_is_allowed_even_outside_a_rust_repo() {
    let dir = repo(&format!("{MINIMAL}[gates]\nsandbox = []\n"), false);
    assert!(
        TaskConfig::load(dir.path())
            .unwrap()
            .gates
            .sandbox
            .is_empty()
    );
    let rust = repo(&format!("{MINIMAL}[gates]\nsandbox = []\n"), true);
    assert!(
        TaskConfig::load(rust.path())
            .unwrap()
            .gates
            .sandbox
            .is_empty(),
        "empty is not replaced by the Rust default"
    );
}

#[test]
fn a_gates_table_without_a_sandbox_list_still_defaults_in_a_rust_repo() {
    let dir = repo(&format!("{MINIMAL}[gates]\ntimeout = \"1m\"\n"), true);
    assert_eq!(TaskConfig::load(dir.path()).unwrap().gates.sandbox.len(), 3);
}

#[test]
fn the_reviewer_defaults_to_a_different_harness() {
    for (worker, reviewer) in [
        ("claude", Harness::Codex),
        ("codex", Harness::Claude),
        ("antigravity", Harness::Codex),
    ] {
        let dir = repo(
            &format!("[worker]\nharness = \"{worker}\"\n{MINIMAL}"),
            true,
        );
        let config = TaskConfig::load(dir.path()).unwrap();
        assert_eq!(config.reviewer.harness, reviewer, "worker {worker}");
        assert!(config.warnings.is_empty());
    }
}

#[test]
fn the_same_harness_set_explicitly_warns_once_and_loads() {
    let dir = repo(
        &format!("[worker]\nharness = \"claude\"\n[reviewer]\nharness = \"claude\"\n{MINIMAL}"),
        true,
    );
    let config = TaskConfig::load(dir.path()).unwrap();
    assert_eq!(config.reviewer.harness, Harness::Claude);
    assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
    assert!(
        config.warnings[0].contains("same harness"),
        "{:?}",
        config.warnings
    );
}

#[test]
fn harness_and_duration_values_are_checked() {
    for (toml, needle) in [
        ("[worker]\nharness = \"gemini\"\n", "gemini"),
        ("[worker]\nharness = \"pi\"\n", "pi"),
        ("[reviewer]\nharness = \"vim\"\n", "not a harness"),
        ("[worker]\ntime_limit = \"2 hours\"\n", "isn't a duration"),
        ("[reviewer]\ntime_limit = \"0m\"\n", "isn't a duration"),
        ("[gates]\ntimeout = \"soon\"\n", "isn't a duration"),
    ] {
        let message = load_err(&format!("{MINIMAL}{toml}"), true);
        assert!(message.contains(needle), "{toml}: {message}");
        assert!(message.contains(FILE_NAME), "{message}");
    }
}

#[test]
fn an_empty_model_and_zero_cpus_are_errors() {
    let model = load_err(&format!("{MINIMAL}[worker]\nmodel = \"\"\n"), true);
    assert!(model.contains("worker.model"), "{model}");
    let cpus = load_err("[sandbox]\nprofile = \"p\"\ncpus = 0\n", true);
    assert!(cpus.contains("sandbox.cpus"), "{cpus}");
}

fn prompt_error(path: &str, setup: impl Fn(&Path)) -> String {
    let dir = repo(&format!("{MINIMAL}[prompts]\nworker = '{path}'\n"), true);
    setup(dir.path());
    format!("{:#}", TaskConfig::load(dir.path()).unwrap_err())
}

#[test]
fn a_prompt_path_must_stay_inside_the_repo() {
    let up = prompt_error("../outside.md", |_| {});
    assert!(
        up.contains("prompts.worker") && up.contains("inside"),
        "{up}"
    );
    let absolute = if cfg!(windows) {
        "C:/x/p.md"
    } else {
        "/x/p.md"
    };
    let abs = prompt_error(absolute, |_| {});
    assert!(
        abs.contains("prompts.worker") && abs.contains("inside"),
        "{abs}"
    );
}

#[test]
fn a_prompt_path_must_be_an_existing_file() {
    let missing = prompt_error("prompts/none.md", |_| {});
    assert!(
        missing.contains("prompts.worker") && missing.contains("none.md"),
        "{missing}"
    );
}

#[test]
fn a_prompt_path_may_not_pass_through_a_link() {
    let message = prompt_error("linked/p.md", |root| {
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("p.md"), "x").unwrap();
        common::dir_link(&root.join("linked"), &real);
    });
    assert!(message.contains("symlink or junction"), "{message}");
}

/// Issue 143 (decision 174(e)): `[finish] sink` says where a spec task's result lands.
#[test]
fn the_finish_sink_defaults_to_local_and_accepts_push() {
    let dir = repo(MINIMAL, true);
    assert_eq!(TaskConfig::load(dir.path()).unwrap().sink, Sink::Local);

    for (value, sink) in [("local", Sink::Local), ("push", Sink::Push)] {
        let dir = repo(&format!("{MINIMAL}\n[finish]\nsink = \"{value}\"\n"), true);
        assert_eq!(TaskConfig::load(dir.path()).unwrap().sink, sink, "{value}");
    }
}

#[test]
fn an_unknown_sink_or_finish_key_is_an_error_naming_the_file() {
    for toml in [
        format!("{MINIMAL}\n[finish]\nsink = \"github\"\n"),
        format!("{MINIMAL}\n[finish]\ndestination = \"local\"\n"),
    ] {
        let message = load_err(&toml, true);
        assert!(message.contains(FILE_NAME), "{message}");
        assert!(
            message.contains("github") || message.contains("destination"),
            "{message}"
        );
    }
}
