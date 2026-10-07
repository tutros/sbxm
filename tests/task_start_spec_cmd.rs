//! Issue 142 (decision 174(d)): `sbxm task start --spec <FILE>` as a command: it prepares the
//! task, runs the worker, then the gates, and prints what happened; no GitHub call is made.

mod common;

use std::fs;
use std::path::PathBuf;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, source};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, SpecOptions};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record;
use sbxm::task::repo::Identity;

fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

fn options(f: &Fixture, spec: PathBuf) -> SpecOptions {
    SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn run(f: &Fixture, opts: &SpecOptions, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run_spec(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &Probe,
        &mut out,
        &mut warn,
    );
    let (out, warn) = (
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap(),
    );
    if let Err(e) = &result {
        eprintln!("task start --spec failed: {e:#}\n--- out ---\n{out}\n--- warn ---\n{warn}");
    }
    Out { result, out, warn }
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
fn a_started_spec_task_prints_what_happened_and_the_next_step() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(&f, &id);
    let github = FakeGitHub::default();

    let out = run(&f, &options(&f, spec), &backend, &github);

    out.result.unwrap();
    assert!(out.out.contains(&id), "{}", out.out);
    assert!(
        out.out.contains("completed") && out.out.contains("1 commit"),
        "{}",
        out.out
    );
    assert!(out.out.contains("gates: passed"), "{}", out.out);
    assert!(out.out.contains("sbxm task review --spec"), "{}", out.out);
    // No GitHub call anywhere in the flow (no --base override would need default_branch either).
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert!(out.warn.is_empty(), "{}", out.warn);
}

#[test]
fn a_failed_worker_is_reported_and_the_command_fails() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let sandbox = format!("sbxm-task-{id}-claude");
    let backend = playing(&f, &id).with_failing_create_for(&sandbox);
    let github = FakeGitHub::default();

    let out = run(&f, &options(&f, spec), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains(&sandbox), "{message}");
}

#[test]
fn a_missing_spec_file_is_refused_before_anything_is_written() {
    let f = fixture();
    let missing = f.env.tmp.path().join("missing.md");
    let backend = backend();
    let github = FakeGitHub::default();

    let out = run(&f, &options(&f, missing), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("missing.md"), "{message}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}
