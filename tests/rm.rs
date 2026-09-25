mod common;

use std::path::PathBuf;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::rm;

fn metadata_dir(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("demo")
}

/// `new demo` with a file in the workspace.
fn setup() -> Env {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    std::fs::write(env.base_dir().join("demo").join("notes.md"), "keep me").unwrap();
    env
}

#[test]
fn removes_sandbox_and_state_and_keeps_workspace() {
    let env = setup();
    let backend = FakeBackend::default();

    rm::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-claude"]);
    assert!(!metadata_dir(&env).exists());
    assert_eq!(
        std::fs::read_to_string(env.base_dir().join("demo").join("notes.md")).unwrap(),
        "keep me"
    );
}

#[test]
fn keeps_other_files_in_the_metadata_dir() {
    let env = setup();
    std::fs::write(metadata_dir(&env).join("sandbox.toml"), "# mine\n").unwrap();
    let backend = FakeBackend::default();

    rm::run(&env.config_dir(), "demo", &backend).unwrap();

    assert!(!metadata_dir(&env).join("state.json").exists());
    assert_eq!(
        std::fs::read_to_string(metadata_dir(&env).join("sandbox.toml")).unwrap(),
        "# mine\n"
    );
}

#[test]
fn unknown_project_points_to_list() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = rm::run(&env.config_dir(), "demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("no sbxm sandbox for project 'demo'; `sbxm list` shows existing ones"),
        "{message}"
    );
    assert!(backend.removes().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    rm::run(&env.config_dir(), "Demo", &backend).unwrap_err();

    assert!(backend.removes().is_empty());
}

#[test]
fn failed_remove_keeps_state() {
    let env = setup();
    let backend = FakeBackend::failing_remove();

    rm::run(&env.config_dir(), "demo", &backend).unwrap_err();

    assert!(metadata_dir(&env).join("state.json").exists());
}
