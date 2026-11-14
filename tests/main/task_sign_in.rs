//! Issue 445: `sbx`'s sign-in is checked before each task's first sandbox (`task run`, each
//! target of a list), and a lapsed one stops cleanly, naming `sbx login` and the command that goes
//! on. A harness's own sign-in lapsing (a recorded message such as "OAuth session expired") is
//! reported as a sign-in failure with its fix, and the task stays resumable. Driven through
//! `commands::task_run::run_list`, as `tests/task_run_list.rs` does.

use crate::common;

use std::path::Path;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, issue_text, ok, open_issue,
    play_tasks, source,
};
use sbxm::backend::{ExecSpec, FakeBackend};
use sbxm::commands::task_run::{ListOptions, Target, run_list};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::{NotSignedIn, says_not_signed_in};
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo::Identity;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

/// What the real backend's `list` fails with once signed out (the issue's real check, `sbx`
/// v0.46.0, stdin closed).
const SIGNED_OUT: &str = "`sbx ls --json` failed (exit status: 1): error: Not authenticated to \
                          Docker\n  try: sbx login";

/// What every call answered once the sign-in lapsed in #92's run.
const LAPSED: &str = "lookup runtime: request failed: 401 Unauthorized: user is not authenticated \
                      to Docker";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

/// Writes `review` into whichever task's reviewer sandbox is running.
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

fn backend_from(f: &Fixture, b: FakeBackend) -> FakeBackend {
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
    b.and_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn backend(f: &Fixture) -> FakeBackend {
    backend_from(f, FakeBackend::default())
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

fn go(f: &Fixture, targets: &[Target], backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_list(
        &f.env.config_dir(),
        &opts(f),
        targets,
        None,
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

fn exists(f: &Fixture, id: &str) -> bool {
    record::task_dir(&f.env.base_dir(), id).exists()
}

#[test]
fn the_recorded_signed_out_answers_are_recognized_and_others_are_not() {
    assert!(says_not_signed_in(SIGNED_OUT));
    assert!(says_not_signed_in(LAPSED));
    assert!(!says_not_signed_in("fake: daemon not running"));
    assert!(!says_not_signed_in(
        "`sbx ls --json` failed (exit status: 1): connection refused"
    ));
}

/// Boundary case 1 of 7: signed in, the check passes and the task runs as before.
#[test]
fn signed_in_a_task_runs_to_ready() {
    let f = config();
    let (b, gh) = (backend(&f), github());

    let out = go(&f, &[Target::Issue(41)], &b, &gh);

    out.result.unwrap();
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
}

/// Boundary case 2 of 7: signed out before the only target, nothing is created and the stop names
/// `sbx login`, then the same `task run` (no task exists to resume).
#[test]
fn signed_out_before_a_task_stops_before_anything_is_created() {
    let f = config();
    let b = backend_from(&f, FakeBackend::default().failing_list_with(SIGNED_OUT));
    let gh = github();

    let out = go(&f, &[Target::Issue(41)], &b, &gh);

    let e = out.result.unwrap_err();
    assert!(e.is::<NotSignedIn>(), "{e:#}");
    let text = format!("{e:#}");
    assert!(text.starts_with("sbx is not signed in ("), "{text}");
    assert!(text.contains("Not authenticated to Docker"), "{text}");
    assert!(
        text.ends_with("; run `sbx login`, then `sbxm task run --issue 41`"),
        "{text}"
    );
    assert!(b.creates().is_empty());
    assert!(!exists(&f, "issue-41"));
}

/// Boundary case 2 of 7, for a list: the whole list stops at its first target, and the way on
/// names every target.
#[test]
fn signed_out_before_the_first_target_stops_the_whole_list() {
    let f = config();
    let b = backend_from(&f, FakeBackend::default().failing_list_with(SIGNED_OUT));
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42), Target::Issue(43)];

    let out = go(&f, &targets, &b, &gh);

    let e = out.result.unwrap_err();
    assert!(e.is::<NotSignedIn>(), "{e:#}");
    let text = format!("{e:#}");
    assert!(
        text.ends_with("; run `sbx login`, then `sbxm task run --issue 41 --issue 42 --issue 43`"),
        "{text}"
    );
    assert!(b.creates().is_empty());
    for id in ["issue-41", "issue-42", "issue-43"] {
        assert!(!exists(&f, id), "{id}");
    }
    let summary = &out.out;
    assert!(
        summary.contains("--issue 41: not started: sbx is not signed in"),
        "{summary}"
    );
    assert!(
        summary.contains("--issue 42: not started; the list stopped"),
        "{summary}"
    );
    assert!(
        summary.contains("--issue 43: not started; the list stopped"),
        "{summary}"
    );
}

/// Boundary case 3 of 7: the sign-in lapses while the first target runs; the first stays as it is
/// (ready), the second isn't started, and the way on names the targets left.
#[test]
fn a_sign_in_lapsing_between_two_targets_stops_before_the_second() {
    let f = config();
    let b = backend(&f);
    let switch = b.list_failure_switch();
    // After issue 41's last sandbox is made: from then on every `sbx ls` answers signed out.
    let b = b.with_create_hook(move |spec| {
        if spec.name == "sbxm-task-issue-41-review-codex" {
            *switch.lock().unwrap() = Some(LAPSED.to_owned());
        }
    });
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42), Target::Issue(43)];

    let out = go(&f, &targets, &b, &gh);

    let e = out.result.unwrap_err();
    assert!(e.is::<NotSignedIn>(), "{e:#}");
    assert_eq!(stage(&f, "issue-41"), (Stage::Ready, Status::Ok));
    assert!(!exists(&f, "issue-42"));
    assert!(!exists(&f, "issue-43"));
    assert!(
        !b.creates()
            .iter()
            .any(|c| c.name.contains("issue-42") || c.name.contains("issue-43"))
    );
    let text = format!("{e:#}");
    assert!(
        text.ends_with("; run `sbx login`, then `sbxm task run --issue 42 --issue 43`"),
        "{text}"
    );
}

