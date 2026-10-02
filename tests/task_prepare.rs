//! M2b slice 6, step 2: `task start`'s checks before acting and its prepare phase (spec §5.1
//! steps 1 and 2): every refusal happens before a write, and a failure while preparing leaves
//! nothing behind.

mod common;

use std::fs;
use std::path::PathBuf;

use common::git;
use common::task_fixture::{
    Fixture, backend, ctx, fixture, fixture_with, issue_text, open_issue, source,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Stage, Status};

fn base_has_nothing_new(f: &Fixture) -> bool {
    // The shared containers may stay (empty); no task folder may.
    let empty_or_absent =
        |dir: PathBuf| fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none());
    empty_or_absent(f.env.base_dir().join("tasks"))
        && empty_or_absent(f.env.base_dir().join(".sbxm").join("tasks"))
}

#[test]
fn a_prepared_task_has_its_folders_repo_clone_files_sandbox_and_record() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    let prepared =
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    let meta = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-41");
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    assert_eq!(
        (prepared.meta.as_path(), prepared.workspace.as_path()),
        (meta.as_path(), workspace.as_path())
    );
    assert!(meta.join("repo.git").is_dir());
    assert_eq!(git(&workspace, &["branch", "--show-current"]), "issue-41");
    assert_eq!(
        git(&workspace, &["config", "user.email"]),
        "dev@example.com"
    );

    // The issue text goes in both places; the agent reads the copy in its workspace.
    assert_eq!(
        fs::read_to_string(meta.join("issue.md")).unwrap(),
        issue_text(41).text
    );
    assert_eq!(
        fs::read_to_string(workspace.join(".sbxm-task").join("issue.md")).unwrap(),
        issue_text(41).text
    );
    let prompt = fs::read_to_string(workspace.join(".sbxm-task").join("prompt.md")).unwrap();
    assert!(
        prompt.contains("#41") && prompt.contains("issue-41") && prompt.contains("- `cargo test`"),
        "{prompt}"
    );
    assert_eq!(
        fs::read_to_string(meta.join("worker-prompt.md")).unwrap(),
        prompt
    );

    // The agent's `git add -A` must not pick up sbxm's own files.
    let exclude = fs::read_to_string(workspace.join(".git").join("info").join("exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == ".sbxm-task/"), "{exclude}");

    let record = record::read(&meta.join("task.json")).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Running)
    );
    assert_eq!(
        (record.id.as_str(), record.number, record.base.as_str()),
        ("issue-41", 41, "main")
    );
    assert_eq!(record.branch, "issue-41");
    assert_eq!(record.title, "Fix 41");
    let worker = record.worker.unwrap();
    assert_eq!(worker.sandbox, "sbxm-task-issue-41-claude");
    assert_eq!(worker.harness, "claude");
    assert!(
        worker.workspace.ends_with("tasks/issue-41"),
        "{}",
        worker.workspace
    );
    assert!(!record.config_hash.is_empty());

    let log = backend.log();
    assert!(
        log.iter().any(|l| l == "create sbxm-task-issue-41-claude"),
        "{log:?}"
    );
}

#[test]
fn the_sandbox_is_created_over_the_workspace_with_the_profiles_kits() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    let prepared =
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    let specs = backend.creates();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].workspace, prepared.workspace);
    assert_eq!(specs[0].agent, "claude");
    assert_eq!((specs[0].cpus, specs[0].memory.as_str()), (4, "8g"));
    assert_eq!(specs[0].kits.len(), 2);
    assert!(
        specs[0]
            .kits
            .iter()
            .all(|k| k.starts_with(prepared.meta.join("kits")))
    );
}

#[test]
fn the_configs_resources_override_the_global_ones() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\ncpus = 2\nmemory = \"3g\"\n[gates]\nsandbox = []\n",
    );
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    let spec = &backend.creates()[0];
    assert_eq!((spec.cpus, spec.memory.as_str()), (2, "3g"));
}

#[test]
fn a_missing_provider_secret_is_refused_before_anything_is_written() {
    let f = fixture();
    let (backend, github) = (FakeBackend::with_secrets(&[]), FakeGitHub::default());
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(
        message.contains("anthropic") && message.contains("sbx secret set"),
        "{message}"
    );
    assert!(base_has_nothing_new(&f));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_profile_service_secret_that_is_missing_is_refused_too() {
    let f = fixture();
    f.env
        .write_profile("default", "[secrets]\nservices = [\"github\"]\n");
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(message.contains("github"), "{message}");
    assert!(base_has_nothing_new(&f));
}

#[test]
fn an_unknown_profile_is_refused_before_anything_is_written() {
    let f = fixture_with("[sandbox]\nprofile = \"nope\"\n[gates]\nsandbox = []\n");
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(message.contains("nope"), "{message}");
    assert!(base_has_nothing_new(&f));
}

#[test]
fn a_closed_issue_is_refused_before_anything_is_written() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let mut issue = issue_text(41);
    issue.state = "CLOSED".into();

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue).unwrap_err()
    );

    assert!(
        message.contains("#41") && message.contains("not open"),
        "{message}"
    );
    assert!(base_has_nothing_new(&f));
}

