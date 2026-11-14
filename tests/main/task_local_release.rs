//! Issue 341 (decisions of the user, 2026-11-02): a local task removes its worker sandbox when the
//! phase that uses it ends (the worker and its gates; each fix round and its gates), whatever the
//! outcome, before any reviewer sandbox is created, and records the removal in `task.json`'s
//! `sandboxes` list. A later phase that needs it makes it again through `ensure_worker_sandbox`.

use crate::common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, issue_text, ok, play_reviews,
    play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::lifetime::Released;
use sbxm::task::pipeline::{self, Prepared, ReviewReport, TaskReport, Tiers};
use sbxm::task::record::{self, Record, SandboxRole, Stage, Status};

const WORKER: &str = "sbxm-task-issue-41-claude";
const REVIEWER: &str = "sbxm-task-issue-41-review-codex";
const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str =
    "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 does the wrong thing.\n";

fn config(fix_rounds: u32) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = {fix_rounds}\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n"
    ))
}

/// The worker (and every fix round) commits `a.txt`; the n-th review is `reviews[n]`. `sbx ls`
/// lists what was created and not removed since.
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
        .with_live_cloud(&[])
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

/// The gate runs answer red where `reds` says so (by run, from 0), green otherwise.
fn gates(backend: FakeBackend, reds: &'static [bool]) -> FakeBackend {
    let runs = std::sync::atomic::AtomicUsize::new(0);
    backend.with_exec_responder(move |sandbox, spec| {
        let mentions = |s: &str| spec.argv.iter().any(|a| a.contains(s));
        if sandbox != WORKER || !mentions("cargo test") || mentions("prompt") {
            return None;
        }
        let n = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(if reds.get(n) == Some(&true) {
            red()
        } else {
            ok("")
        })
    })
}

fn saved(f: &Fixture) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
}

fn start(f: &Fixture, backend: &FakeBackend) -> TaskReport {
    let github = FakeGitHub::default().with_issue_text(issue_text(41));
    let source = source(f);
    pipeline::start(&ctx(f, &source, backend, &github), &[41])
        .pop()
        .unwrap()
}

fn review(f: &Fixture, backend: &FakeBackend) -> (Prepared, anyhow::Result<ReviewReport>) {
    let github = FakeGitHub::default();
    let source = source(f);
    let mut prepared = Prepared::open(&f.env.base_dir(), "issue-41").unwrap();
    let report = pipeline::review_issue(&ctx(f, &source, backend, &github).env(), &mut prepared);
    (prepared, report)
}

/// The log positions of every entry equal to `entry`.
fn all(backend: &FakeBackend, entry: &str) -> Vec<usize> {
    backend
        .log()
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == entry)
        .map(|(i, _)| i)
        .collect()
}

/// The log positions of the worker sandbox's execs whose argv mentions `needle`.
fn execs(backend: &FakeBackend, needle: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut n = 0;
    let execs = backend.execs();
    for (i, line) in backend.log().iter().enumerate() {
        if !line.starts_with("exec ") {
            continue;
        }
        let (sandbox, spec) = &execs[n];
        n += 1;
        if sandbox == WORKER && spec.argv.iter().any(|a| a.contains(needle)) {
            found.push(i);
        }
    }
    found
}

/// The worker sandbox's lifetime entries in `task.json`: (role, removed).
fn lives(f: &Fixture) -> Vec<(SandboxRole, bool)> {
    saved(f)
        .sandboxes
        .iter()
        .filter(|s| s.name == WORKER)
        .map(|s| (s.role, s.removed_at.is_some()))
        .collect()
}

fn lines(report: &TaskReport) -> Vec<String> {
    report
        .stopped
        .iter()
        .filter_map(|r| match r {
            Released::Line(l) => Some(l.clone()),
            Released::Warning(_) => None,
        })
        .collect()
}

fn warnings(report: &TaskReport) -> Vec<String> {
    report
        .stopped
        .iter()
        .filter_map(|r| match r {
            Released::Warning(w) => Some(w.clone()),
            Released::Line(_) => None,
        })
        .collect()
}

// ---- The worker phase ----

