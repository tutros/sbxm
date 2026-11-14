//! `[resources] sandbox_slots` in `config.toml` (issue 504, decision 304).

use crate::common;

use common::Env;
use sbxm::config::GlobalConfig;

fn with_slots(env: &Env, line: &str) {
    let path = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    // `Env` writes `[resources]` last, so the line lands in that table.
    std::fs::write(&path, format!("{text}{line}\n")).unwrap();
}

#[test]
fn unset_means_no_limit() {
    let env = Env::new();
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.resources.sandbox_slots, None);
}

#[test]
fn a_number_of_slots_is_read() {
    let env = Env::new();
    with_slots(&env, "sandbox_slots = 2");
    let config = GlobalConfig::load(&env.config_dir()).unwrap();
    assert_eq!(config.resources.sandbox_slots, Some(2));
}

#[test]
fn zero_slots_is_a_config_error_naming_the_setting() {
    let env = Env::new();
    with_slots(&env, "sandbox_slots = 0");
    let err = format!("{:#}", GlobalConfig::load(&env.config_dir()).unwrap_err());
    assert!(err.contains("sandbox_slots = 0"), "{err}");
    assert!(err.contains("[resources]"), "{err}");
    assert!(err.contains("config.toml"), "{err}");
}

#[test]
fn the_setting_is_not_part_of_a_sandbox_config_hash() {
    let env = Env::new();
    let before = GlobalConfig::load(&env.config_dir()).unwrap();
    with_slots(&env, "sandbox_slots = 1");
    let after = GlobalConfig::load(&env.config_dir()).unwrap();
    for harness in [
        sbxm::harness::Harness::Claude,
        sbxm::harness::Harness::Codex,
    ] {
        assert_eq!(
            before
                .current_hash("demo", "default", harness, false)
                .unwrap(),
            after
                .current_hash("demo", "default", harness, false)
                .unwrap()
        );
    }
}
