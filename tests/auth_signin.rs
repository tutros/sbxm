//! `sbxm auth antigravity` (issue 330, decision 270, point 1): sign in inside a throwaway
//! Antigravity sandbox, copy the login out with `sbx exec`, find `agy`'s client pair and save
//! both to `<config dir>/auth/antigravity.json`, removing the sandbox on every path.

mod common;

use common::Env;
use common::login::{NOW, PairEndpoint, UnusableEndpoint};
use common::task_fixture::ok;
use sbxm::auth;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::auth_antigravity::{self, SANDBOX};
use sbxm::disk::Guard;

const TOKEN_FILE: &str = r#"{"access_token":"ya29.fake-access","token_type":"Bearer","refresh_token":"fake-user-refresh","expiry":"2026-10-30T12:00:00Z","auth_method":"consumer"}"#;
const CLIENTS: &str = "111-fakeaaa.apps.googleusercontent.com\n222-fakebbb.apps.googleusercontent.com\n\
                       GOCSPX-FAKE-one\nGOCSPX-FAKE-two\nGOCSPX-FAKE-three\n";

fn signed_in() -> FakeBackend {
    FakeBackend::default()
        .with_exec_output_matching("antigravity-oauth-token", ok(TOKEN_FILE))
        .with_exec_output_matching("GOCSPX", ok(CLIENTS))
}

fn good() -> PairEndpoint {
    PairEndpoint::new(
        "111-fakeaaa.apps.googleusercontent.com",
        "GOCSPX-FAKE-two",
        "fake-fresh-access",
    )
}

fn run(env: &Env, backend: &FakeBackend, endpoint: &PairEndpoint) -> anyhow::Result<String> {
    let mut out = Vec::new();
    auth_antigravity::run(
        &env.config_dir(),
        backend,
        &endpoint.host(),
        &Guard::default(),
        &mut out,
    )?;
    Ok(String::from_utf8(out).unwrap())
}

fn workspace(env: &Env) -> std::path::PathBuf {
    env.base_dir().join(".sbxm").join(".auth-antigravity")
}

#[test]
fn signing_in_saves_the_login_with_the_client_pair_that_refreshes() {
    let env = Env::new();
    let backend = signed_in();
    let endpoint = good();
    let out = run(&env, &backend, &endpoint).unwrap();

    let login = auth::load_login(&env.config_dir()).unwrap();
    assert_eq!(login.client_id, "111-fakeaaa.apps.googleusercontent.com");
    assert_eq!(login.client_secret, "GOCSPX-FAKE-two");
    assert_eq!(login.refresh_token, "fake-user-refresh");
    // The access token the right pair got is cached, so the next call needs no refresh.
    let token = auth::access_token(&env.config_dir(), NOW, &UnusableEndpoint).unwrap();
    assert_eq!(token, "fake-fresh-access");
    // Nothing secret reaches the screen.
    for secret in ["fake-user-refresh", "GOCSPX", "fake-fresh-access", "ya29"] {
        assert!(!out.contains(secret), "{out}");
    }
    assert!(out.contains(&auth::login_path(&env.config_dir()).display().to_string()));
}

#[test]
fn a_cache_that_cant_be_written_saves_no_login_and_claims_nothing() {
    let env = Env::new();
    let cache = auth::cache_path(&env.config_dir());
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("keep"), "").unwrap();
    let backend = signed_in();
    let mut out = Vec::new();
    let err = auth_antigravity::run(
        &env.config_dir(),
        &backend,
        &good().host(),
        &Guard::default(),
        &mut out,
    )
    .unwrap_err();
    let err = format!("{err:#}");
    assert!(err.contains(&cache.display().to_string()), "{err}");
    assert!(!auth::login_path(&env.config_dir()).exists());
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("Saved"), "{out}");
}