#[test]
fn a_worker_whose_gates_pass_has_its_sandbox_removed_before_the_reviewer_is_created() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);

    let report = start(&f, &backend);

    assert!(report.gates.as_ref().unwrap().as_ref().unwrap().passed);
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(removed.len(), 1, "{:?}", backend.log());
    assert!(
        execs(&backend, "cargo test").last().unwrap() < &removed[0],
        "{:?}",
        backend.log()
    );
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
    assert!(
        lines(&report).contains(&format!("sandbox {WORKER} removed")),
        "{:?}",
        report.stopped
    );
    assert_eq!(saved(&f).status, Status::Passed);

    let (_, reviewed) = review(&f, &backend);

    assert_eq!(reviewed.unwrap().must_fix_left, 0);
    let created = all(&backend, &format!("create {REVIEWER}"));
    assert!(removed[0] < created[0], "{:?}", backend.log());
    // Its gates are current: the review needs no worker sandbox.
    assert_eq!(
        all(&backend, &format!("create {WORKER}")).len(),
        1,
        "{:?}",
        backend.log()
    );
}

#[test]
fn a_worker_whose_gates_fail_has_its_sandbox_removed_and_made_again_for_the_fix_round() {
    let f = config(1);
    // Red after the worker, red again before the review, green after the fix round.
    let backend = gates(backend(&f, &[CLEAN]), &[true, true]);

    let report = start(&f, &backend);

    assert!(!report.gates.as_ref().unwrap().as_ref().unwrap().passed);
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
    let first_rm = all(&backend, &format!("rm {WORKER}"))[0];

    let (prepared, reviewed) = review(&f, &backend);

    let report = reviewed.unwrap();
    assert!(report.fix_ran);
    // Made again for the gates before the review, removed when they fail, and made again for
    // the fix round.
    let created = all(&backend, &format!("create {WORKER}"));
    assert_eq!(created.len(), 3, "{:?}", backend.log());
    assert!(first_rm < created[1]);
    let fix = execs(&backend, "fix-prompt.md");
    assert!(created[2] < fix[0], "{:?}", backend.log());
    assert!(
        prepared
            .events
            .contains(&"issue-41: the worker's sandbox was gone; made it again".to_owned()),
        "{:?}",
        prepared.events
    );
    // Removed again after the fix round's gates, before the reviewer.
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(removed.len(), 3, "{:?}", backend.log());
    assert!(removed[1] < created[2]);
    assert!(execs(&backend, "cargo test").last().unwrap() < &removed[2]);
    assert!(removed[2] < all(&backend, &format!("create {REVIEWER}"))[0]);
    assert!(
        lives(&f).iter().all(|(_, removed)| *removed),
        "{:?}",
        lives(&f)
    );
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn a_failed_worker_has_its_sandbox_removed_and_its_stage_kept() {
    let f = config(1);
    let backend = backend(&f, &[CLEAN]).with_exec_responder(|_, spec| {
        spec.argv.iter().any(|a| a == "claude").then(|| ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        })
    });

    let report = start(&f, &backend);

    assert!(report.gates.is_none());
    assert_eq!(all(&backend, &format!("rm {WORKER}")).len(), 1);
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Working, Status::Failed)
    );
}

#[test]
fn a_timed_out_worker_has_its_sandbox_removed_after_its_gates() {
    let f = config(1);
    let backend = backend(&f, &[CLEAN]).with_exec_responder(|_, spec| {
        spec.argv.iter().any(|a| a == "claude").then(|| ExecOutput {
            stdout: String::new(),
            stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
            exit_code: Some(124),
        })
    });

    let report = start(&f, &backend);

    assert!(report.gates.as_ref().unwrap().as_ref().unwrap().passed);
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(removed.len(), 1, "{:?}", backend.log());
    assert!(execs(&backend, "cargo test").last().unwrap() < &removed[0]);
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
}

#[test]
fn a_removal_that_fails_is_a_warning_and_the_task_goes_on_to_its_review() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]).with_remove_errors(&["`sbx rm` failed: busy"]);

    let report = start(&f, &backend);

    let warned = warnings(&report);
    assert_eq!(warned.len(), 1, "{:?}", report.stopped);
    assert!(
        warned[0].contains(WORKER) && warned[0].contains("busy"),
        "{warned:?}"
    );
    assert!(report.result.is_ok());
    // Still there, so its entry stays open.
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, false)]);

    let (_, reviewed) = review(&f, &backend);

    assert_eq!(reviewed.unwrap().must_fix_left, 0);
    assert_eq!(saved(&f).stage, Stage::Ready);
}

