//! Slice 12: `.sbxm/<project>/sandbox.toml` is merged over the profile
//! (decision 57).

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SkillsStore};

/// Writes `.sbxm/demo/sandbox.toml`.
fn write_project_config(env: &Env, contents: &str) {
    let dir = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sandbox.toml"), contents).unwrap();
}

/// The generated `spec.yaml` of `demo`, parsed.
fn spec(env: &Env) -> serde_json::Value {
    let text = std::fs::read_to_string(env.kit_dir("demo").join("spec.yaml")).unwrap();
    serde_norway::from_str(&text).unwrap()
}

#[test]
fn project_network_lists_are_appended_to_the_profile() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[network]\nallow = [\"github.com\"]\ndeny = [\"a.example.com\"]\n",
    );
    write_project_config(
        &env,
        "[network]\nallow = [\"example.org\"]\ndeny = [\"b.example.com\"]\n",
    );

    env.run("demo", &FakeBackend::default()).unwrap();

    let network = &spec(&env)["permissions"]["network"];
    assert_eq!(
        network["allow"],
        serde_json::json!(["github.com", "example.org"])
    );
    assert_eq!(
        network["deny"],
        serde_json::json!(["a.example.com", "b.example.com"])
    );
}

#[test]
fn project_env_is_merged_per_key_with_the_project_winning() {
    let env = Env::new();
    env.write_profile("default", "[env]\nA = \"1\"\nB = \"2\"\n");
    write_project_config(&env, "[env]\nB = \"9\"\nC = \"3\"\n");

    env.run("demo", &FakeBackend::default()).unwrap();

    let variables = &spec(&env)["environment"]["variables"];
    assert_eq!(variables["A"], "1");
    assert_eq!(variables["B"], "9");
    assert_eq!(variables["C"], "3");
}

fn created_skills(env: &Env) -> SkillsStore {
    let backend = FakeBackend::default();
    env.run("demo", &backend).unwrap();
    backend.creates()[0].skills
}

#[test]
fn project_skills_store_overrides_the_profile() {
    let env = Env::new();
    write_project_config(&env, "[skills]\nstore = \"off\"\n");

    assert_eq!(created_skills(&env), SkillsStore::Off);
}

#[test]
fn profile_skills_store_is_kept_when_the_project_sets_none() {
    let env = Env::new();
    env.write_profile("default", "[skills]\nstore = \"off\"\n");
    write_project_config(&env, "[env]\nA = \"1\"\n");

    assert_eq!(created_skills(&env), SkillsStore::Off);
}

/// Runs `new demo`, expecting an error; checks nothing was created.
fn rejected(env: &Env) -> String {
    let backend = FakeBackend::default();
    let err = env.run("demo", &backend).unwrap_err();
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm/demo/kits").exists());
    format!("{err:#}")
}

fn project_config_path(env: &Env) -> String {
    env.base_dir()
        .join(".sbxm")
        .join("demo")
        .join("sandbox.toml")
        .display()
        .to_string()
}

#[test]
fn unknown_key_in_project_config_is_an_error() {
    let env = Env::new();
    write_project_config(&env, "[netwrk]\nallow = [\"github.com\"]\n");

    let message = rejected(&env);

    assert!(
        message.contains(&format!(
            "invalid project config {}",
            project_config_path(&env)
        )),
        "{message}"
    );
}

#[test]
fn readwrite_skills_store_in_project_config_is_rejected() {
    let env = Env::new();
    write_project_config(&env, "[skills]\nstore = \"readwrite\"\n");

    let message = rejected(&env);

    assert!(
        message.contains("skills.store = \"readwrite\" in project config is not allowed"),
        "{message}"
    );
    assert!(message.contains(&project_config_path(&env)), "{message}");
}

#[test]
fn reserved_env_name_in_project_config_is_rejected() {
    let env = Env::new();
    write_project_config(&env, "[env]\nSBXM_PROFILE = \"x\"\n");

    let message = rejected(&env);

    assert!(
        message
            .contains("env name 'SBXM_PROFILE' in project config uses the reserved prefix SBXM_"),
        "{message}"
    );
    assert!(message.contains(&project_config_path(&env)), "{message}");
}
