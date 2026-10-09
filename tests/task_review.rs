//! M2b slice 8, step 3: one reviewer round (spec §5.2): the reviewer runs in its own sandbox on
//! its own clone of the task branch, can't change the worker's files, and its sandbox and clone
//! are removed afterwards, also on error.

mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play_reviewer, play_tasks,
    worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{self, Prepared, Tiers};
use sbxm::task::record::{self, Stage, Status};

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

/// Both providers' secrets stored; the worker's headless run finishes and commits one file; the
/// reviewer's headless run finishes and (as `review` says) writes its review.
fn backend_with(f: &Fixture, review: Option<&str>) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviewer(&f.env.base_dir(), "issue-41", review.map(str::to_owned));
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn clone_dir(f: &Fixture) -> std::path::PathBuf {
    f.env.base_dir().join("tasks").join("issue-41-review")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

/// A task whose worker finished and whose gates passed: ready for its first review.
fn gated(f: &Fixture, backend: &FakeBackend) -> Prepared {
    let mut prepared = worked_task(f, backend);
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(f);
    let ctx = ctx(f, &source, backend, &github);
    let gated = pipeline::run_gates(&ctx.env(), &mut prepared, "after-worker", Tiers::ALL).unwrap();
    assert!(gated.passed);
    prepared
}

fn round(
    f: &Fixture,
    backend: &FakeBackend,
    prepared: &mut Prepared,
    round: u32,
) -> anyhow::Result<pipeline::Reviewed> {
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(f);
    pipeline::run_reviewer(&ctx(f, &source, backend, &github).env(), prepared, round)
}

const CLEAN: &str = "Must-fix findings: 0\n\nChecked the diff against the issue; nothing found.\n";

#[test]
fn a_reviewer_round_saves_the_review_under_the_reviewers_name_and_cleans_up() {
    let f = config();
    let backend = backend_with(&f, Some(CLEAN));
    let mut prepared = gated(&f, &backend);

    let reviewed = round(&f, &backend, &mut prepared, 1).unwrap();

    assert_eq!((reviewed.round, reviewed.must_fix), (1, 0));
    let expected = format!("Reviewer: codex (default model)\n\n{CLEAN}");
    assert_eq!(
        fs::read_to_string(meta(&f).join("review.md")).unwrap(),
        expected
    );
    assert_eq!(
        fs::read_to_string(meta(&f).join("review-1.md")).unwrap(),
        expected
    );
    assert!(fs::read_to_string(meta(&f).join("transcripts").join("review-1.jsonl")).is_ok());

    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Completed)
    );
    let reviewer = record.reviewer.as_ref().unwrap();
    assert_eq!(
        (reviewer.harness.as_str(), reviewer.sandbox.as_str()),
        ("codex", "sbxm-task-issue-41-review-codex")
    );
    assert_eq!(reviewer.run.as_ref().unwrap().status, "completed");
    assert_eq!(prepared.record, record);

    // The reviewer's sandbox and clone are gone; the worker's are untouched.
    assert!(
        backend
            .removes()
            .contains(&"sbxm-task-issue-41-review-codex".to_owned())
    );
    assert!(!clone_dir(&f).exists());
    assert!(f.env.base_dir().join("tasks").join("issue-41").exists());
}