#[test]
fn a_sandbox_already_gone_counts_as_removed_without_a_warning() {
    let f = config(1);
    // Not listed by `sbx ls` (no live list), and `sbx rm` says it doesn't know it.
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
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
        .with_remove_errors(&[&format!("`sbx rm` failed for sandbox {WORKER}: not found")]);

    let report = start(&f, &backend);

    assert!(warnings(&report).is_empty(), "{:?}", report.stopped);
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
}

#[test]
fn uncommitted_changes_in_the_clone_are_reported_before_the_removal() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[])
        .with_exec_output_matching("--porcelain", ok(" M notes.txt\n"));

    let report = start(&f, &backend);

    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    let said = lines(&report);
    let at = said
        .iter()
        .position(|l| l.contains("uncommitted") && l.contains(&workspace.display().to_string()))
        .unwrap_or_else(|| panic!("{said:?}"));
    let removed = said
        .iter()
        .position(|l| l == &format!("sandbox {WORKER} removed"))
        .unwrap();
    assert!(at < removed, "{said:?}");
    assert!(
        execs(&backend, "--porcelain").last().unwrap() < &all(&backend, &format!("rm {WORKER}"))[0]
    );
    // Nothing is deleted from the workspace.
    assert!(workspace.join("a.txt").is_file());
}

// ---- Fix rounds ----

#[test]
fn a_fix_round_has_its_sandbox_removed_after_its_gates_and_a_second_one_makes_it_again() {
    let f = config(2);
    let backend = gates(backend(&f, &[ONE, ONE, CLEAN]), &[]);
    start(&f, &backend);

    let (_, reviewed) = review(&f, &backend);

    let report = reviewed.unwrap();
    assert_eq!(report.must_fix_left, 0);
    let created = all(&backend, &format!("create {WORKER}"));
    let removed = all(&backend, &format!("rm {WORKER}"));
    let fixes = execs(&backend, "fix-prompt.md");
    let reviewers = all(&backend, &format!("create {REVIEWER}"));
    assert_eq!((created.len(), removed.len(), fixes.len()), (3, 3, 2));
    // Worker, removed; fix 1 rebuilt it, removed before review 2; fix 2 rebuilt it again.
    assert!(removed[0] < created[1] && created[1] < fixes[0]);
    assert!(fixes[0] < removed[1] && removed[1] < reviewers[1]);
    assert!(removed[1] < created[2] && created[2] < fixes[1]);
    assert!(fixes[1] < removed[2] && removed[2] < reviewers[2]);
    assert_eq!(
        lives(&f),
        vec![
            (SandboxRole::Worker, true),
            (SandboxRole::Fix, true),
            (SandboxRole::Fix, true)
        ]
    );
}

#[test]
fn a_fix_round_that_fails_has_its_sandbox_removed() {
    let f = config(1);
    let fails = std::sync::atomic::AtomicBool::new(false);
    let backend = backend(&f, &[ONE]).with_exec_responder(move |_, spec| {
        let fix = spec.argv.iter().any(|a| a.contains("fix-prompt.md"));
        (fix && !fails.swap(true, std::sync::atomic::Ordering::SeqCst)).then(|| ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        })
    });
    start(&f, &backend);

    let (_, reviewed) = review(&f, &backend);

    reviewed.unwrap_err();
    let record = saved(&f);
    assert_eq!(
        (record.stage, record.status),
        (Stage::Fixing, Status::Failed)
    );
    assert_eq!(all(&backend, &format!("rm {WORKER}")).len(), 2);
    assert!(
        lives(&f).iter().all(|(_, removed)| *removed),
        "{:?}",
        lives(&f)
    );
}

// ---- Later phases make it again ----

#[test]
fn task_gates_on_a_task_whose_sandbox_was_removed_makes_it_again_and_removes_it_after() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = Prepared::open(&f.env.base_dir(), "issue-41").unwrap();

    let gated = pipeline::gate_on_demand(
        &ctx(&f, &source, &backend, &github).env(),
        &mut prepared,
        Tiers::ALL,
    )
    .unwrap();

    assert!(gated.passed);
    let created = all(&backend, &format!("create {WORKER}"));
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(
        (created.len(), removed.len()),
        (2, 2),
        "{:?}",
        backend.log()
    );
    let gate = *execs(&backend, "cargo test").last().unwrap();
    assert!(removed[0] < created[1] && created[1] < gate && gate < removed[1]);
    assert!(
        lives(&f).iter().all(|(_, removed)| *removed),
        "{:?}",
        lives(&f)
    );
}

