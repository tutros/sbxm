//! Issue 599: every command that creates a cloud sandbox takes one cloud slot (`cloud_slots`)
//! after its input checks and holds it until it ends; it never takes a sandbox slot as well, and a
//! local command still takes a sandbox slot, never a cloud one. Through `FakeBackend`.

mod common;

use std::path::PathBuf;
use std::sync::Mutex;

use common::cloud_slot;
use common::cloud_task::{
    CLEAN, cloud_backend, cloud_backend_for, cloud_backend_observing, cloud_backend_worker,
    identity, run_options, spec, task_record, task_run,
};
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Probe, fixture_with, is_headless, issue_text, ok, open_issue,
};
use sbxm::backend::{ExecOutput, FakeBackend, Location};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, ProcessProbe, Stage, Status};

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const CLOUD_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n";

/// The process that ran a task is gone, as it is for every resume.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn cloud_fs(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("cloud-fs")
}

struct Ran {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn ran(result: anyhow::Result<()>, out: Vec<u8>, warn: Vec<u8>) -> Ran {
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

/// `task run --spec FILE --cloud` with `disk`.
fn run_cloud(f: &Fixture, spec: PathBuf, disk: sbxm::disk::Guard, backend: &FakeBackend) -> Ran {
    let mut opts = run_options(f, spec, Some(Location::Cloud));
    opts.disk = disk;
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &opts,
        None,
        backend,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    ran(result, out, warn)
}

fn assert_refused(ran: &Ran) {
    let err = format!("{:#}", ran.result.as_ref().unwrap_err());
    assert!(err.starts_with(&cloud_slot::refusal()), "{err}");
}

fn assert_holder_kept(f: &Fixture) {
    assert_eq!(
        cloud_slot::holder(&f.env).unwrap().process.pid,
        common::slot::HOLDER_PID
    );
    assert!(cloud_slot::queue(&f.env).is_empty());
}

// ---- task run --cloud ----

#[test]
fn a_cloud_task_run_with_no_free_cloud_slot_and_no_wait_refuses_creating_nothing() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);

    let ran = run_cloud(&f, spec, cloud_slot::full(&f.env, true), &b);

    assert_refused(&ran);
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
    assert_holder_kept(&f);
}

/// One cloud slot through the worker and the review (the reviewer takes no second), and no
/// sandbox slot though `sandbox_slots = 1` is held by another command: the pools are separate.
#[test]
fn a_cloud_task_run_holds_one_cloud_slot_through_worker_and_review_and_no_sandbox_slot() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    // The sandbox slot is held by another command: a take of it would refuse at once.
    let _ = common::slot::full(&f.env, true);
    let mut disk = cloud_slot::free(&f.env);
    disk.slot.no_wait = true;
    let slot = cloud_slot::cloud_slot_file(&f.env);
    // The token of the cloud slot file the first headless run found: one take for the command.
    let first = Mutex::new(None::<String>);
    let runs = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = std::sync::Arc::clone(&runs);
    let b = cloud_backend_observing(
        &cloud_fs(&f),
        &[&id],
        &[CLEAN],
        true,
        ok(CLAUDE_DONE),
        move |sandbox, _, spec| {
            if !is_headless(spec) {
                return;
            }
            let held: sbxm::slot::Holder =
                serde_json::from_str(&std::fs::read_to_string(&slot).unwrap()).unwrap();
            assert_eq!(held.process.pid, std::process::id(), "{sandbox}");
            let mut first = first.lock().unwrap();
            assert_eq!(
                first.get_or_insert(held.token.clone()),
                &held.token,
                "{sandbox}"
            );
            seen.lock().unwrap().push(sandbox.to_owned());
        },
    );

    let ran = run_cloud(&f, spec, disk, &b);

    ran.result.expect(&ran.out);
    let runs = runs.lock().unwrap();
    assert!(runs.iter().any(|s| !s.contains("-review-")), "{runs:?}");
    assert!(runs.iter().any(|s| s.contains("-review-")), "{runs:?}");
    assert!(!ran.warn.contains("waiting for a"), "{}", ran.warn);
    assert!(
        cloud_slot::holder(&f.env).is_none(),
        "given back at the end"
    );
    assert!(cloud_slot::queue(&f.env).is_empty());
    assert_eq!(
        common::slot::holder(&f.env).unwrap().process.pid,
        common::slot::HOLDER_PID,
        "the sandbox slot was never touched"
    );
}

