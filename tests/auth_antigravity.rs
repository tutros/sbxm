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
