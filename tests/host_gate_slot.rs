//! Milestone 5 slice 7 (issue 634, decision 317(5)): a task's host-tier gates take the machine's
//! local sandbox slot while they run, and give it back as soon as they end. A local task already
//! holds it; a cloud task takes it for the host commands only. Through `FakeBackend`.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::cloud_task::{CLEAN, ONE, cloud_backend_observing, run_options, spec};
use common::slot;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, backend, fixture_with, is_headless, issue_text,
    ok, open_issue, play_reviews, play_tasks, source, worked_task,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline::Tiers;
use sbxm::task::record;

/// The host command: it passes in the real shell `task start`'s own gate run uses, too.
const HOST: &str = "exit 0";

fn toml(host: &str) -> String {
    format!(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\nhost = [{host}]\n\n[reviewer]\nharness = \"codex\"\n"
    )
}

fn with_host() -> Fixture {
    fixture_with(&toml(&format!("\"{HOST}\"")))
}

fn cloud_fs(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("cloud-fs")
}

struct Ran {
    result: anyhow::Result<()>,
    out: String,
}

/// A cloud backend whose every headless run (worker, fix round, reviewer) finds the sandbox slot
/// free: a cloud task holds it for its host gates only. The runs seen are counted.
fn cloud_backend_never_holding(
    f: &Fixture,
    id: &str,
    reviews: &[&str],
) -> (FakeBackend, Arc<Mutex<Vec<String>>>) {
    let file = slot::slot_file(&f.env);
    let runs = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&runs);
    let b = cloud_backend_observing(
        &cloud_fs(f),
        &[id],
        reviews,
        true,
        ok(CLAUDE_DONE),
        move |sandbox, _, spec| {
            if !is_headless(spec) {
                return;
            }
            assert!(
                !file.exists(),
                "{sandbox}: the sandbox slot is held: {:?}",
                std::fs::read_to_string(&file)
            );
            seen.lock().unwrap().push(sandbox.to_owned());
        },
    );
    (b, runs)
}

/// `task run --spec FILE --cloud` with `disk` and `host`.
fn run_cloud(
    f: &Fixture,
    spec: PathBuf,
    disk: sbxm::disk::Guard,
    backend: &FakeBackend,
    host: &FakeHostRunner,
) -> Ran {
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
        host,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
    }
}

/// A host runner whose every command finds the sandbox slot held by this process for `task`,
/// with no command waiting in line. The commands it saw are counted.
fn host_seeing_the_slot_held(f: &Fixture, task: &str) -> FakeHostRunner {
    let env_slot = slot::slot_file(&f.env);
    let queue = slot::slots_dir(&f.env).join("queue");
    let task = task.to_owned();
    FakeHostRunner::default().with_hook(move |_, command| {
        let text = std::fs::read_to_string(&env_slot).unwrap_or_else(|e| {
            panic!("{command}: the sandbox slot is not held: {e}");
        });
        let holder: sbxm::slot::Holder = serde_json::from_str(&text).unwrap();
        assert_eq!(holder.process.pid, std::process::id(), "{command}");
        assert_eq!(holder.task.as_deref(), Some(task.as_str()), "{command}");
        let waiting = std::fs::read_dir(&queue).map_or(0, |entries| entries.count());
        assert_eq!(waiting, 0, "{command}: a ticket in line");
    })
}

// ---- Boundary cases ----

#[test]
fn with_sandbox_slots_unset_a_cloud_tasks_host_gates_run_at_once_writing_no_slot_file() {
    let f = with_host();
    let (spec, id) = spec(&f);
    let (b, _) = cloud_backend_never_holding(&f, &id, &[ONE, CLEAN]);
    let host = FakeHostRunner::default();
    // Never waits: its sleep panics.
    let disk = sbxm::disk::Guard {
        slot: slot::gate(&f.env, false),
        ..Default::default()
    };

    let ran = run_cloud(&f, spec, disk, &b, &host);

    ran.result.expect(&ran.out);
    assert!(!host.calls().is_empty(), "{}", ran.out);
    assert!(!f.env.tmp.path().join("local-data").exists());
}