#[test]
fn a_cloud_task_run_with_no_free_cloud_slot_waits_naming_the_holder_then_runs() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);

    let ran = run_cloud(&f, spec, cloud_slot::full_until_first_wait(&f.env), &b);

    ran.result.expect(&ran.out);
    assert_eq!(
        ran.warn
            .matches(&format!(
                "waiting for a cloud slot: held by {}\n",
                cloud_slot::HOLDER
            ))
            .count(),
        1,
        "{}",
        ran.warn
    );
    assert!(!b.creates().is_empty());
    assert!(cloud_slot::holder(&f.env).is_none());
}

#[test]
fn a_cloud_task_run_with_cloud_slots_unset_writes_no_slot_file() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
    let disk = sbxm::disk::Guard {
        slot: common::slot::gate(&f.env, true),
        ..Default::default()
    };

    let ran = run_cloud(&f, spec, disk, &b);

    ran.result.expect(&ran.out);
    assert!(!f.env.tmp.path().join("local-data").exists());
}

// ---- task start --cloud ----

#[test]
fn a_cloud_task_start_with_no_free_cloud_slot_and_no_wait_refuses_creating_nothing() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
    let opts = sbxm::commands::task_start::SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(&f)),
        identity: Some(identity()),
        disk: cloud_slot::full(&f.env, true),
        location: Some(Location::Cloud),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_start::run_spec(
        &f.env.config_dir(),
        &opts,
        &b,
        &FakeGitHub::default(),
        &Probe,
        &mut out,
        &mut warn,
    );

    assert_refused(&ran(result, out, warn));
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
    assert_holder_kept(&f);
}

// ---- task resume ----

fn resume(f: &Fixture, spec: PathBuf, disk: sbxm::disk::Guard, backend: &FakeBackend) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec),
            rounds: None,
            should_fix: false,
            disk,
            identity: Some(identity()),
            forget_ambiguous: false,
        },
        backend,
        &FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    ran(result, out, warn)
}

/// A cloud spec task whose worker failed, its sandbox removed.
fn failed_cloud_task(f: &Fixture) -> (PathBuf, String) {
    let (spec, id) = spec(f);
    let failed = ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    };
    let backend = cloud_backend_worker(&cloud_fs(f), &[&id], &[CLEAN], true, failed);
    let ran = task_run(f, spec.clone(), Some(Location::Cloud), &backend);
    assert!(ran.result.is_err(), "{}", ran.out);
    (spec, id)
}

#[test]
fn a_cloud_task_resume_with_no_free_cloud_slot_and_no_wait_refuses_before_resuming() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = failed_cloud_task(&f);
    let task_json = record::task_dir(&f.env.base_dir(), &id).join("task.json");
    let before = std::fs::read(&task_json).unwrap();
    let b = common::cloud_task::cloud_backend_listing(&cloud_fs(&f), &id, &[CLEAN], false);

    let ran = resume(&f, spec, cloud_slot::full(&f.env, true), &b);

    assert_refused(&ran);
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert_eq!(std::fs::read(&task_json).unwrap(), before);
    assert_holder_kept(&f);
}

#[test]
fn a_cloud_task_resume_holds_a_cloud_slot_and_gives_it_back() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = failed_cloud_task(&f);
    let b = common::cloud_task::cloud_backend_listing(&cloud_fs(&f), &id, &[CLEAN], false);

    let ran = resume(&f, spec, cloud_slot::full_until_first_wait(&f.env), &b);

    ran.result.expect(&ran.out);
    // The resume takes it once its checks have passed, and says so on its progress lines.
    assert!(
        ran.out.contains("waiting for a cloud slot: held by"),
        "{}",
        ran.out
    );
    assert_eq!(task_record(&f, &id).stage, Stage::Ready);
    assert!(cloud_slot::holder(&f.env).is_none());
}

