//! Slice 15c: `harness.claude.managed_settings` is written to
//! `/etc/claude-code/managed-settings.json` by a root install step in the
//! `harness-claude` mixin (decisions 59, 64).

mod common;

#[cfg(unix)]
use common::file_link;
use common::{Env, dir_link};
use sbxm::backend::FakeBackend;

/// Writes `<profiles_dir>/default/<relative>`.
fn write_profile_file(env: &Env, relative: &str, contents: &str) {
    let path = env.profiles_dir().join("default").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Writes `.sbxm/demo/<relative>`.
fn write_project_file(env: &Env, relative: &str, contents: &str) {
    let path = env.base_dir().join(".sbxm").join("demo").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A profile whose `managed_settings` is `claude/managed.json`, holding
/// `settings`.
fn with_managed_settings(env: &Env, settings: &str) {
    env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"claude/managed.json\"\n",
    );
    write_profile_file(env, "claude/managed.json", settings);
}

fn harness_spec(env: &Env) -> serde_json::Value {
    let text = std::fs::read_to_string(env.harness_kit_dir("demo").join("spec.yaml")).unwrap();
    serde_norway::from_str(&text).unwrap()
}

/// The shell command that writes `json` to the managed settings file.
fn install_command(json: &str) -> String {
    format!(
        "set -e\n\
         mkdir -p /etc/claude-code\n\
         cat > /etc/claude-code/managed-settings.json <<'SBXM_EOF'\n\
         {json}\n\
         SBXM_EOF\n\
         chmod 644 /etc/claude-code/managed-settings.json\n"
    )
}

/// Runs `new`, expects an error, and checks nothing was created.
fn refused(env: &Env) -> String {
    let backend = FakeBackend::default();
    let err = env.run("demo", &backend).unwrap_err();
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join("demo").exists());
    format!("{err:#}")
}

fn hash(env: &Env, project: &str) -> String {
    env.run(project, &FakeBackend::default()).unwrap();
    let path = env
        .base_dir()
        .join(".sbxm")
        .join(project)
        .join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    state["sandboxes"]["claude"]["config_hash"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn managed_settings_are_written_by_a_root_install_step() {
    let env = Env::new();
    with_managed_settings(
        &env,
        r#"{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "touch /tmp/started" } ] }
    ]
  }
}
"#,
    );

    env.run("demo", &FakeBackend::default()).unwrap();

    let spec = harness_spec(&env);
    let install = spec["setup"]["install"].as_array().unwrap();
    assert_eq!(install.len(), 1);
    assert_eq!(install[0]["user"], "0");
    assert_eq!(
        install[0]["description"],
        "sbxm: write Claude managed settings"
    );
    assert_eq!(
        install[0]["command"],
        install_command(
            r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"touch /tmp/started","type":"command"}]}]}}"#
        )
    );
}

#[test]
fn dollar_signs_are_escaped() {
    let env = Env::new();
    with_managed_settings(&env, r#"{"env": {"GREETING": "${{ x }} $HOME"}}"#);

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(
        harness_spec(&env)["setup"]["install"][0]["command"],
        // The JSON escape for `$`, built from char 92 so no tool decodes it.
        install_command(
            &r#"{"env":{"GREETING":"<D>{{ x }} <D>HOME"}}"#
                .replace("<D>", &format!("{}u0024", char::from(92)))
        )
    );
}

#[test]
fn project_managed_settings_replace_the_profiles() {
    let env = Env::new();
    with_managed_settings(&env, r#"{"from": "profile"}"#);
    write_project_file(
        &env,
        "sandbox.toml",
        "[harness.claude]\nmanaged_settings = \"managed.json\"\n",
    );
    write_project_file(&env, "managed.json", r#"{"from": "project"}"#);

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(
        harness_spec(&env)["setup"]["install"][0]["command"],
        install_command(r#"{"from":"project"}"#)
    );
}

#[test]
fn invalid_json_is_refused() {
    let env = Env::new();
    with_managed_settings(&env, "{ not json");

    let message = refused(&env);

    assert!(message.contains("managed.json"), "{message}");
    assert!(message.contains("is not a JSON object"), "{message}");
}

#[test]
fn json_that_is_not_an_object_is_refused() {
    let env = Env::new();
    with_managed_settings(&env, "[1, 2]");

    let message = refused(&env);

    assert!(message.contains("is not a JSON object"), "{message}");
}

#[test]
fn managed_settings_outside_the_profile_folder_are_refused() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"../x.json\"\n",
    );

    let message = refused(&env);

    assert!(
        message.contains("harness.claude.managed_settings = \"../x.json\""),
        "{message}"
    );
}

#[test]
fn managed_settings_through_a_link_are_refused() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"linked/managed.json\"\n",
    );
    let outside = env.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("managed.json"), "{}").unwrap();
    dir_link(&env.profiles_dir().join("default").join("linked"), &outside);

    let message = refused(&env);

    assert!(
        message.contains(&format!(
            "harness.claude.managed_settings = \"linked/managed.json\" in profile 'default' \
             passes through the symlink or junction {}",
            env.profiles_dir().join("default").join("linked").display()
        )),
        "{message}"
    );
}

#[test]
fn project_managed_settings_through_a_link_are_refused() {
    let env = Env::new();
    let outside = env.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("managed.json"), "{}").unwrap();
    let metadata = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(
        metadata.join("sandbox.toml"),
        "[harness.claude]\nmanaged_settings = \"linked/managed.json\"\n",
    )
    .unwrap();
    dir_link(&metadata.join("linked"), &outside);

    let message = refused(&env);

    assert!(
        message.contains(&format!(
            "harness.claude.managed_settings = \"linked/managed.json\" in project config \
             passes through the symlink or junction {}",
            metadata.join("linked").display()
        )),
        "{message}"
    );
}

#[test]
#[cfg(unix)]
fn managed_settings_file_itself_a_link_is_refused() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"managed.json\"\n",
    );
    let outside = env.tmp.path().join("outside.json");
    std::fs::write(&outside, "{}").unwrap();
    file_link(
        &env.profiles_dir().join("default").join("managed.json"),
        &outside,
    );

    let message = refused(&env);

    assert!(
        message.contains(&format!(
            "harness.claude.managed_settings = \"managed.json\" in profile 'default' passes \
             through the symlink or junction {}",
            env.profiles_dir()
                .join("default")
                .join("managed.json")
                .display()
        )),
        "{message}"
    );
}

#[test]
fn missing_managed_settings_file_is_refused() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"claude/managed.json\"\n",
    );

    let message = refused(&env);

    assert!(message.contains("is missing or not a file"), "{message}");
}

#[test]
fn reformatting_keeps_the_hash_and_content_changes_it() {
    let env = Env::new();
    with_managed_settings(&env, r#"{"a": 1, "b": 2}"#);
    let before = hash(&env, "one");
    with_managed_settings(&env, "{\n  \"b\": 2,\n  \"a\": 1\n}\n");
    let reformatted = hash(&env, "two");
    with_managed_settings(&env, r#"{"a": 1, "b": 3}"#);

    assert_eq!(before, reformatted);
    assert_ne!(before, hash(&env, "three"));
}
