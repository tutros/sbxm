//! Issue 330 (decision 270): Antigravity sandboxes signed in with the user's Google login through
//! a proxy placeholder: the `antigravity_auth` key, the saved login, the access-token cache and
//! its refresh.

mod common;

use common::Env;
use sbxm::config::{AntigravityAuth, GlobalConfig};

#[test]
fn antigravity_auth_defaults_to_the_api_key() {
    let env = Env::new();
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.antigravity_auth, AntigravityAuth::ApiKey);
}

#[test]
fn antigravity_auth_reads_login_and_api_key() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"login\"");
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.antigravity_auth, AntigravityAuth::Login);

    let env = Env::new();
    env.set_config("antigravity_auth = \"api-key\"");
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.antigravity_auth, AntigravityAuth::ApiKey);
}

#[test]
fn another_antigravity_auth_value_is_refused_naming_the_file() {
    let env = Env::new();
    env.set_config("antigravity_auth = \"oauth\"");
    let err = format!("{:#}", GlobalConfig::load(&env.config_dir()).unwrap_err());
    let path = env.config_dir().join("config.toml");
    assert!(err.contains(&path.display().to_string()), "{err}");
    assert!(err.contains("antigravity_auth"), "{err}");
    assert!(err.contains("login") && err.contains("api-key"), "{err}");
}

mod token {
    use std::path::Path;

    use super::Env;
    use crate::common::login::{Endpoint, NOW, write_cache, write_login};
    use sbxm::auth;

    #[test]
    fn the_paths_are_under_the_config_dirs_auth_folder() {
        let dir = Path::new("cfg");
        assert_eq!(
            auth::login_path(dir),
            dir.join("auth").join("antigravity.json")
        );
        assert_eq!(
            auth::cache_path(dir),
            dir.join("auth").join("antigravity-access.json")
        );
    }

    #[test]
    fn a_cached_token_expiring_in_more_than_5_minutes_is_served_without_a_refresh() {
        let env = Env::new();
        write_login(&env.config_dir());
        write_cache(&env.config_dir(), "cached", NOW + 301);
        let endpoint = Endpoint::fresh("new", 3599);
        let token = auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap();
        assert_eq!(token, "cached");
        assert_eq!(endpoint.calls(), 0);
    }

    #[test]
    fn a_cached_token_expiring_in_exactly_5_minutes_is_refreshed() {
        let env = Env::new();
        write_login(&env.config_dir());
        write_cache(&env.config_dir(), "cached", NOW + 300);
        let endpoint = Endpoint::fresh("new", 3599);
        let token = auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap();
        assert_eq!(token, "new");
        assert_eq!(endpoint.calls(), 1);
    }

    #[test]
    fn a_cached_token_expiring_in_less_than_5_minutes_is_refreshed_and_cached() {
        let env = Env::new();
        write_login(&env.config_dir());
        write_cache(&env.config_dir(), "cached", NOW + 299);
        let endpoint = Endpoint::fresh("new", 3599);
        assert_eq!(
            auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap(),
            "new"
        );
        // The next call, a second later, is served from the new cache.
        assert_eq!(
            auth::access_token(&env.config_dir(), NOW + 1, &endpoint).unwrap(),
            "new"
        );
        assert_eq!(endpoint.calls(), 1);
        let cache = std::fs::read_to_string(auth::cache_path(&env.config_dir())).unwrap();
        assert!(cache.contains(&(NOW + 3599).to_string()), "{cache}");
    }

