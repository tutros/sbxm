//! Slice 15b: `harness.claude.home_files` is copied into the
//! `harness-claude` mixin's `files/home/` (decisions 61, 63).

mod common;

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

/// A profile whose `home_files` is `home/`.
fn with_home_files(env: &Env) {
    env.write_profile("default", "[harness.claude]\nhome_files = \"home\"\n");
}

fn home_file(env: &Env, relative: &str) -> Option<String> {
    let path = env
        .harness_kit_dir("demo")
        .join("files")
        .join("home")
        .join(relative);
    std::fs::read_to_string(path).ok()
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
fn home_files_are_copied_into_the_harness_mixin() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/.claude/agents/reviewer.md", "review");
    write_profile_file(&env, "home/.bash_aliases", "alias ll='ls -l'");

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(
        home_file(&env, ".claude/agents/reviewer.md").as_deref(),
        Some("review")
    );
    assert_eq!(
        home_file(&env, ".bash_aliases").as_deref(),
        Some("alias ll='ls -l'")
    );
}

#[test]
fn project_home_files_replace_the_profiles() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/from-profile.txt", "profile");
    write_project_file(
        &env,
        "sandbox.toml",
        "[harness.claude]\nhome_files = \"myhome\"\n",
    );
    write_project_file(&env, "myhome/from-project.txt", "project");

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(
        home_file(&env, "from-project.txt").as_deref(),
        Some("project")
    );
    assert_eq!(home_file(&env, "from-profile.txt"), None);
}

#[test]
fn claude_md_is_allowed_without_mandatory_instructions() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/.claude/CLAUDE.md", "mine");

    env.run("demo", &FakeBackend::default()).unwrap();

    assert_eq!(
        home_file(&env, ".claude/CLAUDE.md").as_deref(),
        Some("mine")
    );
}

#[test]
fn home_files_outside_the_profile_folder_are_refused() {
    let env = Env::new();
    env.write_profile("default", "[harness.claude]\nhome_files = \"../other\"\n");
    std::fs::create_dir_all(env.profiles_dir().join("other")).unwrap();

    let message = refused(&env);

    assert!(
        message.contains("harness.claude.home_files = \"../other\""),
        "{message}"
    );
    assert!(
        message.contains("must be a relative path inside"),
        "{message}"
    );
}

#[test]
fn missing_home_files_folder_is_refused() {
    let env = Env::new();
    with_home_files(&env);

    let message = refused(&env);

    assert!(message.contains("is missing or not a folder"), "{message}");
}

#[test]
fn link_inside_home_files_is_refused() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/ok.txt", "ok");
    let outside = env.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    dir_link(
        &env.profiles_dir().join("default").join("home").join("link"),
        &outside,
    );

    let message = refused(&env);

    assert!(message.contains("is a symlink or junction"), "{message}");
}

#[test]
fn claude_settings_json_is_refused() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/.claude/settings.json", "{}");

    let message = refused(&env);

    assert!(message.contains(".claude/settings.json"), "{message}");
    assert!(message.contains("replaced by the Claude kit"), "{message}");
    assert!(message.contains("managed settings"), "{message}");
}

#[test]
fn claude_md_with_mandatory_instructions_is_refused() {
    let env = Env::new();
    env.write_profile(
        "default",
        "[instructions]\nmandatory = \"mandatory.md\"\n\n\
         [harness.claude]\nhome_files = \"home\"\n",
    );
    write_profile_file(&env, "mandatory.md", "rules");
    write_profile_file(&env, "home/.claude/CLAUDE.md", "mine");

    let message = refused(&env);

    assert!(message.contains(".claude/CLAUDE.md"), "{message}");
    assert!(message.contains("instructions.mandatory"), "{message}");
}

#[test]
fn home_file_contents_change_the_hash() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/a.txt", "one");
    let before = hash(&env, "one");
    write_profile_file(&env, "home/a.txt", "two");

    assert_ne!(before, hash(&env, "two"));
}

#[test]
fn home_file_rename_changes_the_hash() {
    let env = Env::new();
    with_home_files(&env);
    write_profile_file(&env, "home/a.txt", "same");
    let before = hash(&env, "one");
    std::fs::rename(
        env.profiles_dir().join("default/home/a.txt"),
        env.profiles_dir().join("default/home/b.txt"),
    )
    .unwrap();

    assert_ne!(before, hash(&env, "two"));
}
