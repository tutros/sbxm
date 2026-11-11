//! Issue 570: `task loop --stop` (or a first Ctrl-C) while the loop's next task waits for the
//! machine's sandbox slot. The real steps run `task run`, whose slot wait polls the loop's stop
//! request: the ticket is given up, nothing is created, the issue keeps its `ready-for-worker`
//! label and the loop ends "stopped on request" with exit 0.

mod common;

use std::fs;
use std::sync::Arc;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, issue_text, ok, open_issue,
    play_reviews, play_tasks, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_loop::{self, Env, Options, Steps, StopFlag};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record;

const REPO: &str = "o/r";
const ISSUE: u32 = 550;

fn fixture() -> Fixture {
    fixture_with("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n")
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(ISSUE))
        .with_open_issues(vec![open_issue(ISSUE, &["ready-for-worker", "size:S"], "")])
}

struct Ended {
    result: anyhow::Result<()>,
    out: String,
    gh: FakeGitHub,
}

/// One loop over the labelled queue, the slot held by another live command; `while_waiting`
/// runs at the slot wait's first pause, given the config dir and the loop's Ctrl-C flag.
fn run_loop(
    f: &Fixture,
    while_waiting: impl Fn(&std::path::Path, &StopFlag) + Send + Sync + 'static,
) -> Ended {
    let config_dir = f.env.config_dir();
    let flag = Arc::new(StopFlag::default());
    let mut disk = common::slot::full(&f.env, false);
    let (pressed, dir) = (Arc::clone(&flag), config_dir.clone());
    let pauses = std::sync::atomic::AtomicU32::new(0);
    disk.slot.sleep = Arc::new(move |_| {
        let n = pauses.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(n < 3, "the wait went on after the stop");
        if n == 0 {
            while_waiting(&dir, &pressed);
        }
    });
    let gh = github();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let host = FakeHostRunner::default();
    let repo_root = f.env.tmp.path().join("target-repo");
    let steps = task_loop::TaskSteps {
        config_dir: &config_dir,
        repo_root: repo_root.clone(),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk,
        stop: Some(Arc::clone(&flag)),
    };
    let env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_| panic!("the loop itself never waits here"),
        stop_requested: &|| flag.requested(),
    };
    let mut out = Vec::new();
    let result = task_loop::run(
        &config_dir,
        &Options {
            repo_root,
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        &steps,
        &gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );
    Ended {
        result,
        out: String::from_utf8(out).unwrap(),
        gh,
    }
}

/// Stopped while waiting: exit 0, no task record, the label kept, the slot still the holder's and
/// no ticket left in line.
fn assert_stopped_creating_nothing(f: &Fixture, ended: &Ended) {
    assert!(ended.result.is_ok(), "{:?}", ended.result);
    assert_eq!(task_loop::exit_code(&ended.result), 0);
    assert!(!ended.out.contains("Starting issue-550"), "{}", ended.out);
    assert!(
        ended.out.lines().any(|l| l == "stopped on request"),
        "{}",
        ended.out
    );
    let task_json =
        record::task_dir(&f.env.base_dir(), &format!("issue-{ISSUE}")).join("task.json");
    assert!(!task_json.exists(), "nothing created");
    assert!(
        !ended
            .gh
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(..))),
        "the label is kept: {:?}",
        ended.gh.calls()
    );
    assert!(
        common::slot::queue(&f.env).is_empty(),
        "the ticket is given up"
    );
    let holder = common::slot::holder(&f.env).expect("the slot is still held");
    assert_eq!(holder.process.pid, common::slot::HOLDER_PID);
    assert!(
        !fs::exists(task_loop::stop_path(&f.env.base_dir())).unwrap(),
        "the stop request is removed as the loop stops"
    );
}

#[test]
fn task_loop_stop_during_the_slot_wait_ends_the_loop_creating_nothing() {
    let f = fixture();

    let ended = run_loop(&f, |config_dir, _| {
        task_loop::stop(config_dir, &Probe, &mut Vec::new()).unwrap();
    });

    assert_stopped_creating_nothing(&f, &ended);
}

#[test]
fn a_first_ctrl_c_during_the_slot_wait_ends_the_loop_creating_nothing() {
    let f = fixture();

    let ended = run_loop(&f, |_, flag| {
        assert_eq!(flag.press(), task_loop::Press::First);
    });

    assert_stopped_creating_nothing(&f, &ended);
}

#[test]
fn a_stop_as_the_slot_frees_gives_it_back_and_ends_the_loop_creating_nothing() {
    let f = fixture();
    let slot = common::slot::slot_file(&f.env);

    let ended = run_loop(&f, move |config_dir, _| {
        // The holder ends and the stop comes in the same pause: the next look takes the slot.
        fs::remove_file(&slot).unwrap();
        task_loop::stop(config_dir, &Probe, &mut Vec::new()).unwrap();
    });

    assert!(ended.result.is_ok(), "{:?}", ended.result);
    assert!(!ended.out.contains("Starting issue-550"), "{}", ended.out);
    assert!(
        ended.out.lines().any(|l| l == "stopped on request"),
        "{}",
        ended.out
    );
    let task_json =
        record::task_dir(&f.env.base_dir(), &format!("issue-{ISSUE}")).join("task.json");
    assert!(!task_json.exists(), "nothing created");
    assert!(
        common::slot::holder(&f.env).is_none(),
        "the slot is free again"
    );
    assert!(common::slot::queue(&f.env).is_empty());
    assert!(
        !ended
            .gh
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(..))),
        "the label is kept"
    );
}

