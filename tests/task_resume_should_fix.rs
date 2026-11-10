//! Issue 510 (decision 306): `task resume --issue N --rounds 1 --should-fix` runs one fix round on
//! a clean task (0 must-fix, no stop) with the review's should-fix findings sized S as its work;
//! M, L and unsized ones are left to be filed. At most one such round per task.

mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_resume::{Options, Target, run};
use sbxm::task::config::ShouldFix;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status, Stopped,
};
use sbxm::task::resume::check_should_fix;

const CLEAN: &str = "Must-fix findings: 0\n";
const MUST: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";
const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";
const S_AND_M: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n\n\
    ### S-2 - b.txt:3 a bigger thing.\n\n**Where:** b.txt:3\n**Size:** M (a module)\n";
const NOT_S: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a bigger thing.\n\n**Where:** a.txt:2\n**Size:** L (a format)\n\n\
    ### S-2 - b.txt:3 no size.\n\n**Where:** b.txt:3\n";

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

/// The worker, the reviewer playing `reviews`, and every `review.md` a worker run found in its
/// clone, in order.
fn backend(f: &Fixture, reviews: &[&str]) -> (FakeBackend, Arc<Mutex<Vec<String>>>) {
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
    let seen = Arc::new(Mutex::new(Vec::new()));
    let given = seen.clone();
    let clone = f.env.base_dir().join("tasks").join("issue-41");
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if !sandbox.contains("-review-") && spec.argv.iter().any(|a| a == "claude") {
                let text =
                    fs::read_to_string(clone.join(".sbxm-task/review.md")).unwrap_or_default();
                given.lock().unwrap().push(text);
            }
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    (backend, seen)
}

