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
    // Decision 76: sbxm has no default_harness setting.
    assert!(config.get("default_harness").is_none());
    assert_eq!(config["min_sbx_version"].as_str(), Some("0.43.0"));
    assert!(config["resources"]["cpus"].as_integer().unwrap() > 0);
    assert!(config["resources"]["memory"].as_str().is_some());

    let profile = read_toml(&config_dir.join("profiles/default/profile.toml"));
    assert!(profile["description"].as_str().is_some());
    assert!(profile["network"]["allow"].as_array().is_some());
}

#[test]
fn second_run_refuses_and_leaves_files_unchanged() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.toml");
    let profile_path = tmp.path().join("profiles/default/profile.toml");
    config_init(tmp.path()).success();
    std::fs::write(&config_path, "# edited config\n").unwrap();
    std::fs::write(&profile_path, "# edited profile\n").unwrap();

    let output = config_init(tmp.path()).failure().get_output().clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already exists"), "stderr: {stderr}");
    assert!(stderr.contains("config.toml"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        "# edited config\n"
    );
    assert_eq!(
        std::fs::read_to_string(&profile_path).unwrap(),
        "# edited profile\n"
    );
}

#[test]
fn existing_profile_alone_blocks_writing_config() {
    let tmp = TempDir::new().unwrap();
    let profile_path = tmp.path().join("profiles/default/profile.toml");
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    std::fs::write(&profile_path, "# mine\n").unwrap();

    let output = config_init(tmp.path()).failure().get_output().clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("profile.toml"), "stderr: {stderr}");
    assert!(!tmp.path().join("config.toml").exists());
    assert_eq!(std::fs::read_to_string(&profile_path).unwrap(), "# mine\n");
}

#[test]
fn reports_the_files_it_wrote() {
    let tmp = TempDir::new().unwrap();

    let output = config_init(tmp.path()).success().get_output().clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let config_path = tmp.path().join("config.toml");
    let profile_path = tmp
        .path()
        .join("profiles")
        .join("default")
        .join("profile.toml");
    assert!(
        stdout.contains(&config_path.display().to_string()),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&profile_path.display().to_string()),
        "stdout: {stdout}"
    );
}