#[test]
fn an_existing_task_is_refused_with_its_stage_and_nothing_changes() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();
    let before = fs::read_to_string(
        f.env
            .base_dir()
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .join("task.json"),
    )
    .unwrap();
    let calls = backend.log().len();

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(
        message.contains("task issue-41 exists (stage prepared)"),
        "{message}"
    );
    assert!(
        message.contains("task status") && message.contains("--restart"),
        "{message}"
    );
    assert_eq!(
        backend.log().len(),
        calls,
        "no backend call for a refused task"
    );
    let after = fs::read_to_string(
        f.env
            .base_dir()
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .join("task.json"),
    )
    .unwrap();
    assert_eq!(before, after);
}

#[test]
fn an_invalid_kit_leaves_no_folders_behind() {
    let f = fixture();
    let (backend, github) = (
        FakeBackend::with_invalid_kit("bad mixin"),
        FakeGitHub::default(),
    );
    let backend = backend.and_secrets(&["anthropic"]);
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(message.contains("bad mixin"), "{message}");
    assert!(base_has_nothing_new(&f));
}

#[test]
fn a_clone_that_fails_leaves_no_folders_behind() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let missing = f.env.tmp.path().join("no-such-origin.git");

    let message = format!(
        "{:#}",
        pipeline::prepare(
            &ctx(&f, missing.to_str().unwrap(), &backend, &github),
            &issue_text(41)
        )
        .unwrap_err()
    );

    assert!(message.contains("no-such-origin"), "{message}");
    assert!(base_has_nothing_new(&f));
    assert!(backend.creates().is_empty());
}

#[test]
fn a_failed_sandbox_create_removes_the_folders() {
    let f = fixture();
    let github = FakeGitHub::default();
    let backend = FakeBackend::failing_create().and_secrets(&["anthropic"]);
    let source = source(&f);

    let message = format!(
        "{:#}",
        pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap_err()
    );

    assert!(message.contains("sbxm-task-issue-41-claude"), "{message}");
    assert!(base_has_nothing_new(&f));
}

// ---- Selection: which issues get a task (spec §5.1 step 1, decision 154) ----

#[test]
fn selection_picks_by_the_rules_and_counts_existing_tasks_as_in_progress() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(1)).unwrap();
    let github = FakeGitHub::default().with_open_issues(vec![
        open_issue(1, &["should-fix"], ""),
        open_issue(2, &["must-fix"], ""),
        open_issue(3, &["question"], ""),
        open_issue(4, &["should-fix"], "**Depends on:** #2"),
    ]);

    let selection = pipeline::select_issues(&ctx(&f, &source, &backend, &github), None, 2).unwrap();

    assert_eq!(selection.picks, [2]);
    let reasons: Vec<String> = selection
        .skips
        .iter()
        .map(|(n, r)| format!("#{n}: {r}"))
        .collect();
    assert!(
        reasons.contains(&"#1: already has a task".to_owned()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"#3: a question, needs your answer first".to_owned()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"#4: blocked by open #2".to_owned()),
        "{reasons:?}"
    );
}

#[test]
fn nothing_to_start_is_an_error_listing_why() {
    let f = fixture();
    let (backend, source) = (backend(), source(&f));
    let github = FakeGitHub::default().with_open_issues(vec![open_issue(3, &["question"], "")]);

    let message = format!(
        "{:#}",
        pipeline::select_issues(&ctx(&f, &source, &backend, &github), None, 1).unwrap_err()
    );

    assert!(
        message.contains("nothing to start") && message.contains("#3: a question"),
        "{message}"
    );
}

#[test]
fn an_explicit_issue_that_is_not_open_is_named_in_the_error() {
    let f = fixture();
    let (backend, source) = (backend(), source(&f));
    let github = FakeGitHub::default().with_open_issues(vec![open_issue(1, &[], "")]);

    let message = format!(
        "{:#}",
        pipeline::select_issues(&ctx(&f, &source, &backend, &github), Some(&[9]), 1).unwrap_err()
    );

    assert!(
        message.contains("#9") && message.contains("isn't an open issue"),
        "{message}"
    );
}

#[test]
fn the_record_exists_before_the_slow_sandbox_setup_starts() {
    let f = fixture();
    let meta = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-41");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen_in_hook = seen.clone();
    let backend = backend().with_create_hook(move |_| {
        *seen_in_hook.lock().unwrap() = Some(record::read(&meta.join("task.json")).is_ok());
    });
    let github = FakeGitHub::default();
    let source = source(&f);

    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(true));
}