/// Issue 41 worked and reviewed once, with `review`: a clean one stops `ready` at round 0.
fn reviewed(f: &Fixture, review: &str) {
    let (first, _) = backend(f, &[review]);
    let mut prepared = worked_task(f, &first);
    let env = sbxm::task::pipeline::TaskEnv {
        progress: &sbxm::task::pipeline::quiet,
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &first,
        probe: &Gone,
        host: &FakeHostRunner::default(),
        sleep: &std::thread::sleep,
        scoped_secrets: &[],
        disk: Box::leak(Box::default()),
        identity: None,
        clone_source: None,
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
}

fn task_json(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")
}

fn resume(f: &Fixture, rounds: Option<u32>, backend: &FakeBackend) -> anyhow::Result<String> {
    let opts = Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Issue(41),
        rounds,
        should_fix: true,
        disk: Default::default(),
        identity: None,
        forget_ambiguous: false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    run(
        &f.env.config_dir(),
        &opts,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn should_fix_defaults_to_file_and_reads_fix() {
    let unset = fixture_with("[sandbox]\nprofile = \"default\"\n[gates]\nsandbox = []\n");
    assert_eq!(unset.config.task_loop.should_fix, ShouldFix::File);
    let file = fixture_with(
        "[sandbox]\nprofile = \"default\"\n[gates]\nsandbox = []\n\n[loop]\nshould_fix = \"file\"\n",
    );
    assert_eq!(file.config.task_loop.should_fix, ShouldFix::File);
    let fix = fixture_with(
        "[sandbox]\nprofile = \"default\"\n[gates]\nsandbox = []\n\n[loop]\nshould_fix = \"fix\"\n",
    );
    assert_eq!(fix.config.task_loop.should_fix, ShouldFix::Fix);
}

#[test]
fn an_unknown_should_fix_value_is_refused_naming_the_values() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n[gates]\nsandbox = []\n\n[loop]\nshould_fix = \"later\"\n",
    )
    .unwrap();
    let err = format!(
        "{:#}",
        sbxm::task::config::TaskConfig::load(dir.path()).unwrap_err()
    );
    assert!(err.contains("later"), "{err}");
    assert!(err.contains("file") && err.contains("fix"), "{err}");
}

#[test]
fn this_repo_sets_should_fix_to_fix_for_the_trial() {
    let config =
        sbxm::task::config::TaskConfig::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
    assert_eq!(config.task_loop.should_fix, ShouldFix::Fix);
}

#[test]
fn one_s_should_fix_gets_one_round_and_the_task_ends_ready_without_a_stop() {
    let f = config();
    reviewed(&f, S_ONE);
    let (backend, seen) = backend(&f, &[CLEAN, CLEAN]);

    let out = resume(&f, Some(1), &backend).unwrap();

    assert!(out.contains("should-fix round"), "{out}");
    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(saved.stage, Stage::Ready);
    assert_eq!(saved.round, 1, "exactly one fix round");
    assert_eq!(saved.stopped, None);
    assert!(saved.should_fix_round);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one worker run: {seen:?}");
    assert!(seen[0].contains("S-1"), "{}", seen[0]);
}

#[test]
fn with_an_s_and_an_m_the_round_is_given_the_s_only() {
    let f = config();
    reviewed(&f, S_AND_M);
    let (backend, seen) = backend(&f, &[CLEAN, CLEAN]);

    resume(&f, Some(1), &backend).unwrap();

    let seen = seen.lock().unwrap();
    assert!(seen[0].contains("S-1"), "{}", seen[0]);
    assert!(!seen[0].contains("S-2"), "{}", seen[0]);
    assert!(!seen[0].contains("a bigger thing"), "{}", seen[0]);
    // The task's own review.md is the reviewer's, untouched by the round's input.
    let meta = record::task_dir(&f.env.base_dir(), "issue-41");
    assert!(
        fs::read_to_string(meta.join("review-1.md"))
            .unwrap()
            .contains("S-2")
    );
}

#[test]
fn a_task_without_an_s_should_fix_is_refused_naming_why_and_left_as_it_was() {
    let f = config();
    reviewed(&f, NOT_S);
    let before = fs::read_to_string(task_json(&f)).unwrap();
    let (backend, seen) = backend(&f, &[CLEAN]);

    let err = format!("{:#}", resume(&f, Some(1), &backend).unwrap_err());

    assert!(err.contains("no should-fix finding sized S"), "{err}");
    assert!(err.contains("task file-findings"), "{err}");
    assert_eq!(fs::read_to_string(task_json(&f)).unwrap(), before);
    assert!(seen.lock().unwrap().is_empty());
    assert!(backend.execs().is_empty(), "{:?}", backend.execs());
    assert!(backend.creates().is_empty());
}

#[test]
fn a_task_with_no_findings_at_all_is_refused() {
    let f = config();
    reviewed(&f, CLEAN);
    let (backend, _) = backend(&f, &[CLEAN]);

    let err = format!("{:#}", resume(&f, Some(1), &backend).unwrap_err());

    assert!(err.contains("no should-fix finding sized S"), "{err}");
}

#[test]
fn a_task_that_had_its_should_fix_round_never_gets_a_second() {
    let f = config();
    reviewed(&f, S_ONE);
    let (first, _) = backend(&f, &[S_ONE, S_ONE]);
    resume(&f, Some(1), &first).unwrap();
    let before = fs::read_to_string(task_json(&f)).unwrap();
    let (again, seen) = backend(&f, &[CLEAN]);

    let err = format!("{:#}", resume(&f, Some(1), &again).unwrap_err());

    assert!(err.contains("already had its should-fix round"), "{err}");
    assert_eq!(fs::read_to_string(task_json(&f)).unwrap(), before);
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn the_rounds_new_must_fix_is_a_work_stop() {
    let f = config();
    reviewed(&f, S_ONE);
    let (backend, _) = backend(&f, &[MUST, MUST, MUST]);

    resume(&f, Some(1), &backend).unwrap();

    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(saved.stage, Stage::Ready);
    assert_eq!(saved.round, 1, "no further fix round");
    assert_eq!(saved.stopped, Some(Stopped::RoundsExhausted));
}

#[test]
fn a_task_with_must_fix_left_is_refused() {
    let f = config();
    reviewed(&f, MUST);
    // One fix round ran in `reviewed`, the budget is used: stopped with must-fix left.
    let before = fs::read_to_string(task_json(&f)).unwrap();
    let (backend, _) = backend(&f, &[CLEAN]);

    let err = format!("{:#}", resume(&f, Some(1), &backend).unwrap_err());

    assert!(err.contains("is for a clean task"), "{err}");
    assert_eq!(fs::read_to_string(task_json(&f)).unwrap(), before);
}

#[test]
fn should_fix_without_rounds_is_refused() {
    let f = config();
    reviewed(&f, S_ONE);
    let (backend, _) = backend(&f, &[CLEAN]);

    let err = format!("{:#}", resume(&f, None, &backend).unwrap_err());

    assert!(err.contains("--rounds 1"), "{err}");
}

/// Review M-1: the should-fix round is one round (answer 1), so its new must-fix is a work stop
/// (answer 3); a larger budget would let a further round fix it instead.
#[test]
fn should_fix_with_a_budget_other_than_one_is_refused_and_touches_nothing() {
    for rounds in [0, 2, u32::MAX] {
        let f = config();
        reviewed(&f, S_ONE);
        let before = fs::read_to_string(task_json(&f)).unwrap();
        let (backend, seen) = backend(&f, &[MUST, CLEAN, CLEAN]);

        let err = format!("{:#}", resume(&f, Some(rounds), &backend).unwrap_err());

        assert!(err.contains("--rounds 1"), "{rounds}: {err}");
        assert_eq!(
            fs::read_to_string(task_json(&f)).unwrap(),
            before,
            "{rounds}"
        );
        assert!(seen.lock().unwrap().is_empty(), "{rounds}");
        assert!(
            backend.execs().is_empty(),
            "{rounds}: {:?}",
            backend.execs()
        );
        assert!(backend.creates().is_empty(), "{rounds}");
    }
}

/// The same budget rule for a spec task and a `task fix --pr` task, whose checks are the same
/// function: refused on the budget before `review.md` is even read.
#[test]
fn the_one_round_rule_holds_for_spec_and_fix_tasks_too() {
    let dir = tempfile::tempdir().unwrap();
    for kind in [Kind::Spec, Kind::Fix, Kind::Issue] {
        let mut task = Record::new(
            &NewTask {
                kind,
                number: 9,
                repo: "o/r",
                title: "t",
                base: "main",
                branch: "b",
                config_hash: "h",
                id: Some("t-9"),
            },
            0,
            Process::new(1, 0),
        );
        task.stage = Stage::Ready;
        task.status = Status::Ok;
        for rounds in [2, u32::MAX] {
            let err = format!(
                "{:#}",
                check_should_fix(&task, dir.path(), Some(rounds)).unwrap_err()
            );
            assert!(
                err.contains("--rounds 1 --should-fix"),
                "{kind:?} {rounds}: {err}"
            );
        }
        // Budget 1 gets past the budget rule: refused only for the missing review.
        let err = format!(
            "{:#}",
            check_should_fix(&task, dir.path(), Some(1)).unwrap_err()
        );
        assert!(err.contains("review"), "{kind:?}: {err}");
        assert!(!err.contains("--rounds 1"), "{kind:?}: {err}");
    }
}

#[test]
fn the_cli_requires_rounds_with_should_fix() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "resume", "--issue", "41", "--should-fix"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--rounds"), "{stderr}");
}

#[test]
fn a_failed_should_fix_round_retried_by_a_plain_resume_is_given_the_s_only_again() {
    let f = config();
    reviewed(&f, S_AND_M);
    let (failing, _) = backend(&f, &[CLEAN]);
    let failing = failing.with_failing_exec_matching("claude");
    assert!(resume(&f, Some(1), &failing).is_err());
    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(saved.stage, Stage::Fixing);
    assert!(saved.should_fix_round);
    let (retry, seen) = backend(&f, &[CLEAN, CLEAN]);

    let opts = Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Issue(41),
        rounds: None,
        should_fix: false,
        disk: Default::default(),
        identity: None,
        forget_ambiguous: false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    run(
        &f.env.config_dir(),
        &opts,
        &retry,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )
    .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].contains("S-1"), "{}", seen[0]);
    assert!(!seen[0].contains("S-2"), "{}", seen[0]);
    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(saved.stage, Stage::Ready);
    assert_eq!(saved.stopped, None);
}