#[test]
fn the_reviewer_gets_its_own_sandbox_over_a_clone_of_the_task_branch() {
    let f = config();
    let seen = Arc::new(Mutex::new(None));
    let seen_in_hook = Arc::clone(&seen);
    let clone = clone_dir(&f);
    let inner = backend_with(&f, Some(CLEAN));
    let observer = move |spec: &sbxm::backend::ExecSpec| {
        if spec.argv.iter().any(|a| a == "codex") {
            *seen_in_hook.lock().unwrap() = Some((
                git(&clone, &["branch", "--show-current"]),
                git(&clone, &["log", "--oneline", "origin/main..HEAD"])
                    .lines()
                    .count(),
                clone.join("a.txt").is_file(),
                clone.join(".sbxm-task").join("issue.md").is_file(),
                clone.join(".sbxm-task").join("prompt.md").is_file(),
                clone.join(".sbxm-task").join("previous-review.md").exists(),
            ));
        }
    };
    let backend = inner.with_exec_hook({
        let worker = play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        );
        let reviewer = play_reviewer(&f.env.base_dir(), "issue-41", Some(CLEAN.to_owned()));
        move |sandbox, spec| {
            worker(sandbox, spec);
            observer(spec);
            reviewer(sandbox, spec);
        }
    });
    let mut prepared = gated(&f, &backend);

    round(&f, &backend, &mut prepared, 1).unwrap();

    let (branch, ahead, has_work, has_issue, has_prompt, has_previous) =
        seen.lock().unwrap().clone().expect("the reviewer ran");
    assert_eq!(branch, "issue-41");
    assert_eq!(
        ahead, 1,
        "the worker's commit is on the branch the reviewer sees"
    );
    assert!(has_work && has_issue && has_prompt);
    assert!(!has_previous, "round 1 has no previous review");

    let created = backend.creates();
    let reviewer_spec = created.iter().find(|c| c.name.contains("review")).unwrap();
    assert_eq!(reviewer_spec.agent, "codex");
    assert_eq!(reviewer_spec.workspace, clone_dir(&f));
    assert_ne!(
        reviewer_spec.workspace,
        f.env.base_dir().join("tasks").join("issue-41")
    );
}

#[test]
fn the_reviewers_context_includes_every_saved_review_even_with_gaps_in_the_round_numbers() {
    // Issue 169, part 1: `earlier` is built from `review::saved_rounds`, not from trying every
    // round number below `round`, so saved reviews that don't start at 1 and have gaps (gate
    // failures can use round numbers before the first review) are still all seen, oldest first.
    let f = config();
    let seen = Arc::new(Mutex::new(None));
    let seen_in_hook = Arc::clone(&seen);
    let clone = clone_dir(&f);
    let inner = backend_with(&f, Some(CLEAN));
    let observer = move |spec: &sbxm::backend::ExecSpec| {
        if spec.argv.iter().any(|a| a == "codex") {
            *seen_in_hook.lock().unwrap() =
                Some(fs::read_to_string(clone.join(".sbxm-task").join("previous-review.md")).ok());
        }
    };
    let backend = inner.with_exec_hook({
        let worker = play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        );
        let reviewer = play_reviewer(&f.env.base_dir(), "issue-41", Some(CLEAN.to_owned()));
        move |sandbox, spec| {
            worker(sandbox, spec);
            observer(spec);
            reviewer(sandbox, spec);
        }
    });
    let mut prepared = gated(&f, &backend);
    fs::write(
        meta(&f).join("review-0.md"),
        "zero review must be ignored\n",
    )
    .unwrap();
    fs::write(meta(&f).join("review-2.md"), "second review\n").unwrap();
    fs::write(meta(&f).join("review-5.md"), "fifth review\n").unwrap();

    round(&f, &backend, &mut prepared, 7).unwrap();

    let previous = seen
        .lock()
        .unwrap()
        .clone()
        .flatten()
        .expect("round 7 has earlier reviews to see");
    let at_2 = previous.find("second review").expect("review-2.md's text");
    let at_5 = previous.find("fifth review").expect("review-5.md's text");
    assert!(at_2 < at_5, "oldest first: {previous}");
    assert!(
        !previous.contains("zero review must be ignored"),
        "review-0.md must never be treated as an earlier review: {previous}"
    );
}