/// Unchanged: `task resume` of a local task takes a sandbox slot, not a cloud one, so the local
/// pool's refusal is what it meets with both pools full.
#[test]
fn a_local_task_resume_takes_a_sandbox_slot_not_a_cloud_one() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = common::task_fixture::ready_spec_task(&f);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    task.stage = Stage::Working;
    task.status = Status::Failed;
    record::write(&meta, &task).unwrap();
    let _ = cloud_slot::full(&f.env, true);
    let disk = common::slot::full(&f.env, true);
    let b = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let ran = resume(&f, spec, disk, &b);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.starts_with(&common::slot::refusal()), "{err}");
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert_holder_kept(&f);
}

// ---- task review ----

#[test]
fn a_cloud_task_review_with_no_free_cloud_slot_and_no_wait_refuses_creating_nothing() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let first = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
    let done = task_run(&f, spec.clone(), Some(Location::Cloud), &first);
    done.result.expect(&done.out);
    // A review that failed: tried again, so every check passes and the cloud slot is all that
    // is left.
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    task.stage = Stage::Reviewing;
    task.status = Status::Failed;
    record::write(&meta, &task).unwrap();
    let task_json = std::fs::read(meta.join("task.json")).unwrap();
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
    let opts = sbxm::commands::task_review::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: sbxm::commands::task_review::Target::Spec(spec),
        repo: Some("o/r".into()),
        base: None,
        clone_source: Some(common::task_fixture::source(&f)),
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force: true,
        disk: cloud_slot::full(&f.env, true),
        location: None,
        identity: Some(identity()),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_review::run(
        &f.env.config_dir(),
        &opts,
        &b,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    assert_refused(&ran(result, out, warn));
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert_eq!(std::fs::read(meta.join("task.json")).unwrap(), task_json);
    assert_holder_kept(&f);
}

// ---- task gates ----

/// A cloud issue task after `task start`, its worker sandbox kept.
fn started_cloud_issue(f: &Fixture) {
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")]);
    let opts = sbxm::commands::task_start::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk: Default::default(),
        location: Some(Location::Cloud),
    };
    let backend = cloud_backend_for(&cloud_fs(f), &["issue-41"], &[CLEAN], true);
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_start::run_with(
        &f.env.config_dir(),
        &opts,
        None,
        &backend,
        &github,
        &Probe,
        &mut out,
        &mut warn,
    );
    result.unwrap_or_else(|e| panic!("{e:#}: {}", String::from_utf8_lossy(&out)));
}

fn gates(f: &Fixture, disk: sbxm::disk::Guard, backend: &FakeBackend) -> anyhow::Result<()> {
    sbxm::commands::task_gates::run(
        &f.env.config_dir(),
        &sbxm::commands::task_gates::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issue: 41,
            tiers: sbxm::task::pipeline::Tiers::ALL,
            dry_run: false,
            identity: Some(identity()),
            disk,
        },
        backend,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
    )
}

#[test]
fn cloud_gates_that_make_the_worker_sandbox_again_refuse_with_no_free_cloud_slot_and_no_wait() {
    let f = fixture_with(LOCAL_TOML);
    started_cloud_issue(&f);
    // The worker sandbox is gone: the gates would make it again.
    let gone = cloud_backend_for(&cloud_fs(&f), &[], &[CLEAN], false);

    let result = gates(&f, cloud_slot::full(&f.env, true), &gone);

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.starts_with(&cloud_slot::refusal()), "{err}");
    assert!(gone.creates().is_empty(), "{:?}", gone.log());
    assert_holder_kept(&f);
}

