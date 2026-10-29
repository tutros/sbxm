//! Issue 201, decision 193: `sbxm task run` over a list of `--issue`/`--spec` targets, run in
//! order. The CLI's own wiring (clap accepting both flags together, and the written order
//! surviving `main`'s dispatch) is `tests/task_run_cli.rs`; this is the orchestrator itself
//! (`commands::task_run::run_list`), called directly as `tests/task_run.rs` calls `run`.

mod common;

use std::path::Path;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, issue_text, ok, open_issue,
    play_tasks, source,
};
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend};
use sbxm::commands::task_run::{ListOptions, Target, run_list};
use sbxm::commands::task_start::Restart;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo::Identity;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

/// Writes `review` into whichever task's reviewer sandbox is running (any `issue-<n>-review-
/// <harness>` sandbox), unlike `task_fixture::play_reviews`, which is pinned to one task id.
fn play_any_review(
    base_dir: &Path,
    review: &'static str,
) -> impl Fn(&str, &ExecSpec) + Send + Sync + use<> {
    let base_dir = base_dir.to_path_buf();
    move |sandbox: &str, spec: &ExecSpec| {
        let headless = spec.argv.iter().any(|a| a == "codex" || a == "claude");
        let Some(rest) = sandbox.strip_prefix("sbxm-task-") else {
            return;
        };
        let Some((id, _harness)) = rest.split_once("-review-") else {
            return;
        };
        if !headless {
            return;
        }
        let dir = base_dir
            .join("tasks")
            .join(format!("{id}-review"))
            .join(".sbxm-task");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("review.md"), review).unwrap();
    }
}

fn backend(f: &Fixture) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_any_review(&f.env.base_dir(), CLEAN);
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
        .with_issue_text(issue_text(42))
        .with_issue_text(issue_text(43))
        .with_open_issues(vec![
            open_issue(41, &["should-fix"], ""),
            open_issue(42, &["should-fix"], ""),
            open_issue(43, &["should-fix"], ""),
        ])
}

/// Like [`github`], with `extra` open issues added for a test of one of `select::select`'s own
/// rules (issue 201 review M-1): the base set (41, 42, 43) stays the same so `github_with` can
/// stand in for `github` in every list test, just with one more candidate.
fn github_with(extra: Vec<sbxm::github::Issue>) -> FakeGitHub {
    let mut open = vec![
        open_issue(41, &["should-fix"], ""),
        open_issue(42, &["should-fix"], ""),
        open_issue(43, &["should-fix"], ""),
    ];
    open.extend(extra);
    github().with_open_issues(open)
}

fn opts(f: &Fixture) -> ListOptions {
    ListOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
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
        disk: Default::default(),
        location: None,
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
}

fn go(
    f: &Fixture,
    opts: &ListOptions,
    targets: &[Target],
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_list(
        &f.env.config_dir(),
        opts,
        targets,
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
    }
}

fn stage(f: &Fixture, id: &str) -> (Stage, Status) {
    let task = record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap();
    (task.stage, task.status)
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
fn two_issues_run_in_order_and_both_reach_ready() {
    let f = config();
    let (b, gh) = (backend(&f), github());
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    out.result.unwrap();
    let text = &out.out;
    let first = text.find("task 1 of 2: --issue 41").expect(text);
    let second = text.find("task 2 of 2: --issue 42").expect(text);
    assert!(first < second, "{text}");
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert_eq!(stage(&f, "issue-42"), (Stage::Ready, Status::Ok));
    assert!(text.contains("summary:"), "{text}");
    assert!(
        text.contains("--issue 41: stage ready, must-fix 0; next: sbxm task finish --issue 41"),
        "{text}"
    );
    assert!(
        text.contains("--issue 42: stage ready, must-fix 0; next: sbxm task finish --issue 42"),
        "{text}"
    );
}

/// Decision 193, point 1: a task that stops short of `ready` is recorded, and the list goes on to
/// the next one instead of stopping. The agent's own command exiting non-zero is `Ok` with a
/// failed status (`SandboxBackend::exec` itself still ran): an ordinary per-task stop, unlike
/// `sbx` itself failing to run a command (issue 201 review M-2, repeat of M-2; see
/// `sbx_exec_failing_for_the_worker_stops_the_list` below).
#[test]
fn a_failed_task_does_not_stop_the_list() {
    let f = config();
    let b = backend(&f).with_exec_responder(|sandbox, spec| {
        (sandbox == "sbxm-task-issue-41-claude" && spec.argv.iter().any(|a| a == "claude")).then(
            || ExecOutput {
                stdout: String::new(),
                stderr: "boom".into(),
                exit_code: Some(1),
            },
        )
    });
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("1 of 2 task(s) did not reach ready"),
        "{message}"
    );
    let text = &out.out;
    assert!(
        text.contains("issue-41: worker claude (default model) failed:"),
        "{text}"
    );
    assert_eq!(stage(&f, "issue-42"), (Stage::Ready, Status::Ok));
    assert!(
        text.contains("--issue 42: stage ready, must-fix 0; next: sbxm task finish --issue 42"),
        "{text}"
    );
}

