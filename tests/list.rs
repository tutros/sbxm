mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::list::{self, Entry, Problem};

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
        }]
    );
}
