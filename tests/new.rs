use std::path::{Path, PathBuf};

mod common;

use common::{Env, dir_link};
use sbxm::backend::{CreateSpec, FakeBackend};
use sbxm::commands::new;

fn expected_create(workspace: &Path) -> CreateSpec {
    CreateSpec {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        workspace: workspace.to_path_buf(),
        cpus: 4,
        memory: "8g".into(),
    }
}

#[test]
fn creates_workspace_and_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let workspace = env.base_dir().join("demo");
    assert!(workspace.is_dir());
    assert_eq!(backend.creates(), vec![expected_create(&workspace)]);
}

#[test]
fn reuses_existing_workspace_and_keeps_its_contents() {
    let env = Env::new();
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.md"), "keep me").unwrap();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "keep me"
    );
    assert_eq!(backend.creates(), vec![expected_create(&workspace)]);
}

#[test]
fn missing_base_dir_is_a_clear_error_and_creates_nothing() {
    let env = Env::new();
    std::fs::remove_dir(env.base_dir()).unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("base dir"), "{message}");
    assert!(
        message.contains(&env.base_dir().display().to_string()),
        "{message}"
    );
    assert!(!env.base_dir().exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn missing_config_points_to_config_init() {
    let env = Env::new();
    std::fs::remove_file(env.config_dir().join("config.toml")).unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("sbxm config init"), "{message}");
    assert!(backend.creates().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("Demo", &backend).unwrap_err();

    assert!(backend.creates().is_empty());
    assert!(std::fs::read_dir(env.base_dir()).unwrap().next().is_none());
}

fn state_path(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("demo").join("state.json")
}

#[test]
fn writes_state_for_the_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_path(&env)).unwrap()).unwrap();
    let claude = &state["sandboxes"]["claude"];
    assert_eq!(claude["sandbox"], "sbxm-demo-claude");
    assert_eq!(
        claude["workspace"],
        env.base_dir().join("demo").to_str().unwrap()
    );
    assert!(claude["created_at"].as_u64().unwrap() > 0);
}

#[test]
fn failed_create_writes_no_state() {
    let env = Env::new();
    let backend = FakeBackend::failing_create();

    env.run("demo", &backend).unwrap_err();

    assert!(!state_path(&env).exists());
}

fn seeded(seed: PathBuf) -> new::Options {
    new::Options {
        seed: Some(seed),
        ..Default::default()
    }
}

#[test]
fn seed_is_copied_into_a_new_workspace() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run_with("demo", &seeded(env.seed()), &backend).unwrap();

    let workspace = env.base_dir().join("demo");
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "a"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("sub").join("b.txt")).unwrap(),
        "b"
    );
    assert_eq!(backend.creates(), vec![expected_create(&workspace)]);
}

#[test]
fn seed_on_existing_project_is_an_error_and_changes_nothing() {
    let env = Env::new();
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.md"), "keep me").unwrap();
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(env.seed()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("already exists"), "{message}");
    assert!(message.contains("without --seed"), "{message}");
    assert!(backend.creates().is_empty());
    let names: Vec<_> = std::fs::read_dir(&workspace)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["notes.md"]);
}

#[test]
fn missing_seed_is_an_error_and_creates_nothing() {
    let env = Env::new();
    let missing = env.tmp.path().join("no-such-seed");
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(missing.clone()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn seed_containing_the_base_dir_is_an_error() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(env.tmp.path().to_path_buf()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("contains the base dir"), "{message}");
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn seed_containing_a_link_is_an_error_and_copies_nothing() {
    let env = Env::new();
    let secret = env.tmp.path().join("secret");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(secret.join("key.txt"), "secret").unwrap();
    let seed = env.seed();
    dir_link(&seed.join("sub").join("linked"), &secret);
    let backend = FakeBackend::default();

    let err = env.run_with("demo", &seeded(seed), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("is a symlink or junction"), "{message}");
    assert!(message.contains("linked"), "{message}");
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
}