/// Decision 193(a), issue 201 review M-2 (repeat of M-2): `SandboxBackend::exec` itself failing —
/// "`sbx` itself couldn't run" per its own doc comment, never a failing agent command, which comes
/// back `Ok` with a status — is one of the list's queue-wide stops, not a per-task failure the
/// list goes on past. The previous fix only wired this up for a missing secret or `sbx` not
/// answering `secret_services`; an `exec` failure while the worker itself ran still let the list
/// continue, which this test would have caught.
#[test]
fn sbx_exec_failing_for_the_worker_stops_the_list() {
    let f = config();
    let b = backend(&f).with_failing_exec_for("sbxm-task-issue-41-claude");
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    out.result.unwrap_err();
    assert!(!record::task_dir(&f.env.base_dir(), "issue-42").exists());
    let text = &out.out;
    assert!(
        text.contains("--issue 42: not started; the list stopped"),
        "{text}"
    );
}

/// Decision 193, point 6: restarting more than one task at once would discard several tasks
/// before the user sees any of them settle, so it's refused before anything happens.
#[test]
fn restart_is_refused_with_more_than_one_target() {
    let f = config();
    let (b, gh) = (backend(&f), github());
    let targets = [Target::Issue(41), Target::Issue(42)];
    let restart = Restart {
        confirm: &Yes,
        yes: false,
    };

    let out = go(&f, &opts(&f), &targets, Some(&restart), &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("--restart isn't allowed with more than one task"),
        "{message}"
    );
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
}

/// Decision 193, point 2: the disk is checked again before every task, not just the first, and a
/// failure stops the rest of the list while keeping what the earlier tasks already finished.
#[test]
fn a_low_disk_before_a_later_task_stops_the_list_and_keeps_the_earlier_result() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static READS: AtomicUsize = AtomicUsize::new(0);
    let f = config();
    let (b, gh) = (backend(&f), github());
    let mut options = opts(&f);
    let state = f.env.tmp.path().join("sbx-state");
    std::fs::create_dir_all(&state).unwrap();
    options.disk = sbxm::disk::Guard {
        free_space: |_| {
            let gib = 1024 * 1024 * 1024;
            // Room for issue 41's whole run (its own 3 checks plus this list's own check before
            // it); none left once issue 42's turn comes.
            Ok(if READS.fetch_add(1, Ordering::SeqCst) < 4 {
                20 * gib
            } else {
                gib
            })
        },
        default_state_dir: Some(state),
        ignore: false,
    };
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &options, &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("1 of 2 task(s) did not reach ready"),
        "{message}"
    );
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert!(!record::task_dir(&f.env.base_dir(), "issue-42").exists());
    let text = &out.out;
    assert!(
        text.contains("--issue 42: not started: only 1.0 GiB free on"),
        "{text}"
    );
}