    #[test]
    fn the_refresh_posts_the_refresh_token_with_the_saved_client() {
        let env = Env::new();
        write_login(&env.config_dir());
        let endpoint = Endpoint::fresh("new", 3599);
        auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap();
        let calls = endpoint.calls.lock().unwrap();
        let (url, form) = &calls[0];
        assert_eq!(url, "https://oauth2.googleapis.com/token");
        let field = |k: &str| form.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str());
        assert_eq!(field("grant_type"), Some("refresh_token"));
        assert_eq!(field("refresh_token"), Some("fake-refresh"));
        assert_eq!(
            field("client_id"),
            Some("fake-id.apps.googleusercontent.com")
        );
        assert_eq!(field("client_secret"), Some("fake-secret"));
    }

    #[test]
    fn no_cache_file_means_a_refresh() {
        let env = Env::new();
        write_login(&env.config_dir());
        let endpoint = Endpoint::fresh("new", 3599);
        assert_eq!(
            auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap(),
            "new"
        );
        assert_eq!(endpoint.calls(), 1);
    }

    #[test]
    fn a_corrupt_cache_file_means_a_refresh_never_a_failure() {
        for corrupt in ["", "{", "not json", r#"{"access_token":"x"}"#, r#"[1,2]"#] {
            let env = Env::new();
            write_login(&env.config_dir());
            std::fs::write(auth::cache_path(&env.config_dir()), corrupt).unwrap();
            let endpoint = Endpoint::fresh("new", 3599);
            assert_eq!(
                auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap(),
                "new",
                "{corrupt:?}"
            );
        }
    }

    #[test]
    fn no_login_file_names_sbxm_auth_antigravity() {
        let env = Env::new();
        let endpoint = Endpoint::fresh("new", 3599);
        let err = format!(
            "{:#}",
            auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap_err()
        );
        assert!(err.contains("sbxm auth antigravity"), "{err}");
        assert!(
            err.contains(&auth::login_path(&env.config_dir()).display().to_string()),
            "{err}"
        );
        assert_eq!(endpoint.calls(), 0);
    }

    #[test]
    fn a_login_file_with_a_missing_field_names_it_and_the_fix() {
        for field in ["client_id", "client_secret", "refresh_token"] {
            let env = Env::new();
            let mut login = serde_json::json!({
                "client_id": "fake-id",
                "client_secret": "fake-secret",
                "refresh_token": "fake-refresh",
            });
            login.as_object_mut().unwrap().remove(field);
            std::fs::create_dir_all(env.config_dir().join("auth")).unwrap();
            std::fs::write(auth::login_path(&env.config_dir()), login.to_string()).unwrap();
            let endpoint = Endpoint::fresh("new", 3599);
            let err = format!(
                "{:#}",
                auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap_err()
            );
            assert!(err.contains(field), "{field}: {err}");
            assert!(err.contains("sbxm auth antigravity"), "{err}");
            assert!(
                !err.contains("fake-secret") && !err.contains("fake-refresh"),
                "{err}"
            );
            assert_eq!(endpoint.calls(), 0);
        }
    }

    #[test]
    fn a_refused_refresh_names_sbxm_auth_antigravity_never_the_body() {
        for status in [400, 401] {
            let env = Env::new();
            write_login(&env.config_dir());
            let endpoint = Endpoint::answering(
                status,
                r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked. fake-refresh"}"#,
            );
            let err = format!(
                "{:#}",
                auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap_err()
            );
            assert!(err.contains("sbxm auth antigravity"), "{err}");
            assert!(err.contains(&status.to_string()), "{err}");
            assert!(!err.contains("invalid_grant"), "{err}");
            assert!(!err.contains("fake-refresh"), "{err}");
            assert!(!auth::cache_path(&env.config_dir()).exists());
        }
    }

    #[test]
    fn a_200_without_a_token_is_an_error_without_the_body() {
        let env = Env::new();
        write_login(&env.config_dir());
        let endpoint = Endpoint::answering(200, r#"{"unexpected":"fake-body"}"#);
        let err = format!(
            "{:#}",
            auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap_err()
        );
        assert!(!err.contains("fake-body"), "{err}");
        assert!(err.contains("sbxm auth antigravity"), "{err}");
    }

    #[test]
    fn refresh_always_asks_the_endpoint_even_with_a_good_cache() {
        let env = Env::new();
        write_login(&env.config_dir());
        write_cache(&env.config_dir(), "cached", NOW + 3000);
        let endpoint = Endpoint::fresh("new", 3599);
        auth::refresh(&env.config_dir(), NOW, &endpoint).unwrap();
        assert_eq!(endpoint.calls(), 1);
        assert_eq!(
            auth::access_token(&env.config_dir(), NOW, &endpoint).unwrap(),
            "new"
        );
    }

    #[test]
    fn a_config_dir_inside_the_base_dir_is_refused() {
        let env = Env::new();
        let inside = env.base_dir().join("cfg");
        std::fs::create_dir_all(&inside).unwrap();
        let err = format!(
            "{:#}",
            auth::check_config_dir(&inside, &env.base_dir()).unwrap_err()
        );
        assert!(err.contains(&inside.display().to_string()), "{err}");
        assert!(
            auth::check_config_dir(&env.base_dir(), &env.base_dir()).is_err(),
            "the base dir itself"
        );
        auth::check_config_dir(&env.config_dir(), &env.base_dir()).unwrap();
        // A sibling whose name only starts with the base dir's is outside it.
        let sibling = env.tmp.path().join("base-config");
        std::fs::create_dir_all(&sibling).unwrap();
        auth::check_config_dir(&sibling, &env.base_dir()).unwrap();
    }
}

