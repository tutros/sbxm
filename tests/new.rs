use std::path::{Path, PathBuf};

mod common;

use common::{Env, dir_link};
use sbxm::backend::{CreateSpec, FakeBackend, SkillsStore};
use sbxm::commands::new;
use sbxm::harness::Harness;

fn expected_create(env: &Env, workspace: &Path) -> CreateSpec {
    CreateSpec {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        workspace: workspace.to_path_buf(),
        cpus: 4,
        memory: "8g".into(),
        skills: SkillsStore::ReadOnly,
        kits: vec![env.kit_dir("demo"), env.harness_kit_dir("demo")],
    }
}

#[test]
fn creates_workspace_and_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let workspace = env.base_dir().join("demo");
    assert!(workspace.is_dir());
    assert_eq!(backend.creates(), vec![expected_create(&env, &workspace)]);
}

#[test]
fn reuses_existing_workspace_and_keeps_its_contents() {
    let env = Env::new();
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.md"), "keep me").unwrap();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
        "keep me"
    );
    assert_eq!(backend.creates(), vec![expected_create(&env, &workspace)]);
}

#[test]
fn missing_base_dir_is_a_clear_error_and_creates_nothing() {
    let env = Env::new();
    std::fs::remove_dir(env.base_dir()).unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("base dir"), "{message}");
    assert!(
        message.contains(&env.base_dir().display().to_string()),
        "{message}"
    );
    assert!(!env.base_dir().exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn missing_config_points_to_config_init() {
    let env = Env::new();
    std::fs::remove_file(env.config_dir().join("config.toml")).unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("sbxm config init"), "{message}");
    assert!(backend.creates().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("Demo", &backend).unwrap_err();

    assert!(backend.creates().is_empty());
    assert!(std::fs::read_dir(env.base_dir()).unwrap().next().is_none());
}

fn state_path(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("demo").join("state.json")
}

#[test]
fn writes_state_for_the_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run("demo", &backend).unwrap();

    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_path(&env)).unwrap()).unwrap();
    let claude = &state["sandboxes"]["claude"];
    assert_eq!(claude["sandbox"], "sbxm-demo-claude");
    assert_eq!(
        claude["workspace"],
        env.base_dir().join("demo").to_str().unwrap()
    );
    assert!(claude["created_at"].as_u64().unwrap() > 0);
}

#[test]
fn failed_create_writes_no_state() {
    let env = Env::new();
    let backend = FakeBackend::failing_create();

    env.run("demo", &backend).unwrap_err();

    assert!(!state_path(&env).exists());
}

fn seeded(seed: PathBuf) -> new::Options {
    new::Options {
        seed: Some(seed),
        ..Default::default()
    }
}

#[test]
fn seed_is_copied_into_a_new_workspace() {
    let env = Env::new();
    let backend = FakeBackend::default();

    env.run_with("demo", &seeded(env.seed()), &backend).unwrap();

    let workspace = env.base_dir().join("demo");
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "a"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("sub").join("b.txt")).unwrap(),
        "b"
    );
    assert_eq!(backend.creates(), vec![expected_create(&env, &workspace)]);
}

#[test]
fn seed_on_existing_project_is_an_error_and_changes_nothing() {
    let env = Env::new();
    let workspace = env.base_dir().join("demo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.md"), "keep me").unwrap();
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(env.seed()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("already exists"), "{message}");
    assert!(message.contains("without --seed"), "{message}");
    assert!(backend.creates().is_empty());
    let names: Vec<_> = std::fs::read_dir(&workspace)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["notes.md"]);
}

#[test]
fn missing_seed_is_an_error_and_creates_nothing() {
    let env = Env::new();
    let missing = env.tmp.path().join("no-such-seed");
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(missing.clone()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn seed_that_is_a_file_is_an_error_and_creates_nothing() {
    let env = Env::new();
    let seed_file = env.tmp.path().join("seed.txt");
    std::fs::write(&seed_file, "not a dir").unwrap();
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(seed_file.clone()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert_eq!(
        message,
        format!(
            "seed {} is not a directory; pass a folder with --seed",
            seed_file.display()
        )
    );
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
    // Decision 47: the seed is checked before any kit is written or validated.
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join(".sbxm").exists());
}

#[test]
fn seed_containing_the_base_dir_is_an_error() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = env
        .run_with("demo", &seeded(env.tmp.path().to_path_buf()), &backend)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("contains the base dir"), "{message}");
    assert!(!env.base_dir().join("demo").exists());
    assert!(backend.creates().is_empty());
}