/// Issue 201 review M-1: a later target's own problem (here, an issue that doesn't exist) must
/// surface before an earlier, valid target's sandbox is ever created, not only once the loop
/// reaches the bad target's own turn.
#[test]
fn a_later_invalid_issue_is_refused_before_any_sandbox_is_created() {
    let f = config();
    let (b, gh) = (backend(&f), github());
    let targets = [Target::Issue(41), Target::Issue(999)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--issue 999"), "{message}");
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Issue 201 review M-1: same as above, for a later `--spec` target naming a file that doesn't
/// exist.
#[test]
fn a_later_invalid_spec_is_refused_before_any_sandbox_is_created() {
    let f = config();
    let (b, gh) = (backend(&f), github());
    let missing = f.env.tmp.path().join("missing.md");
    let targets = [Target::Issue(41), Target::Spec(missing.clone())];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Issue 201 review M-1 (repeat of M-1): the preflight pass must apply the same rules an explicit
/// `task start --issue A --issue B` already does (`select::select`), not only `issue_startable`'s
/// own open/existing-task/continued-PR checks — a later target labeled `question` passed the old
/// preflight and only its own turn in the loop found it, after an earlier valid target had already
/// run its worker and reviewer sandboxes.
#[test]
fn a_later_question_issue_is_refused_before_any_sandbox_is_created() {
    let f = config();
    let (b, gh) = (
        backend(&f),
        github_with(vec![open_issue(44, &["question"], "")]),
    );
    let targets = [Target::Issue(41), Target::Issue(44)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--issue 44"), "{message}");
    assert!(message.contains("question"), "{message}");
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Issue 201 review M-1 (repeat of M-1): same kind as above, for a later target blocked by an open
/// dependency (`**Depends on:**`).
#[test]
fn a_later_blocked_issue_is_refused_before_any_sandbox_is_created() {
    let f = config();
    let (b, gh) = (
        backend(&f),
        github_with(vec![open_issue(44, &["should-fix"], "**Depends on:** #43")]),
    );
    let targets = [Target::Issue(41), Target::Issue(44)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--issue 44"), "{message}");
    assert!(message.contains("blocked"), "{message}");
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Issue 201 review M-1 (repeat of M-1): same kind as above, for a later target related
/// (`**Related:**`) to an earlier target in the same batch — the batch's own picks, not only
/// existing tasks on disk, must be in the snapshot `select::select` checks clashes against.
#[test]
fn a_target_related_to_an_earlier_target_is_refused_before_any_sandbox_is_created() {
    let f = config();
    let (b, gh) = (
        backend(&f),
        github_with(vec![open_issue(44, &["should-fix"], "**Related:** #41")]),
    );
    let targets = [Target::Issue(41), Target::Issue(44)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--issue 44"), "{message}");
    assert!(message.contains("related to"), "{message}");
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Decision 193(a), issue 201 review M-2: a missing shared secret is one of the three problems
/// that stop the whole list, instead of being queried and refused again for every later target.
#[test]
fn a_missing_shared_secret_stops_the_list_once() {
    let f = config();
    let b = FakeBackend::default();
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    out.result.unwrap_err();
    let text = &out.out;
    assert!(text.contains("is not stored in sbx"), "{text}");
    assert!(
        text.contains("--issue 42: not started; the list stopped"),
        "{text}"
    );
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Decision 193(a), issue 201 review M-2: `sbx` becoming unreachable partway through the list
/// stops it too, keeping the result of the task that already finished and never starting the
/// one after the one that discovered it.
#[test]
fn sbx_becoming_unreachable_after_an_earlier_task_stops_the_list_and_keeps_its_result() {
    let f = config();
    let b = backend(&f);
    let fail = b.secret_services_failure();
    // After issue 41's own review has already checked its secret and created its sandbox, so
    // issue 41 still reaches `ready`; only issue 42's turn finds `sbx` unreachable.
    let b = b.with_create_hook(move |spec| {
        if spec.name == "sbxm-task-issue-41-review-codex" {
            fail.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42), Target::Issue(43)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    out.result.unwrap_err();
    let text = &out.out;
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert!(
        text.contains("--issue 41: stage ready, must-fix 0; next: sbxm task finish --issue 41"),
        "{text}"
    );
    assert!(text.contains("--issue 42: not started:"), "{text}");
    assert!(
        text.contains("--issue 43: not started; the list stopped"),
        "{text}"
    );
    assert!(!record::task_dir(&f.env.base_dir(), "issue-43").exists());
}

/// Issue 201 review S-1: the same issue listed twice in one `task run` can't be caught by the
/// preflight above (neither copy has a task yet when it runs, before either target starts), so
/// the second copy still discovers the conflict the old way, once the first has already made the
/// task: issue selection now skips #41 as already taken, and that refusal must be what the
/// summary shows for the second copy, not the first copy's `ready` stage borrowed from the
/// now-existing record.
#[test]
fn a_target_repeated_in_the_same_list_reports_the_refusal_not_the_other_copys_stage() {
    let f = config();
    let (b, gh) = (backend(&f), github());
    let targets = [Target::Issue(41), Target::Issue(41)];

    let out = go(&f, &opts(&f), &targets, None, &b, &gh);

    let text = &out.out;
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert!(
        text.contains("--issue 41: stage ready, must-fix 0; next: sbxm task finish --issue 41"),
        "{text}"
    );
    assert!(text.contains("already has a task"), "{text}");
    let ready_lines = text
        .matches("stage ready, must-fix 0; next: sbxm task finish --issue 41")
        .count();
    assert_eq!(
        ready_lines, 1,
        "two lines for the same target, but only one of them reached ready this run: {text}"
    );
}

/// P14 (decision 262(k)): `--local` overrides a cloud `[sandbox] location` for every issue target
/// of the list, each start included: both run here to `ready`, with no call to the cloud.
#[test]
fn local_overrides_a_cloud_config_for_every_issue_target() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    );
    let (b, gh) = (backend(&f), github());
    let targets = [Target::Issue(41), Target::Issue(42)];
    let opts = ListOptions {
        location: Some(sbxm::backend::Location::Local),
        ..opts(&f)
    };

    let out = go(&f, &opts, &targets, None, &b, &gh);

    out.result.expect(&out.out);
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert_eq!(stage(&f, "issue-42"), (Stage::Ready, Status::Ok));
    assert!(
        !b.locations().contains(&sbxm::backend::Location::Cloud),
        "{:?}",
        b.locations()
    );
}