mod token_command {
    use assert_cmd::Command;

    use super::Env;
    use crate::common::login::{Endpoint, NOW, write_cache, write_login};
    use sbxm::commands::auth_token;

    #[test]
    fn prints_only_the_access_token() {
        let env = Env::new();
        write_login(&env.config_dir());
        write_cache(&env.config_dir(), "cached-token", NOW + 3000);
        let mut out = Vec::new();
        auth_token::run(
            &env.config_dir(),
            NOW,
            &Endpoint::fresh("new", 3599),
            &mut out,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "cached-token");
    }

    #[test]
    fn prints_nothing_when_the_refresh_fails() {
        let env = Env::new();
        write_login(&env.config_dir());
        let mut out = Vec::new();
        let err = auth_token::run(
            &env.config_dir(),
            NOW,
            &Endpoint::answering(401, "{}"),
            &mut out,
        )
        .unwrap_err();
        assert!(out.is_empty());
        assert!(format!("{err:#}").contains("sbxm auth antigravity"));
    }

    #[test]
    fn the_cli_has_auth_token_antigravity() {
        let env = Env::new();
        let assert = Command::cargo_bin("sbxm")
            .unwrap()
            .env("SBXM_CONFIG_DIR", env.config_dir())
            .args(["auth", "token", "antigravity"])
            .assert()
            .failure();
        let output = assert.get_output();
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("sbxm auth antigravity"), "{stderr}");
    }
}

mod sandbox {
    use std::path::Path;

    use sbxm::auth;
    use sbxm::backend::{FakeBackend, SandboxBackend, SecretSource, Stdin};

    #[test]
    fn the_secret_is_scoped_to_the_sandbox_on_both_google_hosts_with_sbxms_helper() {
        let backend = FakeBackend::default();
        auth::set_sandbox_secret(&backend, "sbxm-run-x-0-0", Path::new("/opt/sbxm/sbxm")).unwrap();
        let sets = backend.custom_secret_sets();
        assert_eq!(sets.len(), 1);
        let spec = &sets[0];
        assert_eq!(spec.sandbox, "sbxm-run-x-0-0");
        assert_eq!(
            spec.hosts,
            ["daily-cloudcode-pa.googleapis.com", "www.googleapis.com"]
        );
        assert_eq!(spec.placeholder, auth::placeholder("sbxm-run-x-0-0"));
        assert_eq!(
            spec.source,
            SecretSource::CommandLine("/opt/sbxm/sbxm auth token antigravity".into())
        );
    }

    #[test]
    fn the_token_file_holds_only_placeholders_and_a_far_future_expiry() {
        let backend = FakeBackend::default();
        auth::write_token_file(&backend, "sbxm-run-x-0-0").unwrap();
        let execs = backend.execs();
        assert_eq!(execs.len(), 1);
        let (sandbox, spec) = &execs[0];
        assert_eq!(sandbox, "sbxm-run-x-0-0");
        let script = spec.argv.join(" ");
        assert!(
            script.contains(".gemini/antigravity-cli/antigravity-oauth-token"),
            "{script}"
        );
        let Stdin::Piped(json) = &spec.stdin else {
            panic!("the file goes through stdin, never the command line")
        };
        let file: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(file["access_token"], auth::placeholder("sbxm-run-x-0-0"));
        assert_ne!(file["refresh_token"], file["access_token"]);
        assert!(
            file["refresh_token"].as_str().unwrap().starts_with("sbxm-"),
            "{file}"
        );
        assert_eq!(file["auth_method"], "consumer");
        assert_eq!(file["expiry"], "2099-01-01T00:00:00Z");
    }

    #[test]
    fn a_token_file_that_cant_be_written_is_an_error() {
        let backend = FakeBackend::default().with_default_exec_output(sbxm::backend::ExecOutput {
            stdout: String::new(),
            stderr: "disk full".into(),
            exit_code: Some(1),
        });
        let err = auth::write_token_file(&backend, "s").unwrap_err();
        assert!(
            format!("{err:#}").contains("antigravity-oauth-token"),
            "{err:#}"
        );
    }

    #[test]
    fn the_secret_is_removed_by_its_placeholder() {
        let backend = FakeBackend::default();
        auth::set_sandbox_secret(&backend, "s", Path::new("/opt/sbxm")).unwrap();
        auth::remove_sandbox_secret(&backend, "s").unwrap();
        assert!(backend.custom_secrets().unwrap().is_empty());
        assert_eq!(
            backend.log().last().unwrap(),
            &format!("secret-rm s {}", auth::placeholder("s"))
        );
    }
}
