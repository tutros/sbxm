mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::{new, open};

fn kit_dir(env: &Env) -> std::path::PathBuf {
    env.kit_dir("demo")
}

fn open_demo(env: &Env, backend: &FakeBackend) -> anyhow::Result<()> {
    open::run(
        &env.config_dir(),
        "demo",
        &open::Options::default(),
        backend,
        &mut std::io::sink(),
    )
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

    open_demo(&env, &backend).unwrap();

    assert_eq!(backend.log(), ["attach sbxm-demo-claude"]);
}

#[test]
fn stopped_sandbox_is_attached() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("stopped")]);

    open_demo(&env, &backend).unwrap();

    assert_eq!(backend.log(), ["attach sbxm-demo-claude"]);
}

#[test]
fn new_project_is_created_then_attached() {
    let env = Env::new();
    let backend = FakeBackend::default();

    open_demo(&env, &backend).unwrap();

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

    open_demo(&env, &backend).unwrap();

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

    let err = open_demo(&env, &backend).unwrap_err();

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

    open::run(
        &env.config_dir(),
        "Demo",
        &open::Options::default(),
        &backend,
        &mut std::io::sink(),
    )
    .unwrap_err();

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

    open_demo(&env, &backend).unwrap();

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

    open_demo(&env, &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
    assert_eq!(state_entry(&env)["profile"], "default");
}

#[test]
fn changed_config_is_refused_without_rebuild() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    let err = open_demo(&env, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "the config of sbxm-demo-claude (profile 'default') changed since it was created; \
             run `sbxm open demo --rebuild` to recreate it (its session history is lost; \
             the workspace is kept)"
        ),
        "{message}"
    );
    assert!(backend.log().is_empty());
}

#[test]
fn state_without_a_hash_is_refused_without_rebuild() {
    let env = Env::new();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(
        metadata.join("state.json"),
        r#"{"sandboxes": {"claude": {"sandbox": "sbxm-demo-claude", "workspace": "unused", "created_at": 0}}}"#,
    )
    .unwrap();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    let err = open_demo(&env, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "sbxm-demo-claude was created before sbxm recorded config hashes; \
             run `sbxm open demo --rebuild` to recreate it"
        ),
        "{message}"
    );
    assert!(backend.log().is_empty());
}

/// `open demo --rebuild`, returning what it wrote to `warn`.
fn rebuild_demo(env: &Env, backend: &FakeBackend) -> (anyhow::Result<()>, String) {
    let mut warn = Vec::new();
    let result = open::run(
        &env.config_dir(),
        "demo",
        &open::Options { rebuild: true },
        backend,
        &mut warn,
    );
    (result, String::from_utf8(warn).unwrap())
}

/// `kits/<prefix>/common` for the hash in state.
fn state_kit_dir(env: &Env) -> std::path::PathBuf {
    let hash = state_entry(env)["config_hash"].as_str().unwrap().to_owned();
    env.base_dir()
        .join(".sbxm")
        .join("demo")
        .join("kits")
        .join(&hash[..12])
        .join("common")
}

#[test]
fn rebuild_recreates_the_sandbox_from_the_current_config() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let old_hash = state_entry(&env)["config_hash"].clone();
    let workspace = env.base_dir().join("demo");
    std::fs::write(workspace.join("notes.md"), "keep me").unwrap();
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    let (result, warn) = rebuild_demo(&env, &backend);

    result.unwrap();
    assert_ne!(state_entry(&env)["config_hash"], old_hash);
    assert_eq!(
        backend.log(),
        [
            format!("validate {}", state_kit_dir(&env).display()),
            "rm sbxm-demo-claude".to_owned(),
            "create sbxm-demo-claude".to_owned(),
            "attach sbxm-demo-claude".to_owned()
        ]
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "keep me"
    );
    assert!(
        warn.contains(&format!(
            "rebuilding sbxm-demo-claude: its session history will be lost; the workspace {} is kept",
            workspace.display()
        )),
        "{warn}"
    );
}

#[test]
fn rebuild_with_an_invalid_kit_keeps_the_old_sandbox() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::with_invalid_kit("bad").and_sandboxes(vec![sandbox("running")]);

    let (result, _) = rebuild_demo(&env, &backend);

    result.unwrap_err();
    assert!(backend.removes().is_empty());
    assert!(backend.creates().is_empty());
}

fn kits_dir(env: &Env) -> std::path::PathBuf {
    env.base_dir().join(".sbxm").join("demo").join("kits")
}

#[test]
fn rebuild_deletes_the_old_kit_dir() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    rebuild_demo(&env, &backend).0.unwrap();

    assert_eq!(env.kit_dir("demo"), state_kit_dir(&env));
}

#[test]
fn rebuild_without_a_change_keeps_the_kit_dir() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    rebuild_demo(&env, &backend).0.unwrap();

    assert!(state_kit_dir(&env).join("spec.yaml").is_file());
}

#[test]
fn rebuild_keeps_an_old_kit_dir_another_harness_uses() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let old_kit = env.kit_dir("demo");
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let codex = state["sandboxes"]["claude"].clone();
    state["sandboxes"]["codex"] = codex;
    std::fs::write(&path, state.to_string()).unwrap();
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    rebuild_demo(&env, &backend).0.unwrap();

    assert!(old_kit.join("spec.yaml").is_file());
    assert_eq!(std::fs::read_dir(kits_dir(&env)).unwrap().count(), 2);
}

#[test]
fn rebuild_refuses_to_delete_an_old_kit_dir_that_is_a_link() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let old_dir = env.kit_dir("demo").parent().unwrap().to_path_buf();
    let target = env.tmp.path().join("elsewhere");
    std::fs::rename(&old_dir, &target).unwrap();
    common::dir_link(&old_dir, &target);
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::with_sandboxes(vec![sandbox("running")]);

    let err = rebuild_demo(&env, &backend).0.unwrap_err();

    assert!(
        format!("{err:#}").contains("is a symlink or junction"),
        "{err:#}"
    );
    assert!(target.join("common").join("spec.yaml").is_file());
}
