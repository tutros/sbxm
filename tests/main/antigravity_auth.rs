//! Issue 384, decision 281 (a): `antigravity_auth` in the global config decides whether an
//! interactive Antigravity sandbox (`new`, `open`, `open --rebuild`) needs the `google` secret.
//! `"api-key"` (the default) requires it; `"login"` requires none, and the user signs in inside
//! the sandbox. Headless runs keep requiring it in both modes (`run_config.rs`,
//! `task_review_cmd.rs`, `task_start_cmd.rs`).

use crate::common;

use common::Env;
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::{new, open};
use sbxm::config::{AntigravityAuth, GlobalConfig};
use sbxm::harness::Harness;

const MISSING_GOOGLE: &str = "secret 'google' is not stored in sbx";

fn antigravity() -> new::Options {
    new::Options {
        harness: Harness::Antigravity,
        ..Default::default()
    }
}

fn login(env: &Env) {
    env.set_config("antigravity_auth = \"login\"");
}

fn open_with(env: &Env, rebuild: bool, backend: &FakeBackend) -> anyhow::Result<()> {
    open::run(
        &env.config_dir(),
        "demo",
        &open::Options {
            rebuild,
            harness: Harness::Antigravity,
            ..Default::default()
        },
        backend,
        &mut std::io::sink(),
    )
}

fn existing_sandbox() -> Vec<SandboxInfo> {
    vec![SandboxInfo {
        name: "sbxm-demo-antigravity".into(),
        agent: "antigravity".into(),
        status: "running".into(),
    }]
}

fn attached(backend: &FakeBackend) -> bool {
    backend
        .log()
        .contains(&"attach sbxm-demo-antigravity".to_owned())
}

fn assert_nothing_created(env: &Env, backend: &FakeBackend) {
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm").join("demo").exists());
}

#[test]
fn the_default_is_api_key() {
    let env = Env::new();

    let config = GlobalConfig::load(&env.config_dir()).unwrap();

    assert_eq!(config.antigravity_auth, AntigravityAuth::ApiKey);
}

#[test]
fn both_values_load() {
    for (text, value) in [
        ("login", AntigravityAuth::Login),
        ("api-key", AntigravityAuth::ApiKey),
    ] {
        let env = Env::new();
        env.set_config(&format!("antigravity_auth = \"{text}\""));

        let config = GlobalConfig::load(&env.config_dir()).unwrap();

        assert_eq!(config.antigravity_auth, value);
    }
}

#[test]
fn any_other_value_is_refused_and_creates_nothing() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"oauth\"");
    let backend = FakeBackend::with_secrets(&["google"]);

    let err = env.run_with("demo", &antigravity(), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("antigravity_auth"), "{message}");
    assert!(
        message.contains("login") && message.contains("api-key"),
        "{message}"
    );
    assert_nothing_created(&env, &backend);
}

/// The key is account-level: switching it never shows as config drift on an existing sandbox.
#[test]
fn the_mode_is_not_in_the_config_hash() {
    let env = Env::new();
    env.run_with(
        "demo",
        &antigravity(),
        &FakeBackend::with_secrets(&["google"]),
    )
    .unwrap();
    login(&env);
    let backend = FakeBackend::default().and_sandboxes(existing_sandbox());

    open_with(&env, false, &backend).unwrap();

    assert!(attached(&backend), "{:?}", backend.log());
}

// login × interactive: no `google` secret required.

