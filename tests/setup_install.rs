//! Slice 15a: the profile's `[[setup.install]]` steps reach the sandbox
//! through the `common` mixin; a project's steps are appended.

mod common;

use common::Env;
use sbxm::backend::FakeBackend;

/// Writes `.sbxm/demo/sandbox.toml`.
fn write_project_config(env: &Env, contents: &str) {
    let dir = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sandbox.toml"), contents).unwrap();
}

/// `setup` of the generated common `spec.yaml`, as YAML.
fn setup_yaml(env: &Env) -> String {
    let text = std::fs::read_to_string(env.kit_dir("demo").join("spec.yaml")).unwrap();
    let spec: serde_norway::Value = serde_norway::from_str(&text).unwrap();
    serde_norway::to_string(&spec["setup"]).unwrap()
}

#[test]
fn profile_install_steps_become_the_common_mixins_setup() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[[setup.install]]\n\
         command = \"apt-get install -y ripgrep\"\n\
         description = \"ripgrep\"\n\
         \n\
         [[setup.install]]\n\
         command = \"echo hello\"\n\
         user = \"agent\"\n",
    );

    env.run("demo", &FakeBackend::default()).unwrap();

    insta::assert_snapshot!(setup_yaml(&env), @r"
    install:
    - command: apt-get install -y ripgrep
      description: ripgrep
    - command: echo hello
      user: agent
    ");
}

#[test]
fn project_install_steps_run_after_the_profiles() {
    let env = Env::new();
    env.write_profile("default", "[[setup.install]]\ncommand = \"echo profile\"\n");
    write_project_config(&env, "[[setup.install]]\ncommand = \"echo project\"\n");

    env.run("demo", &FakeBackend::default()).unwrap();

    insta::assert_snapshot!(setup_yaml(&env), @r"
    install:
    - command: echo profile
    - command: echo project
    ");
}

#[test]
fn no_install_steps_means_no_setup_section() {
    let env = Env::new();

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(setup_yaml(&env), "null\n");
}

#[test]
fn unknown_key_in_an_install_step_is_an_error() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[[setup.install]]\ncommand = \"echo hi\"\nrun_as = \"agent\"\n",
    );
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    assert!(format!("{err:#}").contains("run_as"), "{err:#}");
    assert!(backend.log().is_empty());
    assert!(!env.base_dir().join("demo").exists());
}
