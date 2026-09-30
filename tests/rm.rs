mod common;

use std::cell::RefCell;
use std::path::PathBuf;

use common::{Env, dir_link};
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::commands::{new, rm};
use sbxm::confirm::Confirm;
use sbxm::harness::Harness;

/// Answers prompts from a script and records them.
struct FakeConfirm {
    interactive: bool,
    answer: bool,
    prompts: RefCell<Vec<String>>,
}

impl FakeConfirm {
    fn new(interactive: bool, answer: bool) -> Self {
        Self {
            interactive,
            answer,
            prompts: RefCell::new(Vec::new()),
        }
    }

    /// For commands that must never ask.
    fn never() -> Self {
        Self::new(true, false)
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.borrow().clone()
    }
}

impl Confirm for FakeConfirm {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.prompts.borrow_mut().push(prompt.to_owned());
        Ok(self.answer)
    }
}

fn plain_rm(env: &Env, project: &str, backend: &FakeBackend) -> anyhow::Result<()> {
    let confirm = FakeConfirm::never();
    let result = rm::run(
        &env.config_dir(),
        project,
        &rm::Options::default(),
        backend,
        &confirm,
    );
    assert!(confirm.prompts().is_empty(), "plain rm must not prompt");
    result
}

fn purge(env: &Env, yes: bool, backend: &FakeBackend, confirm: &FakeConfirm) -> anyhow::Result<()> {
    let options = rm::Options {
        purge: true,
        yes,
        ..Default::default()
    };
    rm::run(&env.config_dir(), "demo", &options, backend, confirm)
}

fn metadata_dir(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("demo")
}

/// `new demo` with a file in the workspace.
fn setup() -> Env {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    std::fs::write(env.base_dir().join("demo").join("notes.md"), "keep me").unwrap();
    env
}

#[test]
fn removes_sandbox_and_state_and_keeps_workspace() {
    let env = setup();
    let backend = FakeBackend::default();

    plain_rm(&env, "demo", &backend).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-claude"]);
    assert!(!metadata_dir(&env).exists());
    assert_eq!(
        std::fs::read_to_string(env.base_dir().join("demo").join("notes.md")).unwrap(),
        "keep me"
    );
}

#[test]
fn keeps_other_files_in_the_metadata_dir() {
    let env = setup();
    std::fs::write(metadata_dir(&env).join("sandbox.toml"), "# mine\n").unwrap();
    let backend = FakeBackend::default();

    plain_rm(&env, "demo", &backend).unwrap();

    assert!(!metadata_dir(&env).join("state.json").exists());
    assert_eq!(
        std::fs::read_to_string(metadata_dir(&env).join("sandbox.toml")).unwrap(),
        "# mine\n"
    );
}

#[test]
fn unknown_project_points_to_list() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = plain_rm(&env, "demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("no sbxm sandbox for project 'demo'; `sbxm list` shows existing ones"),
        "{message}"
    );
    assert!(backend.removes().is_empty());
}

#[test]
fn wrong_harness_names_the_harnesses_the_project_has() {
    let env = Env::new();
    let codex = new::Options {
        harness: Harness::Codex,
        ..Default::default()
    };
    env.run_with("demo", &codex, &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::default();

    let err = plain_rm(&env, "demo", &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("has a codex sandbox; use `--harness codex`"),
        "{message}"
    );
    assert!(backend.removes().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    plain_rm(&env, "Demo", &backend).unwrap_err();

    assert!(backend.removes().is_empty());
}

fn demo_sandbox() -> SandboxInfo {
    SandboxInfo {
        name: "sbxm-demo-claude".into(),
        agent: "claude".into(),
        status: "running".into(),
    }
}

#[test]
fn failed_remove_keeps_state() {
    let env = setup();
    // The sandbox is still there, so the failure is real.
    let backend = FakeBackend::failing_remove().and_sandboxes(vec![demo_sandbox()]);

    plain_rm(&env, "demo", &backend).unwrap_err();

    assert!(metadata_dir(&env).join("state.json").exists());
}

#[test]
fn sandbox_already_gone_still_clears_state() {
    let env = setup();
    // `sbx rm` fails for a sandbox that no longer exists.
    let backend = FakeBackend::failing_remove();

    plain_rm(&env, "demo", &backend).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-claude"]);
    assert!(!metadata_dir(&env).exists());
    assert!(workspace(&env).join("notes.md").exists());
}

#[test]
fn purge_works_when_the_sandbox_is_already_gone() {
    let env = setup();
    let backend = FakeBackend::failing_remove();

    purge(&env, true, &backend, &FakeConfirm::never()).unwrap();

    assert!(!workspace(&env).exists());
    assert!(!metadata_dir(&env).exists());
}

fn workspace(env: &Env) -> PathBuf {
    env.base_dir().join("demo")
}

#[test]
fn purge_after_confirmation_deletes_workspace_and_metadata() {
    let env = setup();
    std::fs::write(metadata_dir(&env).join("sandbox.toml"), "# mine\n").unwrap();
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(true, true);

    purge(&env, false, &backend, &confirm).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-claude"]);
    assert!(!workspace(&env).exists());
    assert!(!metadata_dir(&env).exists());
    let prompts = confirm.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&workspace(&env).display().to_string()),
        "{prompts:?}"
    );
    assert!(
        prompts[0].contains(&metadata_dir(&env).display().to_string()),
        "{prompts:?}"
    );
}