#[test]
fn with_no_host_gates_a_cloud_task_takes_no_slot_though_another_command_holds_it() {
    let f = fixture_with(&toml(""));
    let (spec, id) = spec(&f);
    let b = cloud_backend_plain(&f, &id);
    let host = FakeHostRunner::default();

    // Held by another command: a take would refuse at once.
    let ran = run_cloud(&f, spec, slot::full(&f.env, true), &b, &host);

    ran.result.expect(&ran.out);
    assert!(host.calls().is_empty());
    assert!(!ran.out.contains("waiting for a"), "{}", ran.out);
    assert_eq!(slot::holder(&f.env).unwrap().process.pid, slot::HOLDER_PID);
    assert!(slot::queue(&f.env).is_empty());
}

fn cloud_backend_plain(f: &Fixture, id: &str) -> FakeBackend {
    common::cloud_task::cloud_backend(&cloud_fs(f), id, &[CLEAN])
}

/// The first gate run (after the worker) passes; the review's must-fix finding brings a fix round,
/// whose gates run on the faked host runner, then a clean review.
#[test]
fn a_cloud_tasks_host_gates_hold_the_sandbox_slot_only_while_they_run() {
    let f = with_host();
    let (spec, id) = spec(&f);
    let (b, runs) = cloud_backend_never_holding(&f, &id, &[ONE, CLEAN]);
    let host = host_seeing_the_slot_held(&f, &id);

    let ran = run_cloud(&f, spec, slot::free(&f.env), &b, &host);

    ran.result.expect(&ran.out);
    assert!(!host.calls().is_empty(), "{}", ran.out);
    let runs = runs.lock().unwrap();
    assert!(
        runs.iter().filter(|s| s.contains("-review-")).count() >= 2,
        "a reviewer ran after the fix round's gates: {runs:?}"
    );
    assert!(slot::holder(&f.env).is_none(), "given back");
    assert!(slot::queue(&f.env).is_empty());
}

#[test]
fn a_cloud_tasks_host_gates_wait_for_the_slot_another_task_holds_then_run() {
    let f = with_host();
    let (spec, id) = spec(&f);
    let b = cloud_backend_plain(&f, &id);
    let host = FakeHostRunner::default();

    let ran = run_cloud(&f, spec, slot::full_until_first_wait(&f.env), &b, &host);

    ran.result.expect(&ran.out);
    assert!(
        ran.out.contains(&format!(
            "waiting for a sandbox slot: held by {}\n",
            slot::HOLDER
        )),
        "{}",
        ran.out
    );
    assert!(slot::holder(&f.env).is_none());
}

#[test]
fn a_cloud_tasks_failing_host_gate_still_gives_the_slot_back() {
    let f = with_host();
    let (spec, id) = spec(&f);
    let (b, _) = cloud_backend_never_holding(&f, &id, &[ONE, CLEAN]);
    let host = FakeHostRunner::default().with_exit(HOST, 1, "", "red\n");

    let ran = run_cloud(&f, spec, slot::free(&f.env), &b, &host);

    assert!(!host.calls().is_empty(), "{}", ran.out);
    assert!(slot::holder(&f.env).is_none(), "{}", ran.out);
    assert!(slot::queue(&f.env).is_empty());
}

#[test]
fn a_cloud_tasks_timed_out_host_gate_still_gives_the_slot_back() {
    let f = with_host();
    let (spec, id) = spec(&f);
    let (b, _) = cloud_backend_never_holding(&f, &id, &[ONE, CLEAN]);
    let host = FakeHostRunner::default().with_timeout(HOST);

    let ran = run_cloud(&f, spec, slot::free(&f.env), &b, &host);

    assert!(!host.calls().is_empty(), "{}", ran.out);
    assert!(slot::holder(&f.env).is_none(), "{}", ran.out);
    assert!(slot::queue(&f.env).is_empty());
}

// ---- A local task ----

fn local_backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
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

