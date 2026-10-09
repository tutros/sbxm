//! Issue 142 (decision 174(d)): `sbxm task review --spec <FILE>` as a command: the same
//! gates/review/fix-round loop as `--issue`'s; the "ready" output names `task finish --spec` as the
//! next step (issue 143: the `local` sink).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, ctx, fixture_with, ok, play, play_reviews,
    source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_review::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline;
use sbxm::task::record;

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

fn backend_for(f: &Fixture, id: &str, reviews: &[&str]) -> FakeBackend {
    let worker = play(
        &f.env.base_dir().join("tasks").join(id),
        "main",
        id,
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        id,
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

/// Prepares the spec task and runs its worker (through the pipeline directly, as `task start
/// --spec` would), leaving it at `working/completed`.
fn worked_spec_task(f: &Fixture, spec: &Path, backend: &FakeBackend) {
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, backend, &github);
    let mut prepared = pipeline::prepare_spec(&context, spec).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
}

fn options(f: &Fixture, spec: PathBuf) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Spec(spec),
        repo: Some("o/r".into()),
        base: None,
        clone_source: None,
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn go(f: &Fixture, opts: &Options, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

#[test]
fn a_clean_review_ends_ready_and_names_finish_spec_as_the_next_step() {
    let f = config();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(&f, &id, &[CLEAN]);
    worked_spec_task(&f, &spec, &backend);
    let github = FakeGitHub::default();

    let out = go(&f, &options(&f, spec), &backend, &github);

    out.result.unwrap();
    assert!(out.out.contains(&format!("{id}: ready")), "{}", out.out);
    assert!(out.out.contains("review.md"), "{}", out.out);
    assert!(
        out.out.contains("next: sbxm task finish --spec"),
        "{}",
        out.out
    );
    assert!(!out.out.contains("refused"), "{}", out.out);
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert!(out.warn.is_empty(), "{}", out.warn);
}

#[test]
fn a_must_fix_finding_gets_a_fix_round_then_ends_ready() {
    let f = config();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(&f, &id, &[ONE, CLEAN, CLEAN]);
    worked_spec_task(&f, &spec, &backend);
    let github = FakeGitHub::default();

    let out = go(&f, &options(&f, spec), &backend, &github);

    out.result.unwrap();
    assert!(
        out.out.contains("the fix round ran 1 time(s)"),
        "{}",
        out.out
    );
    assert!(out.out.contains(&format!("{id}: ready")), "{}", out.out);
}

#[test]
fn failed_gates_say_how_to_fix_and_retry_with_spec() {
    let f = config();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(&f, &id, &[CLEAN]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "assertion failed".into(),
            exit_code: Some(1),
        },
    );
    worked_spec_task(&f, &spec, &backend);
    let github = FakeGitHub::default();

    let out = go(&f, &options(&f, spec.clone()), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains(&format!("sbxm task review --spec {}", spec.display())),
        "{message}"
    );
}

#[test]
fn the_gate_failure_hint_quotes_a_spec_path_with_spaces() {
    let f = config();
    let dir = f.env.tmp.path().join("My Specs");
    fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("idea.md");
    fs::write(&spec, "Build a thing.\n").unwrap();
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(&f, &id, &[CLEAN]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "assertion failed".into(),
            exit_code: Some(1),
        },
    );
    worked_spec_task(&f, &spec, &backend);

    let out = go(
        &f,
        &options(&f, spec.clone()),
        &backend,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    let want = format!("sbxm task review --spec \"{}\"", spec.display());
    assert!(message.contains(&want), "{message}");
}