#[test]
fn the_sandbox_is_made_from_the_pinned_kit_attached_to_then_removed_with_its_workspace() {
    let env = Env::new();
    let backend = signed_in();
    run(&env, &backend, &good()).unwrap();

    let log = backend.log();
    let create = log.iter().position(|l| l == &format!("create {SANDBOX}"));
    let attach = log.iter().position(|l| l == &format!("attach {SANDBOX}"));
    let exec = log.iter().position(|l| l == &format!("exec {SANDBOX}"));
    let rm = log.iter().rposition(|l| l == &format!("rm {SANDBOX}"));
    assert!(
        create < attach && attach < exec && exec < rm && create.is_some(),
        "{log:#?}"
    );
    let spec = &backend.creates()[0];
    assert_eq!(
        spec.agent,
        sbxm::harness::Harness::Antigravity.agent_arg(),
        "the pinned kit"
    );
    assert_eq!(spec.workspace.as_deref(), Some(workspace(&env).as_path()));
    assert!(!workspace(&env).exists());
}

#[test]
fn the_sandbox_name_cannot_be_a_projects() {
    // A project sandbox is `sbxm-<project>-<harness>` and a project name has no dot.
    assert!(SANDBOX.contains('.'), "{SANDBOX}");
}

#[test]
fn a_pair_that_answers_only_the_last_try_is_still_found() {
    let env = Env::new();
    let backend = signed_in();
    let endpoint = PairEndpoint::new(
        "222-fakebbb.apps.googleusercontent.com",
        "GOCSPX-FAKE-three",
        "fake-fresh-access",
    );
    run(&env, &backend, &endpoint).unwrap();
    let login = auth::load_login(&env.config_dir()).unwrap();
    assert_eq!(login.client_id, "222-fakebbb.apps.googleusercontent.com");
    assert_eq!(login.client_secret, "GOCSPX-FAKE-three");
    assert_eq!(endpoint.calls.lock().unwrap().len(), 6);
}

#[test]
fn no_pair_that_refreshes_saves_nothing_and_removes_the_sandbox() {
    let env = Env::new();
    let backend = signed_in();
    let endpoint = PairEndpoint::new("nobody", "nothing", "x");
    let err = format!("{:#}", run(&env, &backend, &endpoint).unwrap_err());
    assert!(err.contains("client"), "{err}");
    assert!(!err.contains("invalid_client"), "never the body: {err}");
    assert!(!auth::login_path(&env.config_dir()).exists());
    assert_eq!(backend.removes().last().map(String::as_str), Some(SANDBOX));
    assert!(!workspace(&env).exists());
}

#[test]
fn no_client_in_agy_saves_nothing_and_removes_the_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default()
        .with_exec_output_matching("antigravity-oauth-token", ok(TOKEN_FILE))
        .with_exec_output_matching("GOCSPX", ok("111-fakeaaa.apps.googleusercontent.com\n"));
    let err = format!("{:#}", run(&env, &backend, &good()).unwrap_err());
    assert!(err.contains("agy"), "{err}");
    assert!(!auth::login_path(&env.config_dir()).exists());
    assert_eq!(backend.removes().last().map(String::as_str), Some(SANDBOX));
}

#[test]
fn a_sign_in_that_left_no_token_file_says_so_and_removes_the_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default()
        .with_exec_output_matching(
            "antigravity-oauth-token",
            ExecOutput {
                stdout: String::new(),
                stderr: "cat: No such file or directory".into(),
                exit_code: Some(1),
            },
        )
        .with_exec_output_matching("GOCSPX", ok(CLIENTS));
    let endpoint = good();
    let err = format!("{:#}", run(&env, &backend, &endpoint).unwrap_err());
    assert!(err.contains("sign"), "{err}");
    assert_eq!(endpoint.calls.lock().unwrap().len(), 0);
    assert!(!auth::login_path(&env.config_dir()).exists());
    assert_eq!(backend.removes().last().map(String::as_str), Some(SANDBOX));
}