/// Boundary case 4 of 7: `sbx ls` failing for any other reason stays decision 193's "`sbx` not
/// answering", as before.
#[test]
fn sbx_ls_failing_otherwise_is_still_sbx_not_answering() {
    let f = config();
    let b = backend_from(&f, FakeBackend::default().failing_list());
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &targets, &b, &gh);

    let e = out.result.unwrap_err();
    assert!(!e.is::<NotSignedIn>(), "{e:#}");
    assert!(b.creates().is_empty());
    let summary = &out.out;
    assert!(
        summary.contains("--issue 41: not started: fake: daemon not running"),
        "{summary}"
    );
    assert!(
        summary.contains("--issue 42: not started; the list stopped"),
        "{summary}"
    );
}

// `doctor` reports the local sign-in the way it reports the cloud's.

fn doctor_host() -> sbxm::commands::doctor::Host {
    sbxm::commands::doctor::Host {
        free_space: |_| Ok(20 * 1024 * 1024 * 1024),
        writable: |_| Ok(()),
        temp_dirs: Vec::new(),
        sandbox_state_dir: None,
        now: || 1_800_000_000,
        current_dir: None,
    }
}

#[test]
fn doctor_says_signed_in_when_sbx_ls_answers() {
    let env = common::Env::new();

    let report =
        sbxm::commands::doctor::run(&env.config_dir(), &FakeBackend::default(), &doctor_host());

    let text = report.render();
    assert!(
        text.contains("ok   sbx daemon answers, signed in"),
        "{text}"
    );
}

#[test]
fn doctor_fails_signed_out_naming_sbx_login() {
    let env = common::Env::new();
    let b = FakeBackend::default().failing_list_with(SIGNED_OUT);

    let report = sbxm::commands::doctor::run(&env.config_dir(), &b, &doctor_host());

    assert!(report.failed());
    let text = report.render();
    assert!(
        text.contains("FAIL sbx is not signed in (`sbx ls --json` failed"),
        "{text}"
    );
    assert!(text.contains("; run `sbx login`"), "{text}");
    assert!(!text.contains("sbx daemon:"), "{text}");
}

