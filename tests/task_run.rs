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

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
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

/// Issue 140, decision 180(b): a post-worker gate failure doesn't stop `run`; the review reruns
/// the gates and gives the failure to a fix round.
#[test]
fn failing_gates_after_the_worker_go_on_to_the_review_and_its_fix_round() {
    let f = config();
    // `cargo test` fails until a fix round has run, then passes.
    let fixed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let b = backend(&f, &[CLEAN]).with_exec_responder(move |_, spec| {
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

    let out = go(&f, &options(&f), None, &b, &github());

    out.result.unwrap();
    let text = &out.out;
    // The start step's own report stays, without its `next:` line.
    let failed = text
        .find("gates: failed: `cargo test` (sandbox, exit 101)")
        .expect(text);
    assert!(text.contains("gates.log"), "{text}");
    assert!(!text.contains("next: sbxm task review"), "{text}");
    // The gate failure spent round 1 on a fix round; the reviewer ran in round 2.
    let review = text.find("issue-41: review round 2").expect(text);
    assert!(failed < review, "{text}");
    assert!(text.contains("the fix round ran 1 time(s)"), "{text}");
    assert!(text.contains("next: sbxm task finish --issue 41"), "{text}");
    assert_eq!(stage(&f), (Stage::Ready, Status::Ok));
    // The gates ran after the worker, again in the review, and once more after the fix round.
    let gate_runs = b
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains("cargo test")))
        .count();
    assert_eq!(gate_runs, 3, "{text}");
    // The fix round got the gate's own output.
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    let fix_prompt = std::fs::read_to_string(meta.join("fix-prompt.md")).unwrap();
    assert!(fix_prompt.contains("gate-failure.md"), "{fix_prompt}");
    let gate_output = std::fs::read_to_string(
        f.env
            .base_dir()
            .join("tasks")
            .join("issue-41")
            .join(".sbxm-task")
            .join("gate-failure.md"),
    )
    .unwrap();
    assert!(gate_output.contains("oops"), "{gate_output}");
}

/// Issue 186: unlike a gate that ran and failed, one that could not run at all gives the review
/// nothing to react to, so `task run` stops after the start phase instead of going on to it.
#[test]
fn a_sandbox_gate_that_cannot_run_stops_before_any_reviewer() {
    let f = config();
    let b = backend(&f, &[CLEAN]).with_failing_exec_matching("cargo test");

    let out = go(&f, &options(&f), None, &b, &github());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("1 of 1"), "{message}");
    // The start step's own report says the worker ran and the gate could not run.
    assert!(
        out.out.contains("issue-41: worker completed"),
        "{}",
        out.out
    );
    assert!(out.out.contains("gates: could not run"), "{}", out.out);
    assert!(!out.out.contains("next: sbxm task review"), "{}", out.out);
    assert_eq!(stage(&f), (Stage::Gating, Status::GatesFailed));
    assert_eq!(count(&b, "codex"), 0, "no reviewer ran");
    assert_eq!(b.creates().len(), 1, "no reviewer sandbox");
}

/// Issue 186, the host tier: a gate command the host couldn't even start behaves the same as one
/// the sandbox couldn't run. `task start`'s own gate run always uses the real shell (it has no
/// `HostRunner` parameter to fake), so `nope` there is "command not found" (exit 127: the shell
/// did run) and goes on to the review as today (decision 180); the fake `HostRunner` only reaches
/// the review's own gate run, through `task run`'s `host` parameter, which is where issue 186's
/// distinction is visible: it stops the review without a fix round.
#[test]
fn a_host_gate_that_cannot_start_stops_before_any_reviewer() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\nhost = [\"nope\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    );
    let b = backend(&f, &[CLEAN]);
    let host = FakeHostRunner::default().with_start_error("nope", "no such shell");

    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        &options(&f),
        None,
        &b,
        &github(),
        &Probe,
        &host,
        &mut out,
        &mut warn,
    );
    let out = String::from_utf8(out).unwrap();

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("could not run"), "{message}");
    assert!(message.contains("no such shell"), "{message}");
    assert!(out.contains("gates could not run"), "{out}");
    assert!(!out.contains("next: sbxm task review"), "{out}");
    assert_eq!(count(&b, "codex"), 0, "no reviewer ran");
}

#[test]
fn a_failed_worker_stops_before_any_reviewer_and_fails_the_command() {
    let f = config();
    let b = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching(
            "claude",
            sbxm::backend::ExecOutput {
                stdout: String::new(),
                stderr: "boom".into(),
                exit_code: Some(1),
            },
        )
        .with_exec_output_matching("codex", ok(CODEX_DONE));

    let out = go(&f, &options(&f), None, &b, &github());

    // The error is the start step's own: the review, which would refuse this task too, is not asked.
    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("1 of 1 task(s) failed"), "{message}");
    assert_eq!(count(&b, "claude"), 1, "the worker ran");
    assert!(out.out.contains("issue-41: worker failed"), "{}", out.out);
    assert!(!out.out.contains("review round"), "{}", out.out);
    assert_eq!(b.creates().len(), 1, "no reviewer sandbox");
    assert_eq!(count(&b, "codex"), 0, "no reviewer");
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

/// A `Confirm` that removes the worker's secret from the backend right when the user says yes,
/// modeling it vanishing while the confirmation prompt is pending (decision 185, issue #207).
struct SecretVanishesThenYes<'a> {
    backend: &'a FakeBackend,
}

impl Confirm for SecretVanishesThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        self.backend.set_secrets(&["openai"]);
        Ok(true)
    }
}

/// Decision 185 (issue #207): `task run --restart --issue` puts the old task back, instead of
/// losing it, when the worker's secret vanishes while the confirmation prompt is pending.
#[test]
fn restart_puts_the_old_task_back_if_the_worker_secret_vanishes_during_confirmation() {
    let f = config();
    let first = go(&f, &options(&f), None, &backend(&f, &[CLEAN]), &github());
    first.result.unwrap();
    let b = backend(&f, &[CLEAN]);
    let confirm = SecretVanishesThenYes { backend: &b };

    let out = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    assert!(out.result.is_err(), "{}", out.out);
    assert!(out.out.contains("anthropic"), "{}", out.out);
    assert!(out.out.contains("put back"), "{}", out.out);
    assert!(
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).is_ok(),
        "the old task is back"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "nothing left set aside"
    );
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
