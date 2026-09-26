mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::list::{self, ConfigStatus, Entry, Problem};

fn sandbox(name: &str, agent: &str, status: &str) -> SandboxInfo {
    SandboxInfo {
        name: name.into(),
        agent: agent.into(),
        status: status.into(),
    }
}

/// Runs `sbxm new <project>` against a throwaway fake, leaving its state.
fn with_state(env: &Env, project: &str) {
    env.run(project, &FakeBackend::default()).unwrap();
}

#[test]
fn shows_sbxm_sandbox_with_state() {
    let env = Env::new();
    with_state(&env, "demo");
    let backend =
        FakeBackend::with_sandboxes(vec![sandbox("sbxm-demo-claude", "claude", "running")]);

    let entries = list::entries(&env.config_dir(), &backend).unwrap();

    assert_eq!(
        entries,
        vec![Entry {
            project: "demo".into(),
            harness: "claude".into(),
            sandbox: "sbxm-demo-claude".into(),
            status: "running".into(),
            problem: None,
            config: Some(ConfigStatus::Current),
        }]
    );
}

#[test]
fn hides_sandboxes_sbxm_did_not_create() {
    let env = Env::new();
    let backend = FakeBackend::with_sandboxes(vec![
        sandbox("spike-s6-codex", "codex", "stopped"),
        sandbox("claude-foo", "claude", "running"),
        sandbox("sbxm-x", "claude", "running"),
    ]);

    let entries = list::entries(&env.config_dir(), &backend).unwrap();

    assert_eq!(entries, vec![]);
}

#[test]
fn flags_sandbox_without_state() {
    let env = Env::new();
    let backend =
        FakeBackend::with_sandboxes(vec![sandbox("sbxm-demo-claude", "claude", "stopped")]);

    let entries = list::entries(&env.config_dir(), &backend).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].project, "demo");
    assert_eq!(entries[0].problem, Some(Problem::NoState));
    assert_eq!(entries[0].config, None);
}

#[test]
fn flags_state_without_sandbox() {
    let env = Env::new();
    with_state(&env, "demo");
    let backend = FakeBackend::with_sandboxes(vec![]);

    let entries = list::entries(&env.config_dir(), &backend).unwrap();

    assert_eq!(
        entries,
        vec![Entry {
            project: "demo".into(),
            harness: "claude".into(),
            sandbox: "sbxm-demo-claude".into(),
            status: "missing".into(),
            problem: Some(Problem::NoSandbox),
            config: Some(ConfigStatus::Current),
        }]
    );
}

fn entry(project: &str, harness: &str, status: &str, problem: Option<Problem>) -> Entry {
    Entry {
        project: project.into(),
        harness: harness.into(),
        sandbox: format!("sbxm-{project}-{harness}"),
        status: status.into(),
        problem,
        // Only entries with state have a config to check.
        config: (problem != Some(Problem::NoState)).then_some(ConfigStatus::Current),
    }
}

#[test]
fn entries_are_sorted_by_project_then_harness() {
    let env = Env::new();
    let backend = FakeBackend::with_sandboxes(vec![
        sandbox("sbxm-web-codex", "codex", "running"),
        sandbox("sbxm-api-claude", "claude", "running"),
        sandbox("sbxm-web-claude", "claude", "running"),
    ]);

    let entries = list::entries(&env.config_dir(), &backend).unwrap();

    let names: Vec<_> = entries.iter().map(|e| e.sandbox.as_str()).collect();
    assert_eq!(
        names,
        ["sbxm-api-claude", "sbxm-web-claude", "sbxm-web-codex"]
    );
}

#[test]
fn table_aligns_columns_and_explains_orphans() {
    let with_config = |mut e: Entry, config| {
        e.config = Some(config);
        e
    };
    let entries = vec![
        entry("api", "claude", "running", None),
        with_config(
            entry("demo", "claude", "stopped", None),
            ConfigStatus::Changed,
        ),
        // A missing sandbox is recreated from the current config anyway.
        with_config(
            entry("demo", "codex", "missing", Some(Problem::NoSandbox)),
            ConfigStatus::Changed,
        ),
        with_config(
            entry("web", "claude", "running", None),
            ConfigStatus::Unknown,
        ),
        entry("website", "claude", "stopped", Some(Problem::NoState)),
    ];

    assert_eq!(
        list::render_table(&entries),
        "PROJECT  HARNESS  STATUS   CONFIG   NOTE\n\
         api      claude   running  current\n\
         demo     claude   stopped  changed  config changed; `sbxm open demo --rebuild` recreates it\n\
         demo     codex    missing  changed  sandbox missing; `sbxm open demo` recreates it\n\
         web      claude   running  unknown  its profile or sandbox.toml doesn't load; `sbxm open web` shows why\n\
         website  claude   stopped           no sbxm state; not created by sbxm here\n"
    );
}

#[test]
fn empty_table_says_so() {
    assert_eq!(list::render_table(&[]), "No sbxm sandboxes.\n");
}

#[test]
fn json_has_one_object_per_entry() {
    let entries = vec![
        entry("demo", "claude", "running", None),
        entry("demo", "codex", "missing", Some(Problem::NoSandbox)),
    ];

    let json: serde_json::Value = serde_json::from_str(&list::render_json(&entries)).unwrap();

    assert_eq!(
        json,
        serde_json::json!([
            {"project": "demo", "harness": "claude", "sandbox": "sbxm-demo-claude", "status": "running", "problem": null, "config": "current"},
            {"project": "demo", "harness": "codex", "sandbox": "sbxm-demo-codex", "status": "missing", "problem": "no_sandbox", "config": "current"}
        ])
    );
}

/// The config status `list` reports for `sbxm-demo-claude`.
fn demo_config(env: &Env) -> Option<ConfigStatus> {
    let backend =
        FakeBackend::with_sandboxes(vec![sandbox("sbxm-demo-claude", "claude", "running")]);
    let entries = list::entries(&env.config_dir(), &backend).unwrap();
    entries[0].config
}

#[test]
fn changed_profile_shows_as_changed() {
    let env = Env::new();
    with_state(&env, "demo");
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");

    assert_eq!(demo_config(&env), Some(ConfigStatus::Changed));
}

#[test]
fn state_without_a_hash_shows_as_changed() {
    let env = Env::new();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(
        metadata.join("state.json"),
        r#"{"sandboxes": {"claude": {"sandbox": "sbxm-demo-claude", "workspace": "unused", "created_at": 0}}}"#,
    )
    .unwrap();

    assert_eq!(demo_config(&env), Some(ConfigStatus::Changed));
}

#[test]
fn profile_that_no_longer_loads_shows_as_unknown() {
    let env = Env::new();
    with_state(&env, "demo");
    env.write_profile("default", "not valid toml [");

    assert_eq!(demo_config(&env), Some(ConfigStatus::Unknown));
}