#[test]
fn the_reviewer_is_run_under_the_timeout_with_high_effort_on_a_prompt_file() {
    let f = config();
    let backend = backend_with(&f, Some(CLEAN));
    let mut prepared = gated(&f, &backend);

    round(&f, &backend, &mut prepared, 1).unwrap();

    let execs = backend.execs();
    let (sandbox, spec) = execs
        .iter()
        .find(|(_, s)| s.argv.iter().any(|a| a == "codex"))
        .unwrap();
    assert_eq!(sandbox, "sbxm-task-issue-41-review-codex");
    assert_eq!(&spec.argv[..2], ["timeout", "-v"]);
    assert!(
        spec.argv.iter().any(|a| a == "model_reasoning_effort=high"),
        "{:?}",
        spec.argv
    );
    assert!(
        spec.argv.iter().any(|a| a.contains(".sbxm-task/prompt.md")),
        "{:?}",
        spec.argv
    );
    // 45 minutes: the reviewer's default limit.
    assert_eq!(spec.argv[3], (45 * 60).to_string());
}

#[test]
fn a_review_without_the_count_line_is_kept_for_reading_but_not_used() {
    let f = config();
    let backend = backend_with(&f, Some("Looks fine to me.\n"));
    let mut prepared = gated(&f, &backend);

    let message = format!("{:#}", round(&f, &backend, &mut prepared, 1).unwrap_err());

    assert!(
        message.contains("Must-fix findings: <count>") && message.contains("isn't used"),
        "{message}"
    );
    assert!(
        fs::read_to_string(meta(&f).join("review-1.md"))
            .unwrap()
            .contains("Looks fine")
    );
    assert!(!meta(&f).join("review.md").exists());
    assert_eq!(saved(&f).status, Status::Failed);
    assert!(!clone_dir(&f).exists(), "cleaned up after an error too");
    assert!(
        backend
            .removes()
            .contains(&"sbxm-task-issue-41-review-codex".to_owned())
    );
}

#[test]
fn a_reviewer_that_writes_no_review_is_an_error_and_an_earlier_review_never_stands_in() {
    let f = config();
    let backend = backend_with(&f, None);
    let mut prepared = gated(&f, &backend);
    fs::write(
        meta(&f).join("review.md"),
        "Must-fix findings: 0\n(from an earlier run)\n",
    )
    .unwrap();

    let message = format!("{:#}", round(&f, &backend, &mut prepared, 1).unwrap_err());

    assert!(message.contains("wrote no review.md"), "{message}");
    assert!(
        !meta(&f).join("review.md").exists(),
        "the stale review must not stand in"
    );
    assert_eq!(saved(&f).status, Status::Failed);
}

#[test]
fn a_reviewer_that_hits_its_time_limit_is_a_failure_and_nothing_is_used() {
    let f = config();
    // The reviewer's headless command is killed by the in-sandbox timeout (a fresh backend: the
    // fake answers with the first matching scripted output).
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching(
            "codex",
            ExecOutput {
                stdout: String::new(),
                stderr: "timeout: sending signal TERM to command 'codex'\n".into(),
                exit_code: Some(124),
            },
        )
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook({
            let worker = play_tasks(
                &f.env.base_dir(),
                "main",
                Play {
                    commits: vec!["a.txt".into()],
                    result_md: Some(b"done\n".to_vec()),
                    bundle_bytes: None,
                },
            );
            let reviewer = play_reviewer(&f.env.base_dir(), "issue-41", Some(CLEAN.to_owned()));
            move |sandbox, spec| {
                worker(sandbox, spec);
                reviewer(sandbox, spec);
            }
        });
    let mut prepared = gated(&f, &backend);

    let message = format!("{:#}", round(&f, &backend, &mut prepared, 1).unwrap_err());

    assert!(message.contains("time limit"), "{message}");
    assert!(!meta(&f).join("review.md").exists());
    assert_eq!(saved(&f).status, Status::Failed);
}

#[test]
fn a_reviewer_that_cannot_be_run_still_leaves_nothing_behind() {
    let f = config();
    let backend = backend_with(&f, Some(CLEAN)).with_failing_exec_matching("codex");
    let mut prepared = gated(&f, &backend);

    assert!(round(&f, &backend, &mut prepared, 1).is_err());

    assert!(!clone_dir(&f).exists());
    assert!(
        backend
            .removes()
            .contains(&"sbxm-task-issue-41-review-codex".to_owned())
    );
    assert_eq!(saved(&f).status, Status::Failed);
}