#[test]
fn cloud_gates_in_a_worker_sandbox_still_there_take_no_cloud_slot() {
    let f = fixture_with(LOCAL_TOML);
    started_cloud_issue(&f);
    let there = cloud_backend_for(&cloud_fs(&f), &["issue-41"], &[CLEAN], true);

    gates(&f, cloud_slot::full(&f.env, true), &there).unwrap();

    assert!(there.creates().is_empty(), "{:?}", there.log());
    assert_holder_kept(&f);
}

// ---- task fix --pr ----

#[test]
fn a_cloud_fix_with_no_free_cloud_slot_and_no_wait_refuses_creating_nothing() {
    let f = fixture_with(CLOUD_TOML);
    let head = common::fix_task::new_head(&f);
    // Every input is good: the cloud slot is the only refusal left.
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version(common::cloud_task::CLOUD_MIN);
    let github = common::fix_task::github_at(&head)
        .with_pr_comments(7, vec![common::fix_task::posted_review(&head)]);

    let result = sbxm::commands::task_fix::run(
        &f.env.config_dir(),
        &sbxm::commands::task_fix::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some("o/r".into()),
            base: None,
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: cloud_slot::full(&f.env, true),
            check_run: None,
            location: None,
        },
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.starts_with(&cloud_slot::refusal()), "{err}");
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert!(!record::tasks_root(&f.env.base_dir()).exists());
    assert_eq!(common::fix_task::origin_head(&f), head);
    assert_holder_kept(&f);
}

// ---- Review of issue 599, M-1 and M-2: the cloud slot comes after every input check, and only
// for a command that goes on to make a cloud sandbox ----
//
// [`cloud_slot::full`] with `no_wait = false` is a full pool whose gate panics at the first wait:
// a command that reaches the wait over a bad input fails these tests.

/// The duration error a `--time-limit nonsense` gets.
const BAD_LIMIT: &str = "--time-limit \"nonsense\" isn't a duration";

fn assert_no_ticket(f: &Fixture) {
    assert!(
        cloud_slot::queue(&f.env).is_empty(),
        "{:?}",
        cloud_slot::queue(&f.env)
    );
    assert_holder_kept(f);
}

#[test]
fn a_cloud_spec_run_with_a_bad_time_limit_says_so_before_the_cloud_slot() {
    for no_wait in [false, true] {
        let f = fixture_with(LOCAL_TOML);
        let (spec, id) = spec(&f);
        let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
        let mut opts = run_options(&f, spec, Some(Location::Cloud));
        opts.time_limit = Some("nonsense".into());
        opts.disk = cloud_slot::full(&f.env, no_wait);

        let result = sbxm::commands::task_run::run_spec(
            &f.env.config_dir(),
            &opts,
            None,
            &b,
            &FakeGitHub::default(),
            &Probe,
            &FakeHostRunner::default(),
            &mut Vec::new(),
            &mut Vec::new(),
        );

        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains(BAD_LIMIT), "no_wait {no_wait}: {err}");
        assert!(b.creates().is_empty(), "{:?}", b.log());
        assert_no_ticket(&f);
    }
}

fn issue_github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
}

fn issue_run_options(f: &Fixture, disk: sbxm::disk::Guard) -> sbxm::commands::task_run::Options {
    sbxm::commands::task_run::Options {
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
        clone_source: Some(common::task_fixture::source(f)),
        identity: Some(identity()),
        disk,
        location: Some(Location::Cloud),
    }
}

