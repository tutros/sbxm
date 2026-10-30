//! Issue 330 (decision 270, point 5): `sbxm doctor` in login mode checks the saved Antigravity
//! login and one refresh, with no sandbox.

mod common;

use common::Env;
use common::login::{Endpoint, write_login};
use sbxm::auth::TokenEndpoint;
use sbxm::backend::FakeBackend;
use sbxm::commands::doctor;

fn host(endpoint: &'static (dyn TokenEndpoint + Sync)) -> doctor::Host {
    doctor::Host {
        free_space: |_| Ok(u64::MAX),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
        sandbox_state_dir: None,
        now: || common::login::NOW,
        current_dir: None,
        token_endpoint: endpoint,
    }
}

fn leak(endpoint: Endpoint) -> &'static Endpoint {
    Box::leak(Box::new(endpoint))
}

fn login_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains("Antigravity")).collect()
}

#[test]
fn a_login_that_refreshes_passes_without_a_sandbox() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"login\"");
    write_login(&env.config_dir());
    let endpoint = leak(Endpoint::fresh("new", 3599));
    let backend = FakeBackend::default();
    let report = doctor::run(&env.config_dir(), &backend, &host(endpoint));
    let text = report.render();
    assert_eq!(
        login_lines(&text),
        [format!(
            "ok   Antigravity login {} refreshes",
            sbxm::auth::login_path(&env.config_dir()).display()
        )],
        "{text}"
    );
    assert_eq!(endpoint.calls(), 1);
    assert!(backend.creates().is_empty());
    assert!(
        !sbxm::auth::cache_path(&env.config_dir()).exists(),
        "doctor changes nothing"
    );
    assert!(!text.contains("new"), "never the token: {text}");
}

#[test]
fn a_missing_login_fails_naming_the_sign_in() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"login\"");
    let endpoint = leak(Endpoint::fresh("new", 3599));
    let report = doctor::run(&env.config_dir(), &FakeBackend::default(), &host(endpoint));
    assert!(report.failed());
    let text = report.render();
    let lines = login_lines(&text);
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].starts_with("FAIL "), "{text}");
    assert!(lines[0].contains("sbxm auth antigravity"), "{text}");
    assert_eq!(endpoint.calls(), 0);
}

#[test]
fn a_refused_refresh_fails_naming_the_sign_in_never_the_body() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"login\"");
    write_login(&env.config_dir());
    let endpoint = leak(Endpoint::answering(400, r#"{"error":"invalid_grant"}"#));
    let report = doctor::run(&env.config_dir(), &FakeBackend::default(), &host(endpoint));
    assert!(report.failed());
    let text = report.render();
    let lines = login_lines(&text);
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].contains("HTTP 400"), "{text}");
    assert!(lines[0].contains("sbxm auth antigravity"), "{text}");
    assert!(!text.contains("invalid_grant"), "{text}");
}

#[test]
fn a_config_dir_inside_the_base_dir_fails_in_login_mode() {
    let env = Env::new();
    let inside = env.base_dir().join("cfg");
    std::fs::create_dir_all(&inside).unwrap();
    let text = std::fs::read_to_string(env.config_dir().join("config.toml")).unwrap();
    std::fs::write(
        inside.join("config.toml"),
        format!("antigravity_auth = \"login\"\n{text}"),
    )
    .unwrap();
    write_login(&inside);
    let endpoint = leak(Endpoint::fresh("new", 3599));
    let report = doctor::run(&inside, &FakeBackend::default(), &host(endpoint));
    assert!(report.failed());
    let text = report.render();
    assert!(
        login_lines(&text)
            .iter()
            .any(|l| l.starts_with("FAIL ") && l.contains("SBXM_CONFIG_DIR")),
        "{text}"
    );
}

#[test]
fn api_key_mode_checks_no_login() {
    let env = Env::new();
    let endpoint = leak(Endpoint::fresh("new", 3599));
    let report = doctor::run(&env.config_dir(), &FakeBackend::default(), &host(endpoint));
    assert!(
        login_lines(&report.render()).is_empty(),
        "{}",
        report.render()
    );
    assert_eq!(endpoint.calls(), 0);
}
