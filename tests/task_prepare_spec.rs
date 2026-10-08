//! Issue 142 (decision 174(d)): `task start --spec <file>`'s prepare phase. Mirrors
//! `tests/task_prepare.rs` for the issue path, but the source is a file read from disk instead
//! of a GitHub issue: no GitHub call is made, and the file's text becomes `source.md`.

mod common;

use std::fs;
use std::path::PathBuf;

use common::git;
use common::task_fixture::{Fixture, backend, ctx, fixture, source};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Stage, Status};

fn base_has_nothing_new(f: &Fixture) -> bool {
    let empty_or_absent =
        |dir: PathBuf| fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none());
    empty_or_absent(f.env.base_dir().join("tasks"))
        && empty_or_absent(f.env.base_dir().join(".sbxm").join("tasks"))
}

fn spec_file(f: &Fixture, name: &str, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join(name);
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_prepared_spec_task_has_its_folders_repo_clone_source_md_sandbox_and_record() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let spec = spec_file(&f, "idea.md", "Build a thing.\n");

    let prepared = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();

    let id = &prepared.record.id;
    assert!(id.starts_with("spec-idea-"), "{id}");
    let meta = f.env.base_dir().join(".sbxm").join("tasks").join(id);
    let workspace = f.env.base_dir().join("tasks").join(id);
    assert_eq!(
        (prepared.meta.as_path(), prepared.workspace.as_path()),
        (meta.as_path(), workspace.as_path())
    );
    assert!(meta.join("repo.git").is_dir());
    assert_eq!(git(&workspace, &["branch", "--show-current"]), *id);

    assert_eq!(
        fs::read_to_string(meta.join("source.md")).unwrap(),
        "Build a thing.\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join(".sbxm-task").join("source.md")).unwrap(),
        "Build a thing.\n"
    );
    // There is no issue.md for a spec task: the worker prompt points at source.md instead.
    assert!(!workspace.join(".sbxm-task").join("issue.md").exists());
    let prompt = fs::read_to_string(workspace.join(".sbxm-task").join("prompt.md")).unwrap();
    assert!(
        prompt.contains("source.md") && prompt.contains("- `cargo test`"),
        "{prompt}"
    );
    assert!(!prompt.contains("GitHub issue"), "{prompt}");

    let record = record::read(&meta.join("task.json")).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Running)
    );
    assert_eq!(record.id, *id);
    assert_eq!(record.branch, *id);
    assert_eq!(record.title, "idea.md");
    let worker = record.worker.unwrap();
    assert_eq!(worker.harness, "claude");
    assert!(!record.config_hash.is_empty());

    let log = backend.log();
    assert!(
        log.iter()
            .any(|l| l == &format!("create {}", worker.sandbox)),
        "{log:?}"
    );
    // No GitHub calls at all for a spec task.
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}

#[test]
fn starting_the_same_spec_file_twice_is_refused_and_changes_nothing() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let spec = spec_file(&f, "idea.md", "text");
    let prepared = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap();
    let id = prepared.record.id.clone();
    let calls = backend.log().len();

    let message = format!(
        "{:#}",
        pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap_err()
    );

    assert!(
        message.contains(&id) && message.contains("stage prepared"),
        "{message}"
    );
    assert_eq!(backend.log().len(), calls);
    // The first task's own folders still exist; a refused restart deleted nothing.
    assert!(!base_has_nothing_new(&f));
}

#[test]
fn a_missing_spec_file_is_refused_before_anything_is_written() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let missing = f.env.tmp.path().join("missing.md");

    let message = format!(
        "{:#}",
        pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &missing).unwrap_err()
    );

    assert!(message.contains("missing.md"), "{message}");
    assert!(base_has_nothing_new(&f));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[cfg(unix)]
#[test]
fn a_spec_file_that_is_a_link_is_refused_before_anything_is_written() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    let real = spec_file(&f, "real.md", "Build a thing.\n");
    let link = f.env.tmp.path().join("idea.md");
    common::file_link(&link, &real);

    let message = format!(
        "{:#}",
        pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &link).unwrap_err()
    );

    assert!(message.contains("is a link"), "{message}");
    assert!(base_has_nothing_new(&f));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_missing_provider_secret_is_refused_before_anything_is_written() {
    let f = fixture();
    let (backend, github) = (FakeBackend::with_secrets(&[]), FakeGitHub::default());
    let source = source(&f);
    let spec = spec_file(&f, "idea.md", "text");

    let message = format!(
        "{:#}",
        pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &spec).unwrap_err()
    );

    assert!(
        message.contains("anthropic") && message.contains("sbx secret set"),
        "{message}"
    );
    assert!(base_has_nothing_new(&f));
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn two_spec_files_with_the_same_name_get_different_tasks() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);
    fs::create_dir_all(f.env.tmp.path().join("a")).unwrap();
    fs::create_dir_all(f.env.tmp.path().join("b")).unwrap();
    let a = spec_file(&f, "a/idea.md", "a");
    let b = spec_file(&f, "b/idea.md", "b");

    let prepared_a = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &a).unwrap();
    let prepared_b = pipeline::prepare_spec(&ctx(&f, &source, &backend, &github), &b).unwrap();

    assert_ne!(prepared_a.record.id, prepared_b.record.id);
    assert!(prepared_a.record.id.starts_with("spec-idea-"));
    assert!(prepared_b.record.id.starts_with("spec-idea-"));
}