#[test]
fn login_new_needs_no_google_secret() {
    let env = Env::new();
    login(&env);
    let backend = FakeBackend::default();

    env.run_with("demo", &antigravity(), &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn login_open_creates_without_a_google_secret_and_attaches() {
    let env = Env::new();
    login(&env);
    let backend = FakeBackend::default();

    open_with(&env, false, &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
    assert!(attached(&backend), "{:?}", backend.log());
}

#[test]
fn login_open_rebuild_needs_no_google_secret() {
    let env = Env::new();
    login(&env);
    env.run_with("demo", &antigravity(), &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::default().and_sandboxes(existing_sandbox());

    open_with(&env, true, &backend).unwrap();

    assert_eq!(backend.removes(), vec!["sbxm-demo-antigravity".to_owned()]);
    assert_eq!(backend.creates().len(), 1);
}

/// Login mode only drops the harness's own secret: one the profile names is still checked.
#[test]
fn login_still_checks_the_profiles_own_secrets() {
    let env = Env::new();
    login(&env);
    env.write_profile("default", "[secrets]\nservices = [\"github\"]\n");
    let backend = FakeBackend::default();

    let err = env.run_with("demo", &antigravity(), &backend).unwrap_err();

    assert!(format!("{err:#}").contains("secret 'github'"), "{err:#}");
    assert_nothing_created(&env, &backend);
}

// login × interactive with a global `google` secret: refused (issue 384 review M-1, the user's
// option (a), 2026-11-03). The pinned Antigravity kit makes `sbx` inject that secret into every
// sandbox, so login mode would hand the stored key to the sandbox anyway; it is refused instead,
// before anything is created or attached.

const GOOGLE_STORED: &str = "secret 'google' is stored globally in sbx";

fn assert_google_stored_refusal(err: &anyhow::Error) {
    let message = format!("{err:#}");
    assert!(message.contains(GOOGLE_STORED), "{message}");
    assert!(message.contains("sbx secret rm google"), "{message}");
    assert!(
        message.contains("antigravity_auth = \"api-key\""),
        "{message}"
    );
}

#[test]
fn login_new_with_a_global_google_secret_is_refused_before_anything_is_created() {
    let env = Env::new();
    login(&env);
    let backend = FakeBackend::with_secrets(&["google"]);

    let err = env.run_with("demo", &antigravity(), &backend).unwrap_err();

    assert_google_stored_refusal(&err);
    assert_nothing_created(&env, &backend);
}

#[test]
fn login_open_with_a_global_google_secret_creates_and_attaches_nothing() {
    let env = Env::new();
    login(&env);
    let backend = FakeBackend::with_secrets(&["google"]);

    let err = open_with(&env, false, &backend).unwrap_err();

    assert_google_stored_refusal(&err);
    assert!(backend.creates().is_empty() && !attached(&backend));
    assert!(!env.base_dir().join("demo").exists());
}

/// A sandbox made while no `google` secret was stored: storing one later must not reach it on
/// the next `open`, which restarts and attaches to it.
#[test]
fn login_open_of_an_existing_sandbox_with_a_global_google_secret_does_not_attach() {
    let env = Env::new();
    login(&env);
    env.run_with("demo", &antigravity(), &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::with_secrets(&["google"]).and_sandboxes(existing_sandbox());

    let err = open_with(&env, false, &backend).unwrap_err();

    assert_google_stored_refusal(&err);
    assert!(!attached(&backend), "{:?}", backend.log());
}

#[test]
fn login_open_of_an_existing_sandbox_without_a_google_secret_attaches() {
    let env = Env::new();
    login(&env);
    env.run_with("demo", &antigravity(), &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::default().and_sandboxes(existing_sandbox());

    open_with(&env, false, &backend).unwrap();

    assert!(attached(&backend), "{:?}", backend.log());
}

#[test]
fn login_open_rebuild_with_a_global_google_secret_keeps_the_old_sandbox() {
    let env = Env::new();
    login(&env);
    env.run_with("demo", &antigravity(), &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::with_secrets(&["google"]).and_sandboxes(existing_sandbox());

    let mut warn = Vec::new();
    let err = open::run(
        &env.config_dir(),
        "demo",
        &open::Options {
            rebuild: true,
            harness: Harness::Antigravity,
            ..Default::default()
        },
        &backend,
        &mut warn,
    )
    .unwrap_err();

    assert_google_stored_refusal(&err);
    assert!(backend.removes().is_empty() && backend.creates().is_empty());
    // Refused before it says the session history will be lost (PR 408 narrow review, nit).
    assert!(warn.is_empty(), "{}", String::from_utf8_lossy(&warn));
}

/// An existing `api-key` sandbox: once `google` is removed, `agy` has no key, so `open` refuses
/// to attach and names the fix (PR 408 narrow review, should-fix).
#[test]
fn api_key_open_of_an_existing_sandbox_without_the_google_secret_does_not_attach() {
    let env = Env::new();
    env.run_with(
        "demo",
        &antigravity(),
        &FakeBackend::with_secrets(&["google"]),
    )
    .unwrap();
    let backend = FakeBackend::default().and_sandboxes(existing_sandbox());

    let err = open_with(&env, false, &backend).unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GOOGLE), "{err:#}");
    assert!(!attached(&backend), "{:?}", backend.log());
}

// api-key × interactive: the `google` secret is required, before anything is written.

#[test]
fn api_key_new_without_the_google_secret_is_refused_before_anything_is_created() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]);

    let err = env.run_with("demo", &antigravity(), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains(MISSING_GOOGLE), "{message}");
    assert!(message.contains("sbx secret set -g google"), "{message}");
    assert!(
        message.contains("antigravity_auth = \"login\""),
        "{message}"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn api_key_new_with_the_google_secret_creates() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["google"]);

    env.run_with("demo", &antigravity(), &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn api_key_open_without_the_google_secret_is_refused_and_attaches_nothing() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"api-key\"");
    let backend = FakeBackend::default();

    let err = open_with(&env, false, &backend).unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GOOGLE), "{err:#}");
    assert!(backend.creates().is_empty() && !attached(&backend));
    assert!(!env.base_dir().join("demo").exists());
}

#[test]
fn api_key_open_rebuild_without_the_google_secret_keeps_the_old_sandbox() {
    let env = Env::new();
    env.run_with(
        "demo",
        &antigravity(),
        &FakeBackend::with_secrets(&["google"]),
    )
    .unwrap();
    let backend = FakeBackend::default().and_sandboxes(existing_sandbox());

    let err = open_with(&env, true, &backend).unwrap_err();

    assert!(format!("{err:#}").contains(MISSING_GOOGLE), "{err:#}");
    assert!(backend.removes().is_empty() && backend.creates().is_empty());
}

/// Other harnesses sign in inside their sandbox in either mode: no provider secret is required.
#[test]
fn api_key_mode_leaves_other_harnesses_alone() {
    for harness in [Harness::Claude, Harness::Codex, Harness::Pi] {
        let env = Env::new();
        let backend = FakeBackend::default();

        env.run_with(
            "demo",
            &new::Options {
                harness,
                ..Default::default()
            },
            &backend,
        )
        .unwrap();

        assert_eq!(backend.creates().len(), 1, "{harness:?}");
    }
}
