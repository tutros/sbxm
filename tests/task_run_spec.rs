//! Issue 194 (decision 174(d)): `sbxm task run --spec FILE` is `start --spec` then `review --spec`
//! for one spec task, and stops before `finish`, the same way `task run --issue N` does.

mod common;

use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, CLEAN_REVIEW, CODEX_DONE, Fixture, Play, Probe, fixture_with, ok, play,
    play_reviews, source, spec_file,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_finish;
use sbxm::commands::task_run::{SpecOptions, run_spec};
use sbxm::commands::task_start::Restart;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo::Identity;

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn spec(f: &Fixture) -> PathBuf {
    spec_file(f, "Build a thing.\n")
}

fn id_of(spec: &Path) -> String {
    record::spec_id(spec).unwrap().0
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
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        })
}

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

fn options(f: &Fixture, spec: PathBuf) -> SpecOptions {
    SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
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
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn go(
    f: &Fixture,
    opts: &SpecOptions,
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_spec(
        &f.env.config_dir(),
        opts,
        restart,
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

fn stage(f: &Fixture, id: &str) -> (Stage, Status) {
    let task = record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap();
    (task.stage, task.status)
}

#[test]
fn it_starts_then_reviews_and_stops_before_finish() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        None,
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    let text = &out.out;
    let worker = text.find(&format!("{id}: worker completed")).expect(text);
    let review = text.find(&format!("{id}: review round 1")).expect(text);
    assert!(worker < review, "{text}");
    assert!(text.contains(&format!("{id}: ready")), "{text}");
    // Only the last step's hint is left, and it is the finish command.
    assert!(!text.contains("sbxm task review --spec"), "{text}");
    assert!(
        text.contains(&format!("next: sbxm task finish --spec {}", s.display())),
        "{text}"
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
    // No GitHub call anywhere in this path.
    assert!(FakeGitHub::default().calls().is_empty());
}

/// Issue 140, decision 180(b): a post-worker gate failure doesn't stop `run --spec`; the review
/// reruns the gates and gives the failure to a fix round.
#[test]
fn failing_gates_after_the_worker_go_on_to_the_review_and_its_fix_round() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let fixed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]).with_exec_responder(move |_, spec| {
        let mentions = |needle: &str| spec.argv.iter().any(|a| a.contains(needle));
        if mentions("fix-prompt.md") {
            fixed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        (mentions("cargo test") && !fixed.load(std::sync::atomic::Ordering::SeqCst)).then(|| {
            ExecOutput {
                stdout: String::new(),
                stderr: "undefined reference to `oops`\n".into(),
                exit_code: Some(101),
            }
        })
    });

    let out = go(
        &f,
        &options(&f, s.clone()),
        None,
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    let text = &out.out;
    let failed = text
        .find("gates: failed: `cargo test` (sandbox, exit 101)")
        .expect(text);
    assert!(!text.contains("next: sbxm task review"), "{text}");
    let review = text.find(&format!("{id}: review round 2")).expect(text);
    assert!(failed < review, "{text}");
    assert!(text.contains("the fix round ran 1 time(s)"), "{text}");
    assert!(
        text.contains(&format!("next: sbxm task finish --spec {}", s.display())),
        "{text}"
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
}

/// Issue 186: unlike a gate that ran and failed, one that could not run at all gives the review
/// nothing to react to, so `task run --spec` stops after the start phase instead of going on to it.
#[test]
fn a_sandbox_gate_that_cannot_run_stops_before_any_reviewer() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]).with_failing_exec_matching("cargo test");

    let out = go(
        &f,
        &options(&f, s.clone()),
        None,
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("gates could not run"), "{message}");
    assert!(out.out.contains("gates: could not run"), "{}", out.out);
    assert!(!out.out.contains("next: sbxm task review"), "{}", out.out);
    assert_eq!(stage(&f, &id), (Stage::Gating, Status::GatesFailed));
    assert_eq!(count(&b, "codex"), 0, "no reviewer ran");
    assert_eq!(b.creates().len(), 1, "no reviewer sandbox");
}

struct Yes;

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// The first run's worker fails before committing anything, so its `repo.git` has no commits
/// beyond the base: nothing for decision 178's protection to refuse (see the next test for the
/// case where it does), and `--restart` can discard it and run again right away.
#[test]
fn restart_discards_the_old_task_then_runs() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    assert!(
        out.out
            .contains(&format!("removed sandbox sbxm-task-{id}-claude")),
        "{}",
        out.out
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
}

/// Decision 178: a finished spec task's result lives only in its `repo.git` until fetched, so
/// `--restart` protects it exactly as `task rm --spec` does, since it has no `--force` of its own.
#[test]
fn restart_refuses_to_discard_an_unfetched_finished_result() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &backend_for(&f, &id, &[CLEAN_REVIEW]),
        &FakeGitHub::default(),
    );
    first.result.unwrap();
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(s.clone()),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--force"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task's result was not deleted"
    );
}

/// Issue #194 review finding M-1: `--restart` must not discard the old task before the new
/// start's own inputs are validated, the same as it already doesn't for `--issue`.
#[test]
fn restart_validates_the_new_start_before_discarding_the_old_task() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s.clone());
    opts.time_limit = Some("soon".into());

    let out = go(
        &f,
        &opts,
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--time-limit"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was deleted before the new start's inputs were validated"
    );
}

/// `start --spec` and `review --spec` would both warn about the same-harness reviewer; `run
/// --spec` says it once (the `Once` writer, as for `--issue`).
#[test]
fn the_configs_warnings_are_said_once() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"claude\"\n",
    );
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(&f, &options(&f, s), None, &b, &FakeGitHub::default());

    let _ = out.result;
    assert!(out.warn.contains("warning:"), "{}", out.warn);
    let lines: Vec<&str> = out.warn.lines().collect();
    let mut unique = lines.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(lines.len(), unique.len(), "{}", out.warn);
}

#[test]
fn a_bad_reviewer_flag_is_refused_before_the_worker_starts() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s);
    opts.reviewer_time_limit = Some("soon".into());

    let out = go(&f, &opts, None, &b, &FakeGitHub::default());

    assert!(out.result.is_err());
    assert!(b.creates().is_empty());
}

#[test]
fn an_empty_or_blank_reviewer_model_flag_is_refused_before_the_worker_starts() {
    for bad in ["", "  "] {
        let f = config();
        let s = spec(&f);
        let id = id_of(&s);
        let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
        let mut opts = options(&f, s);
        opts.reviewer_model = Some(bad.into());
        let gh = FakeGitHub::default();

        let out = go(&f, &opts, None, &b, &gh);

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--reviewer-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
    }
}
