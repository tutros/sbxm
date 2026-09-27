//! Slice 10a: `new` loads and validates the selected profile before acting.

mod common;

use common::Env;
use sbxm::backend::{FakeBackend, SkillsStore};
use sbxm::commands::new;

fn new_with_profile(env: &Env, profile: Option<&str>, backend: &FakeBackend) -> anyhow::Result<()> {
    let options = new::Options {
        profile: profile.map(String::from),
        ..Default::default()
    };
    new::run(
        &env.config_dir(),
        "demo",
        &options,
        backend,
        &mut std::io::sink(),
    )
}

fn assert_nothing_created(env: &Env, backend: &FakeBackend) {
    assert!(
        backend.log().is_empty(),
        "backend calls: {:?}",
        backend.log()
    );
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm").exists());
}

#[test]
fn selected_profile_is_loaded_instead_of_the_default() {
    let env = Env::new();
    env.write_profile("default", "not valid toml [");
    env.write_profile("strict", "description = \"strict\"\n");
    let backend = FakeBackend::default();

    new_with_profile(&env, Some("strict"), &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn default_profile_is_loaded_without_flag() {
    let env = Env::new();
    env.write_profile("default", "not valid toml [");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    let path = env.profiles_dir().join("default").join("profile.toml");
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn missing_profile_says_how_to_fix_it() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, Some("strict"), &backend).unwrap_err();

    let message = format!("{err:#}");
    let path = env.profiles_dir().join("strict").join("profile.toml");
    assert!(
        message.contains(&format!(
            "profile 'strict' not found at {}; create it or pick another with --profile",
            path.display()
        )),
        "{message}"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn unknown_profile_key_is_an_error() {
    let env = Env::new();
    env.write_profile("default", "[netwrk]\nallow = [\"github.com\"]\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("netwrk"), "{message}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn invalid_profile_name_is_rejected() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, Some("../x"), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("invalid profile name '../x'"), "{message}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn starter_profile_sections_parse() {
    let env = Env::new();
    env.write_profile(
        "default",
        "description = \"Default profile\"\n\n\
         [network]\nallow = [\"github.com\"]\ndeny = []\n\n\
         [env]\nEXAMPLE = \"1\"\n\n\
         [secrets]\nservices = []\n",
    );
    let backend = FakeBackend::default();

    new_with_profile(&env, None, &backend).unwrap();
}

#[test]
fn default_profile_setting_picks_a_non_default_name() {
    let env = Env::new();
    let config = env.config_dir().join("config.toml");
    let text = std::fs::read_to_string(&config).unwrap().replace(
        "default_profile = \"default\"",
        "default_profile = \"team\"",
    );
    std::fs::write(&config, text).unwrap();
    env.write_profile("default", "not valid toml [");
    env.write_profile("team", "description = \"team\"\n");
    let backend = FakeBackend::default();

    new_with_profile(&env, None, &backend).unwrap();

    assert_eq!(backend.creates().len(), 1);
}

#[test]
fn invalid_env_name_is_rejected_with_the_profile_named() {
    let env = Env::new();
    env.write_profile("default", "[env]\n\"MY-VAR\" = \"1\"\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("invalid env name 'MY-VAR' in profile 'default'"),
        "{message}"
    );
    assert!(message.contains("letters, digits and '_'"), "{message}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn env_value_with_kit_expression_is_rejected() {
    let env = Env::new();
    env.write_profile("default", "[env]\nGREETING = \"${{ kit.args.name }}\"\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("env value for 'GREETING' in profile 'default' contains '${{'"),
        "{message}"
    );
    assert_nothing_created(&env, &backend);
}

#[test]
fn env_name_with_reserved_sbxm_prefix_is_rejected() {
    let env = Env::new();
    env.write_profile("default", "[env]\nSBXM_PROFILE = \"other\"\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "env name 'SBXM_PROFILE' in profile 'default' uses the reserved prefix SBXM_"
        ),
        "{message}"
    );
    assert_nothing_created(&env, &backend);
}

fn created_skills(env: &Env, profile: &str) -> anyhow::Result<SkillsStore> {
    env.write_profile("default", profile);
    let backend = FakeBackend::default();
    new_with_profile(env, None, &backend)?;
    Ok(backend.creates()[0].skills)
}

#[test]
fn skills_store_defaults_to_readonly() {
    let env = Env::new();
    assert_eq!(created_skills(&env, "").unwrap(), SkillsStore::ReadOnly);
}

#[test]
fn skills_store_can_be_off() {
    let env = Env::new();
    assert_eq!(
        created_skills(&env, "[skills]\nstore = \"off\"\n").unwrap(),
        SkillsStore::Off
    );
}

#[test]
fn skills_store_readwrite_is_rejected_with_the_reason() {
    let env = Env::new();
    env.write_profile("default", "[skills]\nstore = \"readwrite\"\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("skills.store = \"readwrite\" in profile 'default' is not allowed"),
        "{message}"
    );
    assert!(message.contains("every other sandbox"), "{message}");
    assert!(message.contains("use \"readonly\" or \"off\""), "{message}");
    assert_nothing_created(&env, &backend);
}

#[test]
fn skills_store_unknown_value_lists_the_allowed_ones() {
    let env = Env::new();
    env.write_profile("default", "[skills]\nstore = \"maybe\"\n");
    let backend = FakeBackend::default();

    let err = new_with_profile(&env, None, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("\"maybe\""), "{message}");
    assert!(message.contains("use \"readonly\" or \"off\""), "{message}");
    assert_nothing_created(&env, &backend);
}