#[test]
fn a_clone_left_by_a_killed_run_is_cleared_first() {
    let f = config();
    let backend = backend_with(&f, Some(CLEAN));
    let mut prepared = gated(&f, &backend);
    fs::create_dir_all(clone_dir(&f)).unwrap();
    fs::write(clone_dir(&f).join("stale.txt"), "from a killed run\n").unwrap();

    round(&f, &backend, &mut prepared, 1).unwrap();

    assert!(!clone_dir(&f).exists());
}

#[test]
fn round_two_gets_the_previous_review_and_keeps_both_files() {
    let f = config();
    let first = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";
    let backend = backend_with(&f, Some(first));
    let mut prepared = gated(&f, &backend);
    round(&f, &backend, &mut prepared, 1).unwrap();

    // The author had a round; its gates passed again (stage gating/passed), then a new review.
    prepared.record.stage = Stage::Gating;
    prepared.record.status = Status::Passed;
    let previous_seen = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&previous_seen);
    let clone = clone_dir(&f);
    let reviewer = play_reviewer(&f.env.base_dir(), "issue-41", Some(CLEAN.to_owned()));
    let backend2 = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if spec.argv.iter().any(|a| a == "codex") {
                *seen.lock().unwrap() =
                    fs::read_to_string(clone.join(".sbxm-task").join("previous-review.md")).ok();
            }
            reviewer(sandbox, spec);
        });

    let reviewed = round(&f, &backend2, &mut prepared, 2).unwrap();

    assert_eq!((reviewed.round, reviewed.must_fix), (2, 0));
    let previous = previous_seen
        .lock()
        .unwrap()
        .clone()
        .expect("previous-review.md was there");
    assert!(previous.contains("### M-1 - a.txt:1 wrong."), "{previous}");
    assert!(
        fs::read_to_string(meta(&f).join("review-1.md"))
            .unwrap()
            .contains("M-1 - a.txt:1")
    );
    assert!(
        fs::read_to_string(meta(&f).join("review-2.md"))
            .unwrap()
            .contains("nothing found")
    );
    assert!(
        fs::read_to_string(meta(&f).join("review.md"))
            .unwrap()
            .contains("nothing found")
    );
}

// ---- Checks before anything is created ----

#[test]
fn the_reviewers_secret_is_checked_before_anything_is_created() {
    let f = config();
    let worker_backend = backend_with(&f, Some(CLEAN));
    let prepared = gated(&f, &worker_backend);
    // Only anthropic is stored now; the codex reviewer needs openai.
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(&f);

    let message = format!(
        "{:#}",
        pipeline::check_reviewer(&ctx(&f, &source, &backend, &github).env(), &prepared)
            .unwrap_err()
    );

    assert!(
        message.contains("openai") && message.contains("sbx secret set openai"),
        "{message}"
    );
    assert!(backend.creates().is_empty() && backend.log().is_empty());
}

#[test]
fn the_check_returns_the_same_harness_warning_for_a_reviewer_like_the_worker() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n[gates]\nsandbox = []\n[reviewer]\nharness = \"claude\"\n",
    );
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let worker_backend = backend_with(&f, None);
    let prepared = gated(&f, &worker_backend);
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(&f);

    let warnings =
        pipeline::check_reviewer(&ctx(&f, &source, &backend, &github).env(), &prepared).unwrap();

    assert!(
        warnings.iter().any(|w| w.contains("same harness")),
        "{warnings:?}"
    );
}

#[test]
fn a_host_runner_is_never_involved_in_a_review() {
    // The reviewer runs in a sandbox; nothing about it touches the host runner.
    let f = config();
    let backend = backend_with(&f, Some(CLEAN));
    let mut prepared = gated(&f, &backend);
    let host = FakeHostRunner::default();
    round(&f, &backend, &mut prepared, 1).unwrap();
    assert!(host.calls().is_empty());
}