fn run_issue(f: &Fixture, opts: &sbxm::commands::task_run::Options, backend: &FakeBackend) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run(
        &f.env.config_dir(),
        opts,
        None,
        backend,
        &issue_github(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    ran(result, out, warn)
}

#[test]
fn a_cloud_issue_run_with_a_bad_time_limit_or_base_says_so_before_the_cloud_slot() {
    for (limit, base, expected) in [
        (Some("nonsense"), None, BAD_LIMIT),
        (None, Some("a..b"), "isn't a usable branch name"),
    ] {
        let f = fixture_with(LOCAL_TOML);
        let b = cloud_backend_for(&cloud_fs(&f), &["issue-41"], &[CLEAN], true);
        let mut opts = issue_run_options(&f, cloud_slot::full(&f.env, false));
        opts.time_limit = limit.map(str::to_owned);
        opts.base = base.map(str::to_owned);

        let ran = run_issue(&f, &opts, &b);

        let err = format!("{:#}", ran.result.unwrap_err());
        assert!(err.contains(expected), "{err}");
        assert!(b.creates().is_empty(), "{:?}", b.log());
        assert_no_ticket(&f);
    }
}

/// The issue run takes its cloud slot inside its start, after the start's own checks, and holds
/// that one slot on through the review.
#[test]
fn a_cloud_issue_run_holds_one_cloud_slot_through_worker_and_review() {
    let f = fixture_with(LOCAL_TOML);
    let disk = cloud_slot::free(&f.env);
    let slot = cloud_slot::cloud_slot_file(&f.env);
    let tokens = std::sync::Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let seen = std::sync::Arc::clone(&tokens);
    let b = cloud_backend_observing(
        &cloud_fs(&f),
        &["issue-41"],
        &[CLEAN],
        true,
        ok(CLAUDE_DONE),
        move |sandbox, _, spec| {
            if !is_headless(spec) {
                return;
            }
            let held: sbxm::slot::Holder =
                serde_json::from_str(&std::fs::read_to_string(&slot).unwrap()).unwrap();
            assert_eq!(held.process.pid, std::process::id(), "{sandbox}");
            seen.lock().unwrap().push((sandbox.to_owned(), held.token));
        },
    );

    let ran = run_issue(&f, &issue_run_options(&f, disk), &b);

    ran.result.expect(&ran.out);
    let tokens = tokens.lock().unwrap();
    assert!(
        tokens.iter().any(|(s, _)| s.contains("-review-")),
        "{tokens:?}"
    );
    assert!(
        tokens.iter().any(|(s, _)| !s.contains("-review-")),
        "{tokens:?}"
    );
    assert!(tokens.iter().all(|(_, t)| *t == tokens[0].1), "{tokens:?}");
    assert!(
        cloud_slot::holder(&f.env).is_none(),
        "given back at the end"
    );
    assert!(cloud_slot::queue(&f.env).is_empty());
}

#[test]
fn a_cloud_issue_start_with_a_bad_base_says_so_before_the_cloud_slot() {
    let f = fixture_with(LOCAL_TOML);
    let b = cloud_backend_for(&cloud_fs(&f), &["issue-41"], &[CLEAN], true);
    let opts = sbxm::commands::task_start::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("a..b".into()),
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(&f)),
        identity: Some(identity()),
        disk: cloud_slot::full(&f.env, false),
        location: Some(Location::Cloud),
    };

    let result = sbxm::commands::task_start::run(
        &f.env.config_dir(),
        &opts,
        &b,
        &issue_github(),
        &Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("isn't a usable branch name"), "{err}");
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert_no_ticket(&f);
}

#[test]
fn a_cloud_fix_of_a_closed_pr_says_so_before_the_cloud_slot() {
    let f = fixture_with(CLOUD_TOML);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(sbxm::github::PrInfo {
            state: sbxm::github::PrState::Closed,
            ..common::fix_task::pr7("0123456789abcdef0123456789abcdef01234567")
        });

    let result = sbxm::commands::task_fix::run(
        &f.env.config_dir(),
        &sbxm::commands::task_fix::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some("o/r".into()),
            base: None,
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: cloud_slot::full(&f.env, false),
            check_run: None,
            location: None,
        },
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(!err.contains("cloud slot"), "{err}");
    assert!(err.contains("closed"), "{err}");
    assert!(backend.creates().is_empty(), "{:?}", backend.log());
    assert_no_ticket(&f);
}