// A harness's own sign-in lapsing is recognized after its run fails (answer 2).

/// Claude's stream for a run that failed with `result`, exiting 1.
fn claude_failed(result: &str) -> sbxm::backend::ExecOutput {
    let event = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": true,
        "result": result,
        "session_id": "s-1",
    });
    sbxm::backend::ExecOutput {
        stdout: format!("{event}\n"),
        stderr: String::new(),
        exit_code: Some(1),
    }
}

const OAUTH_EXPIRED: &str = "OAuth session expired and could not be refreshed";

/// Boundary case 5 of 7: a worker whose output matches a recorded sign-in message stops as a
/// sign-in failure naming the fix, stays resumable, and the list goes no further.
#[test]
fn a_worker_failing_on_a_lapsed_harness_sign_in_is_a_sign_in_stop() {
    let f = config();
    let b = backend_from(
        &f,
        FakeBackend::default().with_exec_output_matching("claude", claude_failed(OAUTH_EXPIRED)),
    );
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &targets, &b, &gh);

    let e = out.result.unwrap_err();
    assert!(
        e.is::<sbxm::task::pipeline::HarnessSignedOut>(),
        "{e:#}\n{}",
        out.out
    );
    let text = &out.out;
    assert!(
        text.contains("note: the worker's claude sign-in has lapsed (claude reported an error: OAuth session expired"),
        "{text}"
    );
    assert!(
        text.contains("sign in again (`sbx setup`, or `sbx secret set anthropic`) and continue with `sbxm task resume --issue 41`"),
        "{text}"
    );
    assert!(
        text.contains(
            "rebuild profile 'default''s template (`sbxm template build default --harness claude`)"
        ),
        "{text}"
    );
    assert!(
        text.contains("--issue 42: not started; the list stopped"),
        "{text}"
    );
    assert!(!exists(&f, "issue-42"));
    // Resumable: the failed worker's task is kept as any failed worker's is.
    assert_eq!(stage(&f, "issue-41"), (Stage::Working, Status::Failed));
}

/// Boundary case 6 of 7: a failure matching no recorded message stays a plain failure, and the
/// list goes on past it as before.
#[test]
fn a_worker_failing_otherwise_is_a_plain_failure() {
    let f = config();
    let b = backend_from(
        &f,
        FakeBackend::default().with_exec_output_matching("claude", claude_failed("disk on fire")),
    );
    let gh = github();
    let targets = [Target::Issue(41), Target::Issue(42)];

    let out = go(&f, &targets, &b, &gh);

    let e = out.result.unwrap_err();
    assert!(!e.is::<sbxm::task::pipeline::HarnessSignedOut>(), "{e:#}");
    let text = &out.out;
    assert!(!text.contains("sign-in"), "{text}");
    assert!(text.contains("task 2 of 2: --issue 42"), "{text}");
    assert!(exists(&f, "issue-42"));
}

/// The reviewer's harness sign-in lapsing is a sign-in stop too, naming the reviewer's fix.
#[test]
fn a_reviewer_failing_on_a_lapsed_harness_sign_in_is_a_sign_in_stop() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"claude\"\n\n[worker]\nharness = \"codex\"\n",
    );
    let b = backend_from(
        &f,
        FakeBackend::default().with_exec_output_matching("claude", claude_failed(OAUTH_EXPIRED)),
    );
    let gh = github();

    let out = go(&f, &[Target::Issue(41)], &b, &gh);

    let e = out.result.unwrap_err();
    assert!(
        e.is::<sbxm::task::pipeline::HarnessSignedOut>(),
        "{e:#}\n{}",
        out.out
    );
    let text = format!("{e:#}");
    assert!(
        text.contains("the reviewer's claude sign-in has lapsed"),
        "{text}"
    );
    assert!(
        text.contains("continue with `sbxm task resume --issue 41`"),
        "{text}"
    );
}
