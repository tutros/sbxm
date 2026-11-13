//! Issue 556: a spec task whose sandbox name would be over 63 characters gets the capped name,
//! `task.json` records that name, and `task rm` takes it as the task's own. A failing `sbx create`
//! carries `sbx`'s own words into the error.

mod common;

use std::fs;
use std::path::PathBuf;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, source};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, SpecOptions};
use sbxm::github::fake::FakeGitHub;
use sbxm::sandbox_name::MAX_LEN;
use sbxm::task::finish::plan_removal;
use sbxm::task::record;
use sbxm::task::repo::Identity;

/// A spec file whose name is the longest a spec id allows (40 characters), so the worker's
/// `sbxm-task-<id>-claude` would be 69 characters.
fn long_spec(f: &Fixture) -> PathBuf {
    let path = f.env.tmp.path().join(format!("{}.md", "s".repeat(40)));
    fs::write(&path, "Build a thing.\n").unwrap();
    path
}

fn options(f: &Fixture, spec: PathBuf) -> SpecOptions {
    SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    }
}

fn run(f: &Fixture, opts: &SpecOptions, backend: &FakeBackend) -> anyhow::Result<()> {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_start::run_spec(
        &f.env.config_dir(),
        opts,
        backend,
        &FakeGitHub::default(),
        &Probe,
        &mut out,
        &mut warn,
    )
}

fn playing(f: &Fixture, id: &str) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join(id),
            "main",
            id,
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

#[test]
fn a_long_spec_tasks_worker_sandbox_is_capped_recorded_and_removable() {
    let f = fixture();
    let spec = long_spec(&f);
    let (id, _) = record::spec_id(&spec).unwrap();
    let full = format!("sbxm-task-{id}-claude");
    assert!(full.len() > MAX_LEN, "{full}");
    let backend = playing(&f, &id);

    run(&f, &options(&f, spec), &backend).unwrap();

    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    let expected = record::sandbox_name(&id, "claude");
    assert!(expected.len() <= MAX_LEN, "{expected}");
    assert_eq!(created, vec![expected.clone()]);
    let task = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!(task.worker.as_ref().unwrap().sandbox, expected);
    assert!(
        task.sandboxes.iter().all(|s| s.name == expected),
        "{:?}",
        task.sandboxes
    );

    let plan = plan_removal(&f.env.base_dir(), &id, &Probe).unwrap();
    assert_eq!(plan.sandboxes, vec![expected]);
}

#[test]
fn a_failing_create_carries_the_backends_words_into_the_error() {
    let f = fixture();
    let spec = long_spec(&f);
    let (id, _) = record::spec_id(&spec).unwrap();
    let sandbox = record::sandbox_name(&id, "claude");
    let backend = playing(&f, &id)
        .with_failing_create_for(&sandbox)
        .with_create_failure_text("ERROR: invalid sandbox name: too long");

    let err = run(&f, &options(&f, spec), &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains(&sandbox), "{message}");
    assert!(
        message.contains("ERROR: invalid sandbox name: too long"),
        "{message}"
    );
}