fn review_spec(
    f: &Fixture,
    spec: PathBuf,
    force: bool,
    disk: sbxm::disk::Guard,
    b: &FakeBackend,
) -> Ran {
    let opts = sbxm::commands::task_review::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: sbxm::commands::task_review::Target::Spec(spec),
        repo: Some("o/r".into()),
        base: None,
        clone_source: Some(common::task_fixture::source(f)),
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force,
        disk,
        location: None,
        identity: Some(identity()),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_review::run(
        &f.env.config_dir(),
        &opts,
        b,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    ran(result, out, warn)
}

#[test]
fn a_cloud_review_of_a_task_already_ready_says_so_before_the_cloud_slot() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = spec(&f);
    let first = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);
    let done = task_run(&f, spec.clone(), Some(Location::Cloud), &first);
    done.result.expect(&done.out);
    let b = cloud_backend(&cloud_fs(&f), &id, &[CLEAN]);

    let ran = review_spec(&f, spec, false, cloud_slot::full(&f.env, false), &b);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.contains("already ready"), "{err}");
    assert!(b.creates().is_empty(), "{:?}", b.log());
    assert_no_ticket(&f);
}

#[test]
fn a_cloud_pr_review_with_a_bad_base_says_so_before_the_cloud_slot() {
    let f = fixture_with(CLOUD_TOML);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let opts = sbxm::commands::task_review::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: sbxm::commands::task_review::Target::Pr(7),
        repo: Some("o/r".into()),
        base: Some("a..b".into()),
        clone_source: Some(common::task_fixture::source(&f)),
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force: false,
        disk: cloud_slot::full(&f.env, false),
        location: None,
        identity: Some(identity()),
    };

    let result = sbxm::commands::task_review::run(
        &f.env.config_dir(),
        &opts,
        &backend,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("isn't a usable branch name"), "{err}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert_no_ticket(&f);
}

/// M-2: a ready cloud fix with only its push left makes no sandbox, so it takes no cloud slot.
#[test]
fn a_push_only_resume_of_a_cloud_fix_takes_no_cloud_slot() {
    let f = common::fix_task::config();
    let head = common::fix_task::new_head(&f);
    let id = common::fix_task::ready_fix(&f, &head);
    let meta = record::task_dir(&f.env.base_dir(), &id);
    let mut task = task_record(&f, &id);
    task.location = Location::Cloud;
    record::write(&meta, &task).unwrap();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Pr(7),
            rounds: None,
            should_fix: false,
            disk: cloud_slot::full(&f.env, true),
            identity: None,
            forget_ambiguous: false,
        },
        &backend,
        &common::fix_task::github_at(&head),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let ran = ran(result, out, warn);
    ran.result.expect(&ran.out);
    assert!(
        ran.out.contains("resuming from ready: its push"),
        "{}",
        ran.out
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert_no_ticket(&f);
}

/// A cloud resume that goes on to make a sandbox still meets the cloud limit, once its checks
/// have passed.
#[test]
fn a_cloud_resume_that_makes_a_sandbox_still_waits_for_the_cloud_slot() {
    let f = fixture_with(LOCAL_TOML);
    let (spec, id) = failed_cloud_task(&f);
    let b = common::cloud_task::cloud_backend_listing(&cloud_fs(&f), &id, &[CLEAN], false);

    let ran = resume(&f, spec, cloud_slot::full(&f.env, true), &b);

    assert_refused(&ran);
    assert!(b.creates().is_empty(), "{:?}", b.log());
}

// ---- Second review of issue 599, M-1 (repeat): the worker's cloud preflight comes before the
// cloud slot too ----

/// What the cloud says when the worker's (claude) login isn't stored.
const NO_WORKER_LOGIN: &str = "no global 'anthropic' login";