#[test]
fn a_local_tasks_host_gates_run_in_the_slot_it_holds_without_a_second_ticket() {
    let f = with_host();
    let b = local_backend(&f, &[ONE, CLEAN]);
    let host = host_seeing_the_slot_held(&f, "issue-41");
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")]);
    let opts = sbxm::commands::task_run::Options {
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
        clone_source: Some(source(&f)),
        identity: Some(common::cloud_task::identity()),
        // Never waits: its sleep panics.
        disk: slot::free(&f.env),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_run::run(
        &f.env.config_dir(),
        &opts,
        None,
        &b,
        &github,
        &Probe,
        &host,
        &mut out,
        &mut warn,
    );

    let out = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    result.expect(&out);
    assert!(!host.calls().is_empty(), "{out}");
    assert!(!out.contains("waiting for a"), "{out}");
    assert!(slot::holder(&f.env).is_none());
}

// ---- task gates ----

#[test]
fn task_gates_of_the_host_tier_with_no_wait_and_the_slot_held_is_refused_running_nothing() {
    let f = with_host();
    let backend = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    worked_task(&f, &backend);
    let host = FakeHostRunner::default();
    let gates = |tiers, disk| {
        let opts = sbxm::commands::task_gates::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issue: 41,
            tiers,
            dry_run: false,
            identity: None,
            disk,
        };
        sbxm::commands::task_gates::run(
            &f.env.config_dir(),
            &opts,
            &backend,
            &Probe,
            &host,
            &mut Vec::new(),
        )
    };
    // The host tier builds on a sandbox pass of the same commit.
    gates(Tiers::SANDBOX, Default::default()).unwrap();
    let task_json = record::task_dir(&f.env.base_dir(), "issue-41").join("task.json");
    let before = std::fs::read(&task_json).unwrap();
    let execs = backend.execs().len();

    let result = gates(Tiers::HOST, slot::full(&f.env, true));

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.starts_with(&slot::refusal()), "{err}");
    assert!(host.calls().is_empty());
    assert_eq!(backend.execs().len(), execs, "nothing ran in the sandbox");
    assert_eq!(std::fs::read(&task_json).unwrap(), before);
    assert_eq!(slot::holder(&f.env).unwrap().process.pid, slot::HOLDER_PID);
}

/// Issue 41 started in the cloud, its worker sandbox kept, and its sandbox tier passed with
/// `task gates --tier sandbox`.
fn cloud_issue_gated(f: &Fixture) -> FakeBackend {
    let b = common::cloud_task::cloud_backend(&cloud_fs(f), "issue-41", &[CLEAN]);
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
        clone_source: Some(source(f)),
        identity: Some(common::cloud_task::identity()),
        disk: Default::default(),
        location: Some(Location::Cloud),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let started = sbxm::commands::task_start::run_with(
        &f.env.config_dir(),
        &opts,
        None,
        &b,
        &github,
        &Probe,
        &mut out,
        &mut warn,
    );
    let out = String::from_utf8(out).unwrap();
    started.expect(&out);
    cloud_gates(
        f,
        &b,
        Tiers::SANDBOX,
        Default::default(),
        &FakeHostRunner::default(),
    )
    .result
    .unwrap();
    b
}

