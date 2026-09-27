//! Slice 16: `sbxm config show [project] [--profile p] [--kits]` prints the
//! hash input, the hash and optionally the generated kits, creating nothing
//! (decision 65).

mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::{config_show, new};
use sbxm::harness::Harness;

fn show(env: &Env, project: Option<&str>, options: &config_show::Options) -> String {
    config_show::render(&env.config_dir(), project, options).unwrap()
}

/// The `# config hash: …` value in `output`.
fn printed_hash(output: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix("# config hash: "))
        .expect("hash line")
        .to_owned()
}

/// `output` with the hash and sbxm's version replaced, for snapshots.
fn stable(output: &str) -> String {
    output
        .replace(&printed_hash(output), "<hash>")
        .replace(env!("CARGO_PKG_VERSION"), "<version>")
}

fn recorded_hash(env: &Env, project: &str) -> String {
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

/// Writes `<profiles_dir>/default/<relative>`.
fn write_profile_file(env: &Env, relative: &str, contents: &str) {
    let path = env.profiles_dir().join("default").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Writes `.sbxm/demo/sandbox.toml`.
fn write_project_config(env: &Env, contents: &str) {
    let dir = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sandbox.toml"), contents).unwrap();
}

#[test]
fn prints_the_hash_input_of_a_profile() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[network]\nallow = [\"github.com\"]\n\n\
         [env]\nFLAG = \"1\"\n\n\
         [instructions]\nmandatory = \"mandatory.md\"\n\n\
         [[setup.install]]\ncommand = \"echo hi\"\n\n\
         [harness.claude]\nhome_files = \"home\"\n",
    );
    write_profile_file(&env, "mandatory.md", "Run the tests.\n");
    write_profile_file(&env, "home/a.txt", "a");

    let output = show(&env, None, &config_show::Options::default());

    insta::assert_snapshot!(stable(&output), @r#"
    # profile: default
    # config hash: <hash>

    sbxm_version = "<version>"
    harness = "claude"
    profile_name = "default"

    [profile]
    skills_store = "readonly"
    mandatory_instructions = """
    Run the tests.
    """

    [profile.network]
    allow = ["github.com"]
    deny = []

    [profile.env]
    FLAG = "1"

    [profile.secrets]
    services = []

    [[profile.setup.install]]
    command = "echo hi"

    [profile.claude_home_files]
    "a.txt" = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"

    [resources]
    cpus = 4
    memory = "8g"
    "#);
}

#[test]
fn project_hash_is_the_one_new_records() {
    let env = Env::new();
    write_project_config(&env, "[env]\nFROM_PROJECT = \"yes\"\n");
    env.run("demo", &FakeBackend::default()).unwrap();

    let output = show(&env, Some("demo"), &config_show::Options::default());

    assert_eq!(printed_hash(&output), recorded_hash(&env, "demo"));
    assert!(output.contains("# project: demo"), "{output}");
    assert!(output.contains("FROM_PROJECT = \"yes\""), "{output}");
}

#[test]
fn project_uses_the_profile_recorded_in_its_state() {
    let env = Env::new();
    env.write_profile("other", "[env]\nOTHER = \"1\"\n");
    let options = new::Options {
        profile: Some("other".into()),
        ..new::Options::default()
    };
    env.run_with("demo", &options, &FakeBackend::default())
        .unwrap();

    let output = show(&env, Some("demo"), &config_show::Options::default());

    assert!(output.contains("# profile: other"), "{output}");
    assert_eq!(printed_hash(&output), recorded_hash(&env, "demo"));
}

#[test]
fn profile_flag_overrides_the_recorded_profile() {
    let env = Env::new();
    env.write_profile("other", "[env]\nOTHER = \"1\"\n");
    env.run("demo", &FakeBackend::default()).unwrap();

    let options = config_show::Options {
        profile: Some("other".into()),
        ..config_show::Options::default()
    };
    let output = show(&env, Some("demo"), &options);

    assert!(output.contains("# profile: other"), "{output}");
    assert!(output.contains("OTHER = \"1\""), "{output}");
}

#[test]
fn kits_flag_prints_the_generated_kits_and_creates_nothing() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[network]\nallow = [\"github.com\"]\n\n\
         [instructions]\nmandatory = \"mandatory.md\"\n",
    );
    write_profile_file(&env, "mandatory.md", "Run the tests.\n");
    let options = config_show::Options {
        kits: true,
        ..config_show::Options::default()
    };

    let output = show(&env, Some("demo"), &options);

    let stable = stable(&output);
    let kits = &stable[stable.find("# kit: common").expect("common kit")..];
    insta::assert_snapshot!(kits, @r"
    # kit: common
    schemaVersion: '2'
    kind: mixin
    name: sbxm-common
    description: Generated by sbxm from profile 'default'. Do not edit.
    permissions:
      network:
        allow:
        - github.com
        deny: []
    environment:
      variables:
        SBXM_CONFIG_HASH: <hash>
        SBXM_PROFILE: default

    # kit: harness-claude
    schemaVersion: '2'
    kind: mixin
    name: sbxm-harness-claude
    description: Generated by sbxm from profile 'default' for claude. Do not edit.
    requires:
      agent: claude
    # files/home/.claude/CLAUDE.md
    ");
    assert!(!env.base_dir().join("demo").exists());
    assert!(!env.base_dir().join(".sbxm").exists());
}

#[test]
fn invalid_project_name_is_refused() {
    let env = Env::new();

    let err = config_show::render(
        &env.config_dir(),
        Some("Bad"),
        &config_show::Options::default(),
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("Bad"), "{err:#}");
}

#[test]
fn cli_prints_the_config() {
    let env = Env::new();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["config", "show", "demo", "--kits"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("# config hash: "), "{stdout}");
    assert!(stdout.contains("# kit: harness-claude"), "{stdout}");
}

/// Slice 18c: `--harness` shows that harness's hash input and kits (decision 69).
#[test]
fn codex_uses_its_own_recorded_profile_and_hash() {
    let env = Env::new();
    env.write_profile("other", "[env]\nOTHER = \"1\"\n");
    env.run("demo", &FakeBackend::default()).unwrap();
    let codex = new::Options {
        profile: Some("other".into()),
        harness: Harness::Codex,
        ..new::Options::default()
    };
    env.run_with("demo", &codex, &FakeBackend::default())
        .unwrap();
    let options = config_show::Options {
        harness: Harness::Codex,
        ..config_show::Options::default()
    };

    let output = show(&env, Some("demo"), &options);

    assert!(output.contains("# profile: other"), "{output}");
    assert!(output.contains("harness = \"codex\""), "{output}");
    let path = env.base_dir().join(".sbxm").join("demo").join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        printed_hash(&output),
        state["sandboxes"]["codex"]["config_hash"].as_str().unwrap()
    );
}

#[test]
fn codex_kits_are_printed() {
    let env = Env::new();
    env.write_profile("default", "[instructions]\nmandatory = \"mandatory.md\"\n");
    write_profile_file(&env, "mandatory.md", "Run the tests.\n");
    let options = config_show::Options {
        kits: true,
        harness: Harness::Codex,
        ..config_show::Options::default()
    };

    let output = show(&env, None, &options);

    assert!(output.contains("# kit: harness-codex"), "{output}");
    assert!(output.contains("# files/home/.codex/AGENTS.md"), "{output}");
    assert!(!output.contains("harness-claude"), "{output}");
}
