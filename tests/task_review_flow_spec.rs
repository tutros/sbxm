//! Issue 142 (decision 174(d)): a spec task runs the same gates/review/fix-round flow as an issue
//! task's (`pipeline::review_issue`, reused unchanged: it already calls no GitHub), but the
//! reviewer gets the spec wording (`Role::ReviewerSpec`) and there is no PR to comment on.

mod common;

use std::fs;
use std::path::PathBuf;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play, play_reviews, source,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, Prepared};
use sbxm::task::record::{self, Stage, Status};

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n1. must-fix: a.txt:1 does the wrong thing.\n";

fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

/// A backend that plays the worker (and fix round) in `id`'s workspace and the reviewer in its
/// `<id>-review` clone, the n-th review being `reviews[n]` (the last repeats).
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

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

/// Prepares the spec task and runs its worker, leaving it at `working/completed`. Returns the
/// backend's recorded GitHub-call counterpart alongside, so a test can prove none were made.
fn worked_spec_task(
    f: &Fixture,
    text: &str,
    reviews: &[&str],
) -> (Prepared, FakeBackend, FakeGitHub) {
    let spec = spec_file(f, text);
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = backend_for(f, &id, reviews);
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_spec(&context, &spec).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    (prepared, backend, github)
}

#[test]
fn a_clean_review_runs_the_gates_first_and_ends_ready_without_a_fix_round() {
    let f = config();
    let (mut prepared, backend, github) = worked_spec_task(&f, "Build a thing.\n", &[CLEAN]);
    let id = prepared.record.id.clone();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);
    let env = context.env();

    let report = pipeline::review_issue(&env, &mut prepared).unwrap();

    assert_eq!(report.must_fix_left, 0);
    assert!(!report.fix_ran && report.gates_failed.is_none());
    let record = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert_eq!(count(&backend, "cargo test"), 1);
    assert_eq!(count(&backend, "codex"), 1);
    assert_eq!(count(&backend, "fix-prompt.md"), 0);

    let meta = record::task_dir(&f.env.base_dir(), &id);
    assert!(
        fs::read_to_string(meta.join("review.md"))
            .unwrap()
            .contains("Nothing found.")
    );
    // The reviewer got the spec wording, not the GitHub-issue one.
    let reviewer_prompt = fs::read_to_string(meta.join("reviewer-prompt-1.md")).unwrap();
    assert!(
        reviewer_prompt.contains("no pull request")
            && reviewer_prompt.contains(".sbxm-task/source.md"),
        "{reviewer_prompt}"
    );
    assert!(
        !reviewer_prompt.contains("GitHub issue"),
        "{reviewer_prompt}"
    );

    // Nothing calls GitHub, anywhere in the flow.
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}

#[test]
fn a_must_fix_finding_gets_a_fix_round_with_the_spec_wording_then_a_clean_full_review() {
    let f = config();
    let (mut prepared, backend, github) =
        worked_spec_task(&f, "Build a thing.\n", &[ONE, CLEAN, CLEAN]);
    let id = prepared.record.id.clone();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);
    let env = context.env();

    let report = pipeline::review_issue(&env, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert_eq!(report.must_fix_left, 0);
    let record = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(record.stopped.is_none());

    let meta = record::task_dir(&f.env.base_dir(), &id);
    let fix_prompt = fs::read_to_string(meta.join("fix-prompt.md")).unwrap();
    assert!(fix_prompt.contains(".sbxm-task/review.md"), "{fix_prompt}");
    assert!(!fix_prompt.contains("issue #"), "{fix_prompt}");
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}
