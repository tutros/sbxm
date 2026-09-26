//! Slice 13: every `secrets.services` entry must be stored in `sbx` before
//! anything is created (decisions 30, 58).

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::open;

const MISSING_GITHUB: &str = "secret 'github' (secrets.services) is not stored in sbx; \
     add it with `sbx secret set github` or import it with `sbx setup`";

fn assert_nothing_created(env: &Env, backend: &FakeBackend) {
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join("demo").exists());
    assert!(
        !env.base_dir()
            .join(".sbxm")
            .join("demo")
            .join("kits")
            .exists()
    );
}

#[test]
fn missing_secret_fails_before_anything_is_created() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[secrets]\nservices = [\"anthropic\", \"github\"]\n",
    );
    let backend = FakeBackend::with_secrets(&["anthropic"]);

    let err = env.run("demo", &backend).unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GITHUB), "{err:#}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn stored_secrets_let_the_sandbox_be_created() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[secrets]\nservices = [\"anthropic\", \"github\"]\n",
    );
    let backend = FakeBackend::with_secrets(&["anthropic", "github"]);

    env.run("demo", &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn secrets_from_the_project_config_are_checked_too() {
    let env = Env::new();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(
        metadata.join("sandbox.toml"),
        "[secrets]\nservices = [\"github\"]\n",
    )
    .unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GITHUB), "{err:#}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn rebuild_with_a_missing_secret_keeps_the_old_sandbox() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    env.write_profile("default", "[secrets]\nservices = [\"github\"]\n");
    let backend = FakeBackend::default().and_sandboxes(vec![SandboxInfo {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        status: "running".into(),
    }]);

    let err = open::run(
        &env.config_dir(),
        "demo",
        &open::Options { rebuild: true },
        &backend,
        &mut std::io::sink(),
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GITHUB), "{err:#}");
    assert!(backend.removes().is_empty());
    assert!(backend.creates().is_empty());
}