struct Gone;

impl sbxm::task::record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

#[test]
fn task_resume_on_a_task_whose_sandbox_was_removed_makes_it_again() {
    let f = config(1);
    // The gates fail after the worker; the task is resumed, which reruns them.
    let backend = gates(backend(&f, &[CLEAN]), &[true]);
    start(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = Prepared::open(&f.env.base_dir(), "issue-41").unwrap();
    let ctx = ctx(&f, &source, &backend, &github);
    let env = pipeline::TaskEnv {
        progress: &sbxm::task::pipeline::quiet,
        probe: &Gone,
        ..ctx.env()
    };

    let resumed = sbxm::task::resume::resume(&env, &mut prepared, None).unwrap();

    assert!(resumed.sandbox_remade);
    let created = all(&backend, &format!("create {WORKER}"));
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(created.len(), 2, "{:?}", backend.log());
    assert!(removed[0] < created[1]);
    let gate = *execs(&backend, "cargo test").last().unwrap();
    assert!(created[1] < gate && gate < *removed.last().unwrap());
    assert!(*removed.last().unwrap() < all(&backend, &format!("create {REVIEWER}"))[0]);
    assert_eq!(saved(&f).stage, Stage::Ready);
    assert!(
        lives(&f).iter().all(|(_, removed)| *removed),
        "{:?}",
        lives(&f)
    );
}

#[test]
fn task_resume_of_a_failed_review_whose_sandbox_was_removed_does_not_make_it_again() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    // The review failed after its gates passed: it runs again, and needs no worker sandbox.
    let mut task = saved(&f);
    task.stage = Stage::Reviewing;
    task.status = Status::Failed;
    record::write(&record::task_dir(&f.env.base_dir(), "issue-41"), &task).unwrap();
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = Prepared::open(&f.env.base_dir(), "issue-41").unwrap();
    let ctx = ctx(&f, &source, &backend, &github);
    let env = pipeline::TaskEnv {
        progress: &sbxm::task::pipeline::quiet,
        probe: &Gone,
        ..ctx.env()
    };

    let resumed = sbxm::task::resume::resume(&env, &mut prepared, None).unwrap();

    assert!(!resumed.sandbox_remade);
    assert_eq!(
        all(&backend, &format!("create {WORKER}")).len(),
        1,
        "{:?}",
        backend.log()
    );
    assert_eq!(saved(&f).stage, Stage::Ready);
}

// ---- Review findings (issue 341, review round 1) ----

/// M-3: a fix round's gates that fail end that round's phase: the sandbox goes before the next
/// fix round makes it again, as it does before a review.
#[test]
fn each_fix_round_fed_by_failing_gates_removes_its_sandbox_before_the_next_one() {
    let f = config(2);
    // Red after the worker, before the review and after fix round 1; green after fix round 2.
    let backend = gates(backend(&f, &[CLEAN]), &[true, true, true]);
    start(&f, &backend);

    let (_, reviewed) = review(&f, &backend);

    assert!(reviewed.unwrap().fix_ran);
    let created = all(&backend, &format!("create {WORKER}"));
    let removed = all(&backend, &format!("rm {WORKER}"));
    let fixes = execs(&backend, "fix-prompt.md");
    assert_eq!(fixes.len(), 2, "{:?}", backend.log());
    let gate_after_fix_1 = *execs(&backend, "cargo test")
        .iter()
        .find(|&&g| fixes[0] < g)
        .unwrap();
    let rm = removed
        .iter()
        .find(|&&r| gate_after_fix_1 < r)
        .copied()
        .unwrap_or(usize::MAX);
    let made = created
        .iter()
        .find(|&&c| rm < c)
        .copied()
        .unwrap_or(usize::MAX);
    assert!(
        rm < made && made < fixes[1],
        "gates {gate_after_fix_1}, rm {rm}, create {made}, fix {}: {:?}",
        fixes[1],
        backend.log()
    );
    assert!(
        lives(&f).iter().all(|(_, removed)| *removed),
        "{:?}",
        lives(&f)
    );
    assert!(removed.last().unwrap() < &all(&backend, &format!("create {REVIEWER}"))[0]);
}

