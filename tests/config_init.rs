use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn config_init(config_dir: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", config_dir)
        .args(["config", "init"])
        .assert()
}

fn read_toml(path: &Path) -> toml::Table {
    std::fs::read_to_string(path).unwrap().parse().unwrap()
}

#[test]
fn writes_starter_global_config_and_default_profile() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("nested").join("sbxm");

    config_init(&config_dir).success();

    let config = read_toml(&config_dir.join("config.toml"));
    assert!(
        config["base_dir"]
            .as_str()
            .unwrap()
            .ends_with("sbxm-projects")
    );
    assert_eq!(
        Path::new(config["profiles_dir"].as_str().unwrap()),
        config_dir.join("profiles")
    );
    assert_eq!(config["default_profile"].as_str(), Some("default"));
    assert_eq!(config["default_harness"].as_str(), Some("claude"));
    assert_eq!(config["min_sbx_version"].as_str(), Some("0.43.0"));
    assert!(config["resources"]["cpus"].as_integer().unwrap() > 0);
    assert!(config["resources"]["memory"].as_str().is_some());

    let profile = read_toml(&config_dir.join("profiles/default/profile.toml"));
    assert!(profile["description"].as_str().is_some());
    assert!(profile["network"]["allow"].as_array().is_some());
}