/// `task gates --issue 41` of `tiers`.
fn cloud_gates(
    f: &Fixture,
    b: &FakeBackend,
    tiers: Tiers,
    disk: sbxm::disk::Guard,
    host: &FakeHostRunner,
) -> Ran {
    let mut out = Vec::new();
    let result = sbxm::commands::task_gates::run(
        &f.env.config_dir(),
        &sbxm::commands::task_gates::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issue: 41,
            tiers,
            dry_run: false,
            identity: Some(common::cloud_task::identity()),
            disk,
        },
        b,
        &Probe,
        host,
        &mut out,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

#[test]
fn task_gates_of_a_cloud_tasks_host_tier_with_no_wait_and_the_slot_held_is_refused_running_nothing()
{
    let f = with_host();
    let b = cloud_issue_gated(&f);
    let task_json = record::task_dir(&f.env.base_dir(), "issue-41").join("task.json");
    let before = std::fs::read(&task_json).unwrap();
    let execs = b.execs().len();
    let host = FakeHostRunner::default();

    let ran = cloud_gates(&f, &b, Tiers::HOST, slot::full(&f.env, true), &host);

    let err = format!("{:#}", ran.result.unwrap_err());
    assert!(err.starts_with(&slot::refusal()), "{err}");
    assert!(host.calls().is_empty());
    assert_eq!(b.execs().len(), execs, "nothing ran in the sandbox");
    assert_eq!(std::fs::read(&task_json).unwrap(), before);
    assert_eq!(slot::holder(&f.env).unwrap().process.pid, slot::HOLDER_PID);
}

#[test]
fn task_gates_of_a_cloud_task_hold_the_slot_for_the_host_tier_and_give_it_back() {
    let f = with_host();
    let b = cloud_issue_gated(&f);
    let host = host_seeing_the_slot_held(&f, "issue-41");

    let ran = cloud_gates(&f, &b, Tiers::HOST, slot::free(&f.env), &host);

    ran.result.expect(&ran.out);
    assert_eq!(host.calls().len(), 1);
    assert!(slot::holder(&f.env).is_none());
}

/// All tiers: the sandbox tier runs without the slot, the host tier waits for it, saying so.
#[test]
fn task_gates_of_a_cloud_task_wait_for_the_slot_for_the_host_tier_saying_so() {
    let f = with_host();
    let b = cloud_issue_gated(&f);
    let host = FakeHostRunner::default();

    let ran = cloud_gates(
        &f,
        &b,
        Tiers::ALL,
        slot::full_until_first_wait(&f.env),
        &host,
    );

    ran.result.expect(&ran.out);
    assert!(
        ran.out.contains(&format!(
            "waiting for a sandbox slot: held by {}\n",
            slot::HOLDER
        )),
        "{}",
        ran.out
    );
    assert_eq!(host.calls().len(), 1);
    assert!(slot::holder(&f.env).is_none());
}

/// `task run --issue N --cloud` holds its slots through worker and review (issue 599): the host
/// gates' slot is given back all the same, before the next reviewer runs.
#[test]
fn a_cloud_issue_runs_host_gates_give_the_slot_back_before_the_review_goes_on() {
    let f = with_host();
    let (b, runs) = cloud_backend_never_holding(&f, "issue-41", &[ONE, CLEAN]);
    let host = host_seeing_the_slot_held(&f, "issue-41");
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")]);
    let opts = sbxm::commands::task_run::Options {
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
        clone_source: Some(source(&f)),
        identity: Some(common::cloud_task::identity()),
        disk: slot::free(&f.env),
        location: Some(Location::Cloud),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = sbxm::commands::task_run::run(
        &f.env.config_dir(),
        &opts,
        None,
        &b,
        &github,
        &Probe,
        &host,
        &mut out,
        &mut warn,
    );

    let out = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    result.expect(&out);
    assert!(!host.calls().is_empty(), "{out}");
    let runs = runs.lock().unwrap();
    assert!(
        runs.iter().filter(|s| s.contains("-review-")).count() >= 2,
        "a reviewer ran after the fix round's gates: {runs:?}"
    );
    assert!(slot::holder(&f.env).is_none());
}

/// Review M-1 (one lock order, cloud before local): `task gates --tier host` of a cloud task
/// whose worker sandbox is gone waits for the cloud slot to make it again without holding the
/// sandbox slot meanwhile, which a running cloud task, holding its cloud slot, may be waiting for.
#[test]
fn task_gates_of_a_cloud_task_never_waits_for_a_cloud_slot_holding_the_sandbox_slot() {
    let f = with_host();
    cloud_issue_gated(&f);
    let gone = common::cloud_task::cloud_backend_for(&cloud_fs(&f), &["issue-41"], &[CLEAN], false);
    slot::set_slots(&f.env, 1);
    let mut disk = common::cloud_slot::full(&f.env, false);
    let (local, cloud) = (
        slot::slot_file(&f.env),
        common::cloud_slot::cloud_slot_file(&f.env),
    );
    disk.slot.sleep = Arc::new(move |_| {
        assert!(
            !local.exists(),
            "waiting for a cloud slot while holding the sandbox slot"
        );
        std::fs::remove_file(&cloud).unwrap();
    });
    let host = host_seeing_the_slot_held(&f, "issue-41");

    let ran = cloud_gates(&f, &gone, Tiers::HOST, disk, &host);

    ran.result.expect(&ran.out);
    assert!(ran.out.contains("waiting for a cloud slot"), "{}", ran.out);
    assert!(
        !gone.creates().is_empty(),
        "the worker sandbox was made again"
    );
    assert_eq!(host.calls().len(), 1);
    assert!(slot::holder(&f.env).is_none());
}
