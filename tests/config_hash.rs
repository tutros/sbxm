//! Slice 11: every sandbox records the profile and config hash it was built
//! from (decisions 28, 55).

mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::new;

/// Runs `new` for `project` with `profile` and returns its state entry.
fn created(env: &Env, project: &str, profile: Option<&str>) -> serde_json::Value {
    let options = new::Options {
        profile: profile.map(String::from),
        ..new::Options::default()
    };
    env.run_with(project, &options, &FakeBackend::default())
        .unwrap();
    let path = env
        .base_dir()
        .join(".sbxm")
        .join(project)
        .join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    state["sandboxes"]["claude"].clone()
}

fn hash(env: &Env, project: &str) -> String {
    created(env, project, None)["config_hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn state_records_profile_and_full_sha256_hash() {
    let env = Env::new();

    let entry = created(&env, "demo", None);

    assert_eq!(entry["profile"], "default");
    let hash = entry["config_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64, "{hash}");
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "{hash}"
    );
}

#[test]
fn same_config_gives_the_same_hash() {
    let env = Env::new();

    assert_eq!(hash(&env, "one"), hash(&env, "two"));
}

#[test]
fn network_change_changes_the_hash() {
    let env = Env::new();
    let before = hash(&env, "one");
    env.write_profile("default", "[network]\nallow = [\"github.com\"]\n");

    assert_ne!(before, hash(&env, "two"));
}

#[test]
fn description_change_keeps_the_hash() {
    let env = Env::new();
    let before = hash(&env, "one");
    env.write_profile("default", "description = \"reworded\"\n");

    assert_eq!(before, hash(&env, "two"));
}

#[test]
fn resources_change_changes_the_hash() {
    let env = Env::new();
    let before = hash(&env, "one");
    let config = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replace("cpus = 4", "cpus = 2")).unwrap();

    assert_ne!(before, hash(&env, "two"));
}

#[test]
fn profile_name_is_part_of_the_hash() {
    let env = Env::new();
    env.write_profile("other", "description = \"test default\"\n");

    let default = created(&env, "one", None);
    let other = created(&env, "two", Some("other"));

    assert_eq!(other["profile"], "other");
    assert_ne!(default["config_hash"], other["config_hash"]);
}

#[test]
fn kit_dir_is_named_after_the_hash_prefix() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(env.base_dir().join(".sbxm/demo/state.json")).unwrap(),
    )
    .unwrap();
    let hash = state["sandboxes"]["claude"]["config_hash"]
        .as_str()
        .unwrap();
    let expected = env
        .base_dir()
        .join(".sbxm")
        .join("demo")
        .join("kits")
        .join(&hash[..12])
        .join("common");
    assert!(expected.join("spec.yaml").is_file());
    assert_eq!(backend.creates()[0].kits, vec![expected]);
}