/// M-2: a local task saved before issue 341 has no lifetime entries; its worker sandbox, still
/// there, is taken as the task's and removed before the reviewer is created.
#[test]
fn a_task_saved_without_lifetime_entries_still_has_its_worker_sandbox_removed() {
    let f = config(1);
    // The removal at the end of the worker's phase fails, so the sandbox stays.
    let backend = gates(backend(&f, &[CLEAN]), &[]).with_remove_errors(&["`sbx rm` failed: busy"]);
    start(&f, &backend);
    let mut task = saved(&f);
    task.sandboxes.clear();
    record::write(&record::task_dir(&f.env.base_dir(), "issue-41"), &task).unwrap();

    let (_, reviewed) = review(&f, &backend);

    assert_eq!(reviewed.unwrap().must_fix_left, 0);
    let removed = all(&backend, &format!("rm {WORKER}"));
    assert_eq!(removed.len(), 2, "{:?}", backend.log());
    assert!(removed[1] < all(&backend, &format!("create {REVIEWER}"))[0]);
    assert_eq!(lives(&f), vec![(SandboxRole::Worker, true)]);
}

/// M-2: a task saved without lifetime entries whose sandbox is gone records nothing and removes
/// nothing.
#[test]
fn a_task_saved_without_lifetime_entries_and_no_sandbox_removes_nothing() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    let mut task = saved(&f);
    task.sandboxes.clear();
    record::write(&record::task_dir(&f.env.base_dir(), "issue-41"), &task).unwrap();

    let (_, reviewed) = review(&f, &backend);

    assert_eq!(reviewed.unwrap().must_fix_left, 0);
    assert_eq!(all(&backend, &format!("rm {WORKER}")).len(), 1);
    assert!(lives(&f).is_empty(), "{:?}", lives(&f));
}

fn task_gates_cmd(
    f: &Fixture,
    backend: &FakeBackend,
    disk: sbxm::disk::Guard,
) -> anyhow::Result<()> {
    sbxm::commands::task_gates::run(
        &f.env.config_dir(),
        &sbxm::commands::task_gates::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issue: 41,
            tiers: Tiers::ALL,
            dry_run: false,
            identity: None,
            disk,
        },
        backend,
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
    )
}

/// M-1: `task gates` that would make the worker's sandbox again checks the free disk first
/// (decision 194), and refuses before any `sbx` call or write.
#[test]
fn task_gates_that_would_make_the_sandbox_again_refuse_on_low_disk() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    let before =
        std::fs::read(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap();
    let calls = backend.log().len();

    let err = task_gates_cmd(&f, &backend, common::low_disk(&f.env)).unwrap_err();

    assert!(format!("{err:#}").starts_with(common::LOW_DISK), "{err:#}");
    assert_eq!(backend.log().len(), calls, "{:?}", backend.log());
    assert_eq!(
        std::fs::read(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap(),
        before
    );
}

/// Issue 504: `task gates` that would make the worker's sandbox again takes the machine's sandbox
/// slot (decision 304); with `--no-wait` and no free slot it refuses before any `sbx` call.
#[test]
fn task_gates_that_would_make_the_sandbox_again_refuse_with_no_free_slot_and_no_wait() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    let calls = backend.log().len();

    let err = task_gates_cmd(&f, &backend, common::slot::full(&f.env, true)).unwrap_err();

    assert!(
        format!("{err:#}").starts_with(&common::slot::refusal()),
        "{err:#}"
    );
    assert_eq!(backend.log().len(), calls, "{:?}", backend.log());
}

/// M-1: `--ignore-disk` lets it make the sandbox again.
#[test]
fn task_gates_with_ignore_disk_make_the_sandbox_again_on_low_disk() {
    let f = config(1);
    let backend = gates(backend(&f, &[CLEAN]), &[]);
    start(&f, &backend);
    let disk = sbxm::disk::Guard {
        ignore: true,
        ..common::low_disk(&f.env)
    };

    task_gates_cmd(&f, &backend, disk).unwrap();

    assert_eq!(all(&backend, &format!("create {WORKER}")).len(), 2);
}
