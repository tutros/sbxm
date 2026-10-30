//! Issue 330 (decision 270, point 3): `sbxm run` with `antigravity_auth = "login"`: before any
//! write or sandbox, the saved login and one refresh are checked for every Antigravity contestant
//! or judge, and the `google` secret isn't required.

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use common::login::{Endpoint, write_login};
use sbxm::backend::FakeBackend;
use sbxm::commands::run;
use sbxm::run::config::RunConfig;
use sbxm::run::preflight;

fn config(env: &Env, contestants: &[&str], judge: Option<&str>) -> PathBuf {
    let path = env.tmp.path().join("run.toml");
    let mut text = "[task]\nprompt = \"Do it\"\n\n".to_owned();
    for (i, harness) in contestants.iter().enumerate() {
        text.push_str(&format!(
            "[[contestants]]\nharness = \"{harness}\"\nmodel = \"m{i}\"\n\n"
        ));
    }
    if let Some(judge) = judge {
        text.push_str("[[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n");
        text.push_str(&format!(
            "[eval.judge]\nharness = \"{judge}\"\nmodel = \"j\"\n"
        ));
    }
    std::fs::write(&path, text).unwrap();
    path
}

fn login_env() -> Env {
    let env = Env::new();
    env.set_config("antigravity_auth = \"login\"");
    env
}

fn check(env: &Env, path: &Path, backend: &FakeBackend, endpoint: &Endpoint) -> String {
    let config = RunConfig::load(path).unwrap();
    match preflight::check(&env.config_dir(), &config, backend, &endpoint.host()) {
        Ok(_) => String::new(),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn login_mode_needs_no_google_secret_and_refreshes_once() {
    let env = login_env();
    write_login(&env.config_dir());
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let endpoint = Endpoint::fresh("new", 3599);
    let path = config(&env, &["claude", "antigravity"], None);
    assert_eq!(check(&env, &path, &backend, &endpoint), "");
    assert_eq!(endpoint.calls(), 1);
}

#[test]
fn login_mode_covers_an_antigravity_judge() {
    let env = login_env();
    write_login(&env.config_dir());
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let endpoint = Endpoint::fresh("new", 3599);
    let path = config(&env, &["claude", "claude"], Some("antigravity"));
    assert_eq!(check(&env, &path, &backend, &endpoint), "");
    assert_eq!(endpoint.calls(), 1);
}

#[test]
fn login_mode_without_antigravity_never_reads_the_login() {
    let env = login_env();
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let endpoint = Endpoint::fresh("new", 3599);
    let path = config(&env, &["claude", "claude"], None);
    assert_eq!(check(&env, &path, &backend, &endpoint), "");
    assert_eq!(endpoint.calls(), 0);
}

#[test]
fn api_key_mode_still_needs_the_google_secret_and_no_login() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let endpoint = Endpoint::fresh("new", 3599);
    let path = config(&env, &["claude", "antigravity"], None);
    let err = check(&env, &path, &backend, &endpoint);
    assert!(err.contains("'google'"), "{err}");
    assert_eq!(endpoint.calls(), 0);
}

fn refused(env: &Env, endpoint: &Endpoint) -> String {
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let err = run::run_with_auth(
        &env.config_dir(),
        &config(env, &["claude", "antigravity"], None),
        &Default::default(),
        &backend,
        &mut Vec::new(),
        &mut Vec::new(),
        &endpoint.host(),
    )
    .unwrap_err();
    // Nothing written, nothing created.
    assert!(!env.base_dir().join(".sbxm").exists());
    assert_eq!(std::fs::read_dir(env.base_dir()).unwrap().count(), 0);
    assert!(
        !backend.log().iter().any(|l| l.starts_with("create")
            || l.starts_with("secret-set")
            || l.starts_with("validate")),
        "{:?}",
        backend.log()
    );
    let err = format!("{err:#}");
    assert_eq!(err.lines().count(), 1, "{err}");
    err
}

#[test]
fn login_mode_without_a_login_file_is_refused_before_anything_is_written() {
    let env = login_env();
    let endpoint = Endpoint::fresh("new", 3599);
    let err = refused(&env, &endpoint);
    assert!(err.contains("sbxm auth antigravity"), "{err}");
    assert_eq!(endpoint.calls(), 0);
}

#[test]
fn login_mode_with_a_refused_refresh_is_refused_before_anything_is_written() {
    let env = login_env();
    write_login(&env.config_dir());
    let endpoint = Endpoint::answering(400, r#"{"error":"invalid_grant"}"#);
    let err = refused(&env, &endpoint);
    assert!(err.contains("sbxm auth antigravity"), "{err}");
    assert!(!err.contains("invalid_grant"), "{err}");
}
