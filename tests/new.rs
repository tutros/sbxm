use std::path::{Path, PathBuf};

use sbxm::backend::{CreateSpec, FakeBackend};
use sbxm::commands::new;
use tempfile::TempDir;

/// A temp dir holding `config/config.toml` whose `base_dir` is `base/`.
struct Env {
    tmp: TempDir,
}

impl Env {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let env = Env { tmp };
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::create_dir_all(env.base_dir()).unwrap();
        let base = toml::Value::String(env.base_dir().to_str().unwrap().to_owned());
        std::fs::write(
            env.config_dir().join("config.toml"),
            format!("base_dir = {base}\n\n[resources]\ncpus = 4\nmemory = \"8g\"\n"),
        )
        .unwrap();
        env
    }

    fn config_dir(&self) -> PathBuf {
        self.tmp.path().join("config")
    }

    fn base_dir(&self) -> PathBuf {
        self.tmp.path().join("base")
    }

    fn run(&self, project: &str, backend: &FakeBackend) -> anyhow::Result<()> {
        new::run(&self.config_dir(), project, backend)
    }
}

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