// --- a task's later phases ---

const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";

/// Issue 41 worked, and with `review` reviewed too (a clean one with a should-fix finding sized S
/// stops `ready`, its should-fix round due); its record owned by no process.
fn worked(review: Option<&str>) -> Fixture {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = []\n\n[reviewer]\nharness = \"codex\"\n",
    );
    let reviews = review.map(|r| vec![r.to_owned()]).unwrap_or_default();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
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
            let reviewer = play_reviews(&f.env.base_dir(), "issue-41", reviews);
            move |sandbox, spec| {
                worker(sandbox, spec);
                reviewer(sandbox, spec);
            }
        });
    let mut prepared = worked_task(&f, &backend);
    if review.is_some() {
        let env = sbxm::task::pipeline::TaskEnv {
            progress: &sbxm::task::pipeline::quiet,
            config_dir: &f.env.config_dir(),
            config: &f.config,
            backend: &backend,
            probe: &Probe,
            host: &FakeHostRunner::default(),
            sleep: &std::thread::sleep,
            scoped_secrets: &[],
            disk: Box::leak(Box::default()),
            identity: None,
            clone_source: None,
        };
        sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
    }
    let dir = record::task_dir(&f.env.base_dir(), "issue-41");
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.process = None;
    record::write(&dir, &rec).unwrap();
    f
}

/// A phase of issue 41 run by the real steps, the slot held by another live command until the
/// wait's first pause, at which a first Ctrl-C comes in. Returns the phase's result and how
/// many pauses the wait took.
fn phase_stopped_while_waiting(
    f: &Fixture,
    phase: impl Fn(&task_loop::TaskSteps) -> anyhow::Result<()>,
) -> (anyhow::Result<()>, u32) {
    let flag = Arc::new(StopFlag::default());
    let mut disk = common::slot::full(&f.env, false);
    let pauses = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let (seen, pressed, slot) = (
        Arc::clone(&pauses),
        Arc::clone(&flag),
        common::slot::slot_file(&f.env),
    );
    disk.slot.sleep = Arc::new(move |_| {
        let n = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(n < 3, "the wait went on");
        if n == 0 {
            pressed.press();
            // The holder ends too: a wait that goes on takes the slot at its next look.
            fs::remove_file(&slot).unwrap();
        }
    });
    let gh = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41));
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE));
    let host = FakeHostRunner::default();
    let config_dir = f.env.config_dir();
    let steps = task_loop::TaskSteps {
        config_dir: &config_dir,
        repo_root: f.env.tmp.path().join("target-repo"),
        repo: Some(REPO.into()),
        backend: &backend,
        github: &gh,
        probe: &Probe,
        host: &host,
        disk,
        stop: Some(flag),
    };
    let result = phase(&steps);
    (result, pauses.load(std::sync::atomic::Ordering::SeqCst))
}

#[test]
fn a_stop_while_a_resume_waits_for_the_slot_ends_it_changing_nothing() {
    let f = worked(None);
    let task_json = record::task_dir(&f.env.base_dir(), "issue-41").join("task.json");
    let before = fs::read_to_string(&task_json).unwrap();

    let (result, pauses) = phase_stopped_while_waiting(&f, |steps| {
        steps.resume(41, &mut Vec::new(), &mut Vec::new())
    });

    let e = result.expect_err("stopped");
    assert!(e.is::<sbxm::slot::Stopped>(), "{e:#}");
    assert_eq!(pauses, 1);
    assert_eq!(
        fs::read_to_string(&task_json).unwrap(),
        before,
        "nothing written"
    );
    assert!(common::slot::holder(&f.env).is_none(), "no slot kept");
    assert!(common::slot::queue(&f.env).is_empty());
}

#[test]
fn a_stop_while_a_should_fix_round_waits_for_the_slot_lets_the_round_run() {
    let f = worked(Some(S_ONE));
    let rec = |f: &Fixture| {
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
    };
    assert!(!rec(&f).should_fix_round, "the round is still due");

    let (result, pauses) = phase_stopped_while_waiting(&f, |steps| {
        steps.should_fix(41, &mut Vec::new(), &mut Vec::new())
    });

    // Part of the task the loop finishes before it stops: it waits on, then runs.
    if let Err(e) = &result {
        assert!(!e.is::<sbxm::slot::Stopped>(), "{e:#}");
    }
    assert_eq!(pauses, 1, "it took the slot at its next look");
    assert!(rec(&f).should_fix_round, "the round ran");
}