#[test]
fn seed_containing_a_link_is_an_error_and_copies_nothing() {
    let env = Env::new();
    let secret = env.tmp.path().join("secret");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(secret.join("key.txt"), "secret").unwrap();
    let seed = env.seed();
    dir_link(&seed.join("sub").join("linked"), &secret);
    let backend = FakeBackend::default();

    let err = env.run_with("demo", &seeded(seed), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("is a symlink or junction"), "{message}");
    assert!(message.contains("linked"), "{message}");
    assert!(!env.base_dir().join("demo").exists());
    // Decision 47: the seed is checked before any kit is written or validated.
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(!env.base_dir().join(".sbxm").exists());
}

/// Decision 76: `default_harness` was written by `config init` but never read,
/// so a config that still has it fails instead of being silently ignored.
#[test]
fn default_harness_in_config_is_an_error_and_creates_nothing() {
    let env = Env::new();
    let path = env.config_dir().join("config.toml");
    let config = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("default_harness = \"codex\"\n{config}")).unwrap();
    let backend = FakeBackend::default();

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(&format!(
            "default_harness in {} isn't an sbxm setting (use --harness; the default is claude); \
             delete that line",
            path.display()
        )),
        "{message}"
    );
    assert!(backend.log().is_empty());
    assert!(!env.base_dir().join("demo").exists());
}

/// Decision 77: unknown keys in config.toml are errors, as in profiles
/// (decision 51), so a typo never silently falls back to a default.
#[test]
fn unknown_config_keys_are_errors_and_create_nothing() {
    for (line, key) in [
        ("default_profle = \"strict\"\n", "default_profle"),
        ("[resources]\ncpu = 2\n", "cpu"),
    ] {
        let env = Env::new();
        let path = env.config_dir().join("config.toml");
        let config = std::fs::read_to_string(&path).unwrap();
        let config = if line.starts_with("[resources]") {
            config.replace("[resources]\n", line)
        } else {
            format!("{line}{config}")
        };
        std::fs::write(&path, config).unwrap();
        let backend = FakeBackend::default();

        let err = env.run("demo", &backend).unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains(&format!("invalid config {}", path.display())),
            "{message}"
        );
        assert!(
            message.contains(&format!("unknown field `{key}`")),
            "{message}"
        );
        assert!(backend.log().is_empty());
        assert!(!env.base_dir().join("demo").exists());
    }
}

fn assert_refuses_existing(harness: Harness, hint: &str) {
    let env = Env::new();
    let options = new::Options {
        harness,
        ..Default::default()
    };
    env.run_with("demo", &options, &FakeBackend::default())
        .unwrap();
    let kits = env.base_dir().join(".sbxm").join("demo").join("kits");
    let kits_before = std::fs::read_dir(&kits).unwrap().count();
    env.write_profile("default", "[env]\nCHANGED = \"since\"\n");
    let backend = FakeBackend::default();

    let err = env.run_with("demo", &options, &backend).unwrap_err();

    let sandbox = format!("sbxm-demo-{}", harness.as_str());
    assert_eq!(
        format!("{err:#}"),
        format!("sandbox {sandbox} already exists; open it with `{hint}`")
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert_eq!(std::fs::read_dir(&kits).unwrap().count(), kits_before);
}

#[test]
fn existing_sandbox_is_refused_before_any_write_or_call() {
    assert_refuses_existing(Harness::Claude, "sbxm open demo");
}

#[test]
fn existing_codex_sandbox_is_refused_with_the_harness_in_the_hint() {
    assert_refuses_existing(Harness::Codex, "sbxm open demo --harness codex");
}

#[test]
fn invalid_kit_names_the_profile_and_the_project_sandbox_toml() {
    let env = Env::new();
    let metadata_dir = env.base_dir().join(".sbxm").join("demo");
    std::fs::create_dir_all(&metadata_dir).unwrap();
    std::fs::write(metadata_dir.join("sandbox.toml"), "").unwrap();
    let backend = FakeBackend::with_invalid_kit("manifest: bad host");

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    let profile_toml = env
        .profiles_dir()
        .join("default")
        .join("profile.toml")
        .display()
        .to_string();
    let sandbox_toml = metadata_dir.join("sandbox.toml").display().to_string();
    assert!(message.contains(&profile_toml), "{message}");
    assert!(message.contains(&sandbox_toml), "{message}");
    assert!(!message.contains("network entries"), "{message}");
}

#[test]
fn invalid_kit_without_a_sandbox_toml_names_only_the_profile() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("manifest: bad host");

    let err = env.run("demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    let profile_toml = env
        .profiles_dir()
        .join("default")
        .join("profile.toml")
        .display()
        .to_string();
    let sandbox_toml = env
        .base_dir()
        .join(".sbxm")
        .join("demo")
        .join("sandbox.toml")
        .display()
        .to_string();
    assert!(message.contains(&profile_toml), "{message}");
    assert!(!message.contains(&sandbox_toml), "{message}");
    assert!(!message.contains("network entries"), "{message}");
}