#[test]
fn a_token_file_without_a_refresh_token_names_the_field_never_the_file() {
    let env = Env::new();
    let backend = FakeBackend::default()
        .with_exec_output_matching(
            "antigravity-oauth-token",
            ok(r#"{"access_token":"ya29.fake-access"}"#),
        )
        .with_exec_output_matching("GOCSPX", ok(CLIENTS));
    let err = format!("{:#}", run(&env, &backend, &good()).unwrap_err());
    assert!(err.contains("refresh_token"), "{err}");
    assert!(!err.contains("ya29"), "{err}");
    assert!(!auth::login_path(&env.config_dir()).exists());
    assert_eq!(backend.removes().last().map(String::as_str), Some(SANDBOX));
}

#[test]
fn a_failed_create_still_removes_the_sandbox_and_its_workspace() {
    let env = Env::new();
    let backend = FakeBackend::failing_create();
    run(&env, &backend, &good()).unwrap_err();
    assert_eq!(backend.removes().last().map(String::as_str), Some(SANDBOX));
    assert!(!workspace(&env).exists());
}

#[test]
fn a_config_dir_inside_the_base_dir_is_refused_before_any_sandbox() {
    let env = Env::new();
    let inside = env.base_dir().join("cfg");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::copy(
        env.config_dir().join("config.toml"),
        inside.join("config.toml"),
    )
    .unwrap();
    let backend = signed_in();
    let mut out = Vec::new();
    let err = auth_antigravity::run(
        &inside,
        &backend,
        &good().host(),
        &Guard::default(),
        &mut out,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("SBXM_CONFIG_DIR"), "{err:#}");
    assert!(backend.log().is_empty(), "{:#?}", backend.log());
}

#[test]
fn a_host_that_cannot_reach_google_is_refused_before_any_sandbox() {
    let env = Env::new();
    let backend = signed_in();
    let host = auth::Host {
        endpoint: &UnusableEndpoint,
        helper: common::login::helper_path(),
        now: NOW,
    };
    let err = auth_antigravity::run(
        &env.config_dir(),
        &backend,
        &host,
        &Guard::default(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("no HTTP client here"),
        "{err:#}"
    );
    assert!(backend.log().is_empty(), "{:#?}", backend.log());
}

#[test]
fn too_little_free_disk_is_refused_before_any_sandbox() {
    let env = Env::new();
    let state = env.tmp.path().join("sbx-state");
    std::fs::create_dir_all(&state).unwrap();
    env.set_config(&format!(
        "sandbox_state_dir = {}\nmin_free_disk = \"10g\"",
        toml::Value::String(state.to_str().unwrap().to_owned())
    ));
    let backend = signed_in();
    let guard = Guard {
        free_space: |_| Ok(1),
        ..Guard::default()
    };
    let err = auth_antigravity::run(
        &env.config_dir(),
        &backend,
        &good().host(),
        &guard,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("--ignore-disk"), "{err:#}");
    assert!(backend.log().is_empty(), "{:#?}", backend.log());
}

#[test]
fn a_sandbox_left_by_a_killed_sign_in_is_removed_first() {
    let env = Env::new();
    std::fs::create_dir_all(workspace(&env).join("junk")).unwrap();
    let backend = signed_in().and_sandboxes(vec![sbxm::backend::SandboxInfo {
        name: SANDBOX.into(),
        agent: "antigravity".into(),
        status: "stopped".into(),
    }]);
    run(&env, &backend, &good()).unwrap();
    let log = backend.log();
    assert_eq!(
        log.iter()
            .filter(|l| *l == &format!("rm {SANDBOX}"))
            .count(),
        2
    );
    let first_rm = log.iter().position(|l| l == &format!("rm {SANDBOX}"));
    let create = log.iter().position(|l| l == &format!("create {SANDBOX}"));
    assert!(first_rm < create, "{log:#?}");
}
