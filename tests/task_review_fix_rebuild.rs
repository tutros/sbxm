//! Issue 359 (option (a), decision of 2026-11-01): a local fix round whose worker sandbox is gone
//! (removed once its commits were saved, `sdlc-issue-flow` stage 4) makes it again the way
//! `task resume` does, through `ensure_worker_sandbox`: the recorded profile and resources, the
//! same config-drift refusal. A refusal records the task `failed` with a note naming the sandbox.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play_reviews, play_tasks,
    source, worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend, SandboxInfo};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, Prepared, ReviewReport};
use sbxm::task::record::{self, Stage, Status};

const WORKER: &str = "sbxm-task-issue-41-claude";
const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str =
    "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 does the wrong thing.\n";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\ncpus = 3\nmemory = \"5g\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

/// The worker (and a fix round) commit `a.txt`; the n-th review is `reviews[n]`.
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

fn red() -> ExecOutput {
    ExecOutput {
        stdout: String::new(),
        stderr: "red\n".into(),
        exit_code: Some(101),
    }
}

/// The worker sandbox, listed by `sbx ls`.
fn listed() -> Vec<SandboxInfo> {
    vec![SandboxInfo {
        name: WORKER.into(),
        agent: "claude".into(),
        status: "running".into(),
    }]
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
}

fn review(
    f: &Fixture,
    backend: &FakeBackend,
    prepared: &mut Prepared,
) -> anyhow::Result<ReviewReport> {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::review_issue(&ctx(f, &source, backend, &github).env(), prepared)
}

/// The position in `backend`'s log of the first entry `entry`, if any.
fn at(backend: &FakeBackend, entry: &str) -> Option<usize> {
    backend.log().iter().position(|l| l == entry)
}

/// The position in `backend`'s exec list of the fix round's headless run.
fn fix_exec(backend: &FakeBackend) -> Option<usize> {
    backend.execs().iter().position(|(sandbox, spec)| {
        sandbox == WORKER && spec.argv.iter().any(|a| a.contains("fix-prompt.md"))
    })
}

/// The log position of the `n`-th exec (0-based).
fn exec_at(backend: &FakeBackend, n: usize) -> usize {
    backend
        .log()
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("exec "))
        .nth(n)
        .map(|(i, _)| i)
        .unwrap()
}

#[test]
fn a_worker_sandbox_still_there_is_used_as_is() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    // Gates fail, the fix round's bundle and status, the gates again (pass).
    let backend = backend(&f, &[CLEAN])
        .with_exec_outputs(vec![red(), ok(""), ok(""), ok("")])
        .and_sandboxes(listed());

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert!(fix_exec(&backend).is_some());
    let log = backend.log();
    assert!(
        !log.iter()
            .any(|l| l == &format!("create {WORKER}") || l == &format!("rm {WORKER}")),
        "{log:?}"
    );
    assert!(
        !prepared.events.iter().any(|e| e.contains("made it again")),
        "{:?}",
        prepared.events
    );
}

#[test]
fn a_gone_worker_sandbox_is_made_again_before_a_fix_round_after_failed_gates() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let backend = backend(&f, &[CLEAN]).with_exec_outputs(vec![red(), ok(""), ok(""), ok("")]);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    let created = at(&backend, &format!("create {WORKER}")).expect("the worker is made again");
    let fix = fix_exec(&backend).expect("the fix round ran");
    assert!(created < exec_at(&backend, fix), "{:?}", backend.log());
    let create = backend
        .creates()
        .into_iter()
        .find(|c| c.name == WORKER)
        .unwrap();
    assert_eq!((create.cpus, create.memory.as_str()), (3, "5g"));
    assert!(
        prepared
            .events
            .contains(&"issue-41: the worker's sandbox was gone; made it again".to_owned()),
        "{:?}",
        prepared.events
    );
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_gone_worker_sandbox_is_made_again_before_a_fix_round_after_must_fix_findings() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let backend = backend(&f, &[ONE, CLEAN]);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    let created = at(&backend, &format!("create {WORKER}")).expect("the worker is made again");
    let fix = fix_exec(&backend).expect("the fix round ran");
    assert!(created < exec_at(&backend, fix), "{:?}", backend.log());
    assert!(
        prepared
            .events
            .contains(&"issue-41: the worker's sandbox was gone; made it again".to_owned()),
        "{:?}",
        prepared.events
    );
}

#[test]
fn a_gone_worker_sandbox_whose_config_changed_is_refused_and_the_task_fails() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    // The recorded hash no longer matches the profile: the config changed since the task started.
    prepared.record.config_hash = "changed".into();
    record::write(&prepared.meta, &prepared.record).unwrap();
    let backend = backend(&f, &[CLEAN]).with_exec_outputs(vec![red()]);

    let message = format!("{:#}", review(&f, &backend, &mut prepared).unwrap_err());

    assert!(message.contains("changed since it started"), "{message}");
    assert!(
        !backend.creates().iter().any(|c| c.name == WORKER),
        "{:?}",
        backend.log()
    );
    assert!(backend.removes().is_empty(), "{:?}", backend.log());
    assert!(fix_exec(&backend).is_none());
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Fixing, Status::Failed)
    );
    assert!(
        record
            .notes
            .iter()
            .any(|n| n.contains("worker sandbox") && n.contains("changed since")),
        "{:?}",
        record.notes
    );
}
