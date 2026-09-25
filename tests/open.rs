mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::{new, open};

fn kit_dir(env: &Env) -> std::path::PathBuf {
    env.kit_dir("demo")
}

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
        [
            format!("validate {}", kit_dir(&env).display()),
            "create sbxm-demo-claude".to_owned(),
            "attach sbxm-demo-claude".to_owned()
        ]
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
        [
            format!("validate {}", kit_dir(&env).display()),
            "create sbxm-demo-claude".to_owned(),
            "attach sbxm-demo-claude".to_owned()
        ]
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

fn state_entry(env: &Env) -> serde_json::Value {
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    state["sandboxes"]["claude"].clone()
}

#[test]
fn missing_sandbox_is_recreated_with_the_stored_profile() {
    let env = Env::new();
    env.write_profile("strict", "description = \"strict\"\n");
    let options = new::Options {
        profile: Some("strict".into()),
        ..new::Options::default()
    };
    env.run_with("demo", &options, &FakeBackend::default())
        .unwrap();
    // Loading the default profile now fails, so success means it wasn't used.
    env.write_profile("default", "not valid toml [");
    let backend = FakeBackend::default();

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
    assert_eq!(state_entry(&env)["profile"], "strict");
}

#[test]
fn missing_sandbox_without_a_stored_profile_uses_the_default() {
    let env = Env::new();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    // State as written before slice 11: no profile or hash.
    std::fs::write(
        metadata.join("state.json"),
        r#"{"sandboxes": {"claude": {"sandbox": "sbxm-demo-claude", "workspace": "unused", "created_at": 0}}}"#,
    )
    .unwrap();
    let backend = FakeBackend::default();

    open::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
    assert_eq!(state_entry(&env)["profile"], "default");
}
