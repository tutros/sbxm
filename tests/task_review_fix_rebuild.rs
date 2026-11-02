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

// ---- Review M-1: a failed rebuild keeps the round's input for `task resume` ----

/// Two fix rounds, so a second round can find its sandbox gone too.
fn two_rounds() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\ncpus = 3\nmemory = \"5g\"\n\n[worker]\nfix_rounds = 2\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

/// The worker's gate runs answer red where `reds` says so (by run, from 0), green otherwise.
/// With `drift`, the first fix round's run changes that profile file, so a later rebuild of the
/// worker's sandbox is refused for config drift.
fn gates(
    backend: FakeBackend,
    reds: &'static [bool],
    drift: Option<std::path::PathBuf>,
) -> FakeBackend {
    let runs = std::sync::atomic::AtomicUsize::new(0);
    backend.with_exec_responder(move |sandbox, spec| {
        let mentions = |s: &str| spec.argv.iter().any(|a| a.contains(s));
        if let Some(profile) = &drift
            && mentions("fix-prompt.md")
        {
            std::fs::write(profile, "[network]\nallow = [\"github.com\"]\n").unwrap();
        }
        if sandbox != WORKER || !mentions("cargo test") || mentions("prompt") {
            return None;
        }
        let n = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(match reds.get(n) {
            Some(true) => ExecOutput {
                stdout: String::new(),
                stderr: format!("red run {n}\n"),
                exit_code: Some(101),
            },
            _ => ok(""),
        })
    })
}

fn profile_file(f: &Fixture) -> std::path::PathBuf {
    f.env.profiles_dir().join("default").join("profile.toml")
}

fn kept(f: &Fixture, name: &str) -> Option<String> {
    std::fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join(name)).ok()
}

fn resume(f: &Fixture, backend: &FakeBackend, prepared: &mut Prepared) -> anyhow::Result<()> {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx(f, &source, backend, &github);
    // The failed run's process is over.
    let env = sbxm::task::pipeline::TaskEnv {
        probe: &Gone,
        ..ctx.env()
    };
    sbxm::task::resume::resume(&env, prepared, None).map(|_| ())
}

struct Gone;

impl sbxm::task::record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

#[test]
fn a_first_fix_round_whose_rebuild_failed_resumes_with_its_gate_input() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let failing = gates(backend(&f, &[CLEAN]), &[true], None).with_failing_create_for(WORKER);

    review(&f, &failing, &mut prepared).unwrap_err();

    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Fixing, Status::Failed)
    );
    assert!(kept(&f, "gate-failure.md").unwrap().contains("red run 0"));
    assert!(
        kept(&f, "fix-prompt.md")
            .unwrap()
            .contains("gate-failure.md")
    );

    let healthy = gates(backend(&f, &[CLEAN]), &[], None);
    resume(&f, &healthy, &mut prepared).unwrap();

    assert!(fix_exec(&healthy).is_some(), "{:?}", healthy.log());
    let placed = std::fs::read_to_string(prepared.workspace.join(".sbxm-task/gate-failure.md"));
    assert!(placed.unwrap().contains("red run 0"));
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_rebuild_refused_for_drift_resumes_once_the_profile_is_restored() {
    let f = config();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let original = std::fs::read(profile_file(&f)).unwrap();
    std::fs::write(profile_file(&f), "[network]\nallow = [\"github.com\"]\n").unwrap();
    let refused = gates(backend(&f, &[CLEAN]), &[true], None);

    let message = format!("{:#}", review(&f, &refused, &mut prepared).unwrap_err());

    assert!(message.contains("changed since it started"), "{message}");
    assert!(kept(&f, "fix-prompt.md").is_some());
    std::fs::write(profile_file(&f), original).unwrap();
    let healthy = gates(backend(&f, &[CLEAN]), &[], None);
    resume(&f, &healthy, &mut prepared).unwrap();

    assert!(fix_exec(&healthy).is_some(), "{:?}", healthy.log());
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

/// Round 1 from failed gates, round 2 from a review's must-fix finding, whose rebuild is refused:
/// round 2's prompt is kept, and round 1's gate output is gone, so resume fixes the review.
#[test]
fn a_later_review_round_whose_rebuild_was_refused_keeps_its_own_input() {
    let f = two_rounds();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let first = gates(backend(&f, &[ONE]), &[true], Some(profile_file(&f)));
    let original = std::fs::read(profile_file(&f)).unwrap();

    let message = format!("{:#}", review(&f, &first, &mut prepared).unwrap_err());

    assert!(message.contains("changed since it started"), "{message}");
    assert_eq!(saved(&f).round, 1);
    assert_eq!(kept(&f, "gate-failure.md"), None);
    assert!(
        !kept(&f, "fix-prompt.md")
            .unwrap()
            .contains("gate-failure.md")
    );

    std::fs::write(profile_file(&f), original).unwrap();
    let healthy = gates(backend(&f, &[CLEAN]), &[], None);
    resume(&f, &healthy, &mut prepared).unwrap();

    let placed = std::fs::read_to_string(prepared.workspace.join(".sbxm-task/review.md"));
    assert!(placed.unwrap().contains("M-1"));
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

/// Round 1 from a review's must-fix finding, round 2 from failed gates, whose rebuild is refused:
/// round 2's own gate output and prompt are kept, not round 1's prompt.
#[test]
fn a_later_gate_round_whose_rebuild_was_refused_keeps_its_own_input() {
    let f = two_rounds();
    let mut prepared = worked_task(&f, &backend(&f, &[CLEAN]));
    let first = gates(backend(&f, &[ONE]), &[false, true], Some(profile_file(&f)));
    let original = std::fs::read(profile_file(&f)).unwrap();

    let message = format!("{:#}", review(&f, &first, &mut prepared).unwrap_err());

    assert!(message.contains("changed since it started"), "{message}");
    assert_eq!(saved(&f).round, 1);
    assert!(kept(&f, "gate-failure.md").unwrap().contains("red run 1"));
    assert!(
        kept(&f, "fix-prompt.md")
            .unwrap()
            .contains("gate-failure.md")
    );

    std::fs::write(profile_file(&f), original).unwrap();
    let healthy = gates(backend(&f, &[CLEAN]), &[], None);
    resume(&f, &healthy, &mut prepared).unwrap();

    let placed = std::fs::read_to_string(prepared.workspace.join(".sbxm-task/gate-failure.md"));
    assert!(placed.unwrap().contains("red run 1"));
    let record = saved(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}
