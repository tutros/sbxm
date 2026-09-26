//! Slice 12: `.sbxm/<project>/sandbox.toml` is merged over the profile
//! (decision 57).

mod common;

use common::Env;
use sbxm::backend::FakeBackend;

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