fn fix_without_worker_login(
    f: &Fixture,
    head: &str,
    no_wait: bool,
) -> (anyhow::Result<()>, FakeBackend) {
    // Only the reviewer's (codex) login is stored.
    let backend =
        FakeBackend::with_secrets(&["openai"]).with_version(common::cloud_task::CLOUD_MIN);
    let github = common::fix_task::github_at(head)
        .with_pr_comments(7, vec![common::fix_task::posted_review(head)]);
    let result = sbxm::commands::task_fix::run(
        &f.env.config_dir(),
        &sbxm::commands::task_fix::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some("o/r".into()),
            base: None,
            clone_source: Some(common::task_fixture::source(f)),
            identity: Some(identity()),
            disk: cloud_slot::full(&f.env, no_wait),
            check_run: None,
            location: None,
        },
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    (result, backend)
}

#[test]
fn a_cloud_fix_without_its_worker_s_login_says_so_before_the_cloud_slot() {
    for no_wait in [false, true] {
        let f = fixture_with(CLOUD_TOML);
        let head = common::fix_task::new_head(&f);

        let (result, backend) = fix_without_worker_login(&f, &head, no_wait);

        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains(NO_WORKER_LOGIN), "no_wait {no_wait}: {err}");
        assert!(backend.creates().is_empty(), "{:?}", backend.log());
        assert!(!record::tasks_root(&f.env.base_dir()).exists());
        assert_no_ticket(&f);
    }
}

#[test]
fn a_cloud_spec_start_without_its_worker_s_login_says_so_before_the_cloud_slot() {
    for no_wait in [false, true] {
        let f = fixture_with(LOCAL_TOML);
        let (spec, id) = spec(&f);
        let b = FakeBackend::with_secrets(&["openai"]).with_version(common::cloud_task::CLOUD_MIN);
        let opts = sbxm::commands::task_start::SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
            worker_harness: None,
            worker_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: cloud_slot::full(&f.env, no_wait),
            location: Some(Location::Cloud),
        };

        let result = sbxm::commands::task_start::run_spec(
            &f.env.config_dir(),
            &opts,
            &b,
            &FakeGitHub::default(),
            &Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        );

        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains(NO_WORKER_LOGIN), "no_wait {no_wait}: {err}");
        assert!(b.creates().is_empty(), "{:?}", b.log());
        assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
        assert_no_ticket(&f);
    }
}

/// A valid cloud fix still meets the cloud limit, and keeps its one slot through its worker and
/// its review.
#[test]
fn a_valid_cloud_fix_holds_one_cloud_slot_through_worker_and_review() {
    let f = fixture_with(CLOUD_TOML);
    let head = common::fix_task::new_head(&f);
    let id = record::fix_id(7, &head).unwrap();
    let slot = cloud_slot::cloud_slot_file(&f.env);
    let tokens = std::sync::Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let seen = std::sync::Arc::clone(&tokens);
    let b = cloud_backend_observing(
        &cloud_fs(&f),
        &[&id],
        &[CLEAN],
        true,
        ok(CLAUDE_DONE),
        move |sandbox, _, spec| {
            if !is_headless(spec) {
                return;
            }
            let held: sbxm::slot::Holder =
                serde_json::from_str(&std::fs::read_to_string(&slot).unwrap()).unwrap();
            assert_eq!(held.process.pid, std::process::id(), "{sandbox}");
            seen.lock().unwrap().push((sandbox.to_owned(), held.token));
        },
    );
    let github = common::fix_task::github_at(&head)
        .with_pr_comments(7, vec![common::fix_task::posted_review(&head)]);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_fix::run(
        &f.env.config_dir(),
        &sbxm::commands::task_fix::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some("o/r".into()),
            base: None,
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: cloud_slot::free(&f.env),
            check_run: None,
            location: None,
        },
        &b,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    let ran = ran(result, out, warn);
    let tokens = tokens.lock().unwrap();
    assert!(!tokens.is_empty(), "{}\n{}", ran.out, ran.warn);
    assert!(tokens.iter().all(|(_, t)| *t == tokens[0].1), "{tokens:?}");
    assert!(
        tokens.iter().any(|(s, _)| s.contains("-review-")),
        "{tokens:?}\n{}\n{}",
        ran.out,
        ran.warn
    );
    assert!(
        cloud_slot::holder(&f.env).is_none(),
        "given back at the end"
    );
}
