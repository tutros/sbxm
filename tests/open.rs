mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::open;

fn sandbox(status: &str) -> SandboxInfo {
    SandboxInfo {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        status: status.into(),
    }
}

#[test]
fn running_sandbox_is_attached() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.log(), ["attach sbxm-demo-claude"]);
}

#[test]
fn stopped_sandbox_is_attached() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("stopped")]);

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.log(), ["attach sbxm-demo-claude"]);
}

#[test]
fn new_project_is_created_then_attached() {
    let env = Env::new();
    let backend = FakeBackend::default();

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(
        backend.log(),
        ["create sbxm-demo-claude", "attach sbxm-demo-claude"]
    );
    assert!(env.base_dir().join("demo").is_dir());
    assert!(
        env.base_dir()
            .join(".sbxm")
            .join("demo")
            .join("state.json")
            .exists()
    );
}

#[test]
fn missing_sandbox_is_recreated_keeping_the_workspace() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    std::fs::write(env.base_dir().join("demo").join("notes.md"), "keep me").unwrap();
    let backend = FakeBackend::default();

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(
        backend.log(),
        ["create sbxm-demo-claude", "attach sbxm-demo-claude"]
    );
    assert_eq!(
        std::fs::read_to_string(env.base_dir().join("demo").join("notes.md")).unwrap(),
        "keep me"
    );
}

#[test]
fn sandbox_without_state_is_refused() {
    let env = Env::new();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    let err = open::run(&env.config_dir(), "demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("sandbox sbxm-demo-claude exists but sbxm has no state for it;"),
        "{message}"
    );
    assert!(backend.log().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    open::run(&env.config_dir(), "Demo", &backend).unwrap_err();

    assert!(backend.log().is_empty());
}