#[test]
fn purge_declined_changes_nothing() {
    let env = setup();
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(true, false);

    let err = purge(&env, false, &backend, &confirm).unwrap_err();

    assert!(format!("{err:#}").contains("purge cancelled; nothing was deleted"));
    assert!(backend.removes().is_empty());
    assert!(workspace(&env).join("notes.md").exists());
    assert!(metadata_dir(&env).join("state.json").exists());
}

#[test]
fn purge_without_terminal_needs_yes() {
    let env = setup();
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(false, true);

    let err = purge(&env, false, &backend, &confirm).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("--yes"), "{message}");
    assert!(
        message.contains(&workspace(&env).display().to_string()),
        "{message}"
    );
    assert!(confirm.prompts().is_empty());
    assert!(backend.removes().is_empty());
    assert!(workspace(&env).exists());
}

#[test]
fn purge_with_yes_skips_the_prompt() {
    let env = setup();
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(false, false);

    purge(&env, true, &backend, &confirm).unwrap();

    assert!(confirm.prompts().is_empty());
    assert!(!workspace(&env).exists());
    assert!(!metadata_dir(&env).exists());
}

#[test]
fn purge_refuses_a_linked_workspace_and_deletes_nothing() {
    let env = setup();
    let outside = env.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), "keep").unwrap();
    std::fs::remove_dir_all(workspace(&env)).unwrap();
    dir_link(&workspace(&env), &outside);
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(true, true);

    let err = purge(&env, true, &backend, &confirm).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("is a symlink or junction"), "{message}");
    assert!(backend.removes().is_empty());
    assert!(outside.join("keep.txt").exists());
    assert!(workspace(&env).symlink_metadata().is_ok());
    assert!(metadata_dir(&env).join("state.json").exists());
}

#[test]
fn purge_after_plain_rm_deletes_the_workspace() {
    let env = setup();
    plain_rm(&env, "demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::default();

    purge(&env, true, &backend, &FakeConfirm::never()).unwrap();

    assert!(backend.removes().is_empty());
    assert!(!workspace(&env).exists());
}

#[test]
fn purge_with_nothing_to_delete_is_an_error() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = purge(&env, true, &backend, &FakeConfirm::never()).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("nothing to purge for project 'demo'; `sbxm list` shows existing ones"),
        "{message}"
    );
}

/// `new demo` for Claude, then for Codex.
fn setup_both() -> Env {
    let env = setup();
    let codex = new::Options {
        harness: Harness::Codex,
        ..Default::default()
    };
    env.run_with("demo", &codex, &FakeBackend::default())
        .unwrap();
    env
}

fn rm_codex(env: &Env, backend: &FakeBackend) -> anyhow::Result<()> {
    let options = rm::Options {
        harness: Harness::Codex,
        ..Default::default()
    };
    rm::run(
        &env.config_dir(),
        "demo",
        &options,
        backend,
        &FakeConfirm::never(),
    )
}

#[test]
fn rm_one_harness_keeps_the_other_and_its_kit() {
    let env = setup_both();
    let backend = FakeBackend::default();

    rm_codex(&env, &backend).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-codex"]);
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(metadata_dir(&env).join("state.json")).unwrap(),
    )
    .unwrap();
    let harnesses: Vec<_> = state["sandboxes"].as_object().unwrap().keys().collect();
    assert_eq!(harnesses, ["claude"]);
    let kits: Vec<_> = std::fs::read_dir(metadata_dir(&env).join("kits"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(kits.len(), 1, "{kits:?}");
    assert!(kits[0].join("harness-claude").is_dir(), "{kits:?}");
}

#[test]
fn rm_missing_harness_names_it() {
    let env = setup();
    let backend = FakeBackend::default();

    let err = rm_codex(&env, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "no sbxm codex sandbox for project 'demo', which has a claude sandbox; use `--harness claude`"
        ),
        "{message}"
    );
    assert!(backend.removes().is_empty());
}

#[test]
fn purge_removes_every_sandbox_of_the_project() {
    let env = setup_both();
    let backend = FakeBackend::default();
    let confirm = FakeConfirm::new(true, true);

    purge(&env, false, &backend, &confirm).unwrap();

    assert_eq!(backend.removes(), ["sbxm-demo-claude", "sbxm-demo-codex"]);
    assert!(!workspace(&env).exists());
    assert!(!metadata_dir(&env).exists());
    let prompts = confirm.prompts();
    assert!(
        prompts[0].contains("sbxm-demo-claude") && prompts[0].contains("sbxm-demo-codex"),
        "{prompts:?}"
    );
}
