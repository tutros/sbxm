//! M2b slice 10, step 8: `sbxm task run --issue N` is `start` then `review`, and stops before
//! `finish` (spec §4, decision 158).

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, issue_text, ok, open_issue,
    play_reviews, play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_run::{Options, run};
use sbxm::commands::task_start::Restart;
use sbxm::confirm::Confirm;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo::Identity;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
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

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
}

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issue: 41,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
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
    opts: &Options,
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
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

fn stage(f: &Fixture) -> (Stage, Status) {
    let task =
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap();
    (task.stage, task.status)
}

#[test]
fn it_starts_then_reviews_and_stops_before_finish() {
    let f = config();
    let (b, gh) = (backend(&f, &[CLEAN]), github());

    let out = go(&f, &options(&f), None, &b, &gh);

    out.result.unwrap();
    let text = &out.out;
    let worker = text.find("issue-41: worker completed").expect(text);
    let review = text.find("issue-41: review round 1").expect(text);
    assert!(worker < review, "{text}");
    assert!(text.contains("issue-41: ready"), "{text}");
    // Only the last step's hint is left, and it is the finish command.
    assert!(!text.contains("sbxm task review --issue 41"), "{text}");
    assert!(text.contains("next: sbxm task finish --issue 41"), "{text}");
    assert_eq!(stage(&f), (Stage::Ready, Status::Ok));
    // Nothing was published.
    assert!(!gh.calls().iter().any(|c| matches!(c, GhCall::PrCreate(..))));
    let pushed = std::process::Command::new("git")
        .current_dir(&f.origin)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/issue-41"])
        .status()
        .unwrap();
    assert!(!pushed.success(), "run must not push");
}

#[test]
fn findings_left_after_the_fix_round_are_reported_and_it_still_succeeds() {
    let f = config();
    let (b, gh) = (backend(&f, &[ONE, ONE]), github());

    let out = go(&f, &options(&f), None, &b, &gh);

    out.result.unwrap();
    assert!(
        out.out.contains("1 must-fix finding(s) left"),
        "{}",
        out.out
    );
    assert_eq!(stage(&f), (Stage::Ready, Status::Ok));
}

#[test]
fn failing_gates_after_the_worker_stop_before_any_reviewer_and_fail_the_command() {
    let f = config();
    let red = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
        .with_exec_output_matching(
            "cargo test",
            ExecOutput {
                stdout: String::new(),
                stderr: "red\n".into(),
                exit_code: Some(101),
            },
        );

    let out = go(&f, &options(&f), None, &red, &github());

    assert!(out.result.is_err());
    assert!(out.out.contains("gates: failed"), "{}", out.out);
    assert!(!out.out.contains("review round"), "{}", out.out);
    assert_eq!(red.creates().len(), 1, "no reviewer sandbox");
}

#[test]
fn a_bad_reviewer_flag_is_refused_before_the_worker_starts() {
    let f = config();
    let b = backend(&f, &[CLEAN]);
    let mut opts = options(&f);
    opts.reviewer_time_limit = Some("soon".into());

    let out = go(&f, &opts, None, &b, &github());

    assert!(out.result.is_err());
    assert!(b.creates().is_empty());
}

#[test]
fn an_empty_or_blank_reviewer_model_flag_is_refused_before_the_worker_starts() {
    for bad in ["", "  "] {
        let f = config();
        let b = backend(&f, &[CLEAN]);
        let mut opts = options(&f);
        opts.reviewer_model = Some(bad.into());
        let gh = github();

        let out = go(&f, &opts, None, &b, &gh);

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--reviewer-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
    }
}

#[test]
fn an_empty_or_blank_worker_model_flag_is_refused_before_anything_happens() {
    for bad in ["", "  "] {
        let f = config();
        let b = backend(&f, &[CLEAN]);
        let mut opts = options(&f);
        opts.worker_model = Some(bad.into());
        let gh = github();

        let out = go(&f, &opts, None, &b, &gh);

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--worker-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
    }
}

#[test]
fn an_empty_or_blank_reviewer_model_flag_with_restart_leaves_the_existing_task_in_place() {
    for bad in ["", "  "] {
        let f = config();
        let first = go(&f, &options(&f), None, &backend(&f, &[CLEAN]), &github());
        first.result.unwrap();
        let b = backend(&f, &[CLEAN]);
        let gh = github();
        let mut opts = options(&f);
        opts.reviewer_model = Some(bad.into());

        let out = go(
            &f,
            &opts,
            Some(&Restart {
                confirm: &Yes,
                yes: false,
            }),
            &b,
            &gh,
        );

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--reviewer-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert!(b.removes().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
        assert_eq!(stage(&f), (Stage::Ready, Status::Ok), "{bad:?}");
    }
}

#[test]
fn an_empty_or_blank_worker_model_flag_with_restart_leaves_the_existing_task_in_place() {
    for bad in ["", "  "] {
        let f = config();
        let first = go(&f, &options(&f), None, &backend(&f, &[CLEAN]), &github());
        first.result.unwrap();
        let b = backend(&f, &[CLEAN]);
        let gh = github();
        let mut opts = options(&f);
        opts.worker_model = Some(bad.into());

        let out = go(
            &f,
            &opts,
            Some(&Restart {
                confirm: &Yes,
                yes: false,
            }),
            &b,
            &gh,
        );

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--worker-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert!(b.removes().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
        assert_eq!(stage(&f), (Stage::Ready, Status::Ok), "{bad:?}");
    }
}

#[test]
fn the_configs_warnings_are_said_once() {
    // Worker and reviewer both claude: both start and review would warn about it.
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"claude\"\n",
    );
    let b = FakeBackend::with_secrets(&["anthropic"])
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
            let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
            move |sandbox: &str, spec: &sbxm::backend::ExecSpec| {
                worker(sandbox, spec);
                reviewer(sandbox, spec);
            }
        });

    let out = go(&f, &options(&f), None, &b, &github());

    let _ = out.result;
    assert!(out.warn.contains("warning:"), "{}", out.warn);
    let lines: Vec<&str> = out.warn.lines().collect();
    let mut unique = lines.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(lines.len(), unique.len(), "{}", out.warn);
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

#[test]
fn restart_discards_the_old_task_then_runs() {
    let f = config();
    let first = go(&f, &options(&f), None, &backend(&f, &[CLEAN]), &github());
    first.result.unwrap();
    let b = backend(&f, &[CLEAN]);

    let out = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &github(),
    );

    out.result.unwrap();
    assert!(
        out.out
            .contains("removed sandbox sbxm-task-issue-41-claude"),
        "{}",
        out.out
    );
    assert_eq!(stage(&f), (Stage::Ready, Status::Ok));
}

#[test]
fn a_missing_reviewer_secret_is_refused_before_the_worker_creates_anything() {
    let f = config(); // the reviewer is codex, which needs the openai secret
    let b = FakeBackend::with_secrets(&["anthropic"]);

    let out = go(&f, &options(&f), None, &b, &github());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("openai"), "{message}");
    assert!(b.creates().is_empty(), "no sandbox may be created");
    assert!(
        !f.env
            .base_dir()
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .exists(),
        "no task folder may be left"
    );
}
