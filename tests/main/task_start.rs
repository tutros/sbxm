//! M2b slice 6, step 4: starting several picks at once (`--workers N`, decision 144): each task
//! gets its own folders, sandbox and thread, and one failing does not stop the others.

use crate::common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, backend, ctx, fixture, issue_text, ok, play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::headless::RunStatus;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo;

fn what() -> Play {
    Play {
        commits: vec!["a.txt".into()],
        result_md: Some(b"done\n".to_vec()),
        bundle_bytes: None,
    }
}

fn playing(f: &Fixture) -> FakeBackend {
    playing_printing(f, ok(CLAUDE_DONE)).0
}

/// What each sandbox's agent command printed, kept for a failure message (issue 693): the task
/// keeps the worker's stdout as its transcript but drops its stderr when it succeeds.
type Streams = Arc<Mutex<Vec<(String, ExecOutput)>>>;

/// Like [`playing`], with the agent command printing `printed`; also returns what it printed.
fn playing_printing(f: &Fixture, printed: ExecOutput) -> (FakeBackend, Streams) {
    let streams = Streams::default();
    let kept = Arc::clone(&streams);
    let backend = backend()
        .with_exec_responder(move |sandbox, spec| {
            spec.argv.iter().any(|a| a.contains("claude")).then(|| {
                kept.lock()
                    .unwrap()
                    .push((sandbox.to_owned(), printed.clone()));
                printed.clone()
            })
        })
        .with_exec_hook(play_tasks(&f.env.base_dir(), "main", what()));
    (backend, streams)
}

fn github_with(numbers: &[u32]) -> FakeGitHub {
    numbers.iter().fold(FakeGitHub::default(), |gh, n| {
        gh.with_issue_text(issue_text(*n))
    })
}

fn meta(f: &Fixture, number: u32) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), &format!("issue-{number}"))
}

/// What one pick's start said (its warnings, events, result and gates, which hold the stdout and
/// stderr of a step that failed), what its agent command printed, and its task's `run.log` and
/// `gates.log`, for a failure message (issue 693).
fn said(f: &Fixture, report: &pipeline::TaskReport, streams: &Streams) -> String {
    let meta = meta(f, report.number);
    let sandbox = format!("sbxm-task-issue-{}-claude", report.number);
    let printed: String = streams
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| *name == sandbox)
        .map(|(_, out)| {
            format!(
                "agent command in {sandbox}: exit {:?}\nstdout (last 40 lines):\n{}\n\
                 stderr (last 40 lines):\n{}\n",
                out.exit_code,
                common::text_tail(&out.stdout, 40),
                common::text_tail(&out.stderr, 40),
            )
        })
        .collect();
    let printed = if printed.is_empty() {
        format!("no agent command ran in {sandbox}\n")
    } else {
        printed
    };
    format!(
        "warnings: {:?}\nevents: {:?}\nresult: {:#?}\ngates: {:#?}\n{printed}\
         run.log (last 40 lines):\n{}\ngates.log (last 40 lines):\n{}",
        report.warnings,
        report.events,
        report.result,
        report.gates,
        common::file_tail(&meta.join("run.log"), 40),
        common::file_tail(&meta.join("gates.log"), 40),
    )
}

#[test]
fn each_pick_gets_its_own_task_sandbox_and_collected_commits() {
    let f = fixture();
    let (backend, streams) = playing_printing(&f, ok(CLAUDE_DONE));
    let github = github_with(&[1, 2, 3]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2, 3]);

    assert_eq!(
        reports.iter().map(|r| r.number).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for report in &reports {
        let n = report.number;
        // Failed once under load (#104): each message holds what the command said and did.
        let said = said(&f, report, &streams);
        let worked = report
            .result
            .as_ref()
            .unwrap_or_else(|e| panic!("#{n}: {e:#}\n{said}"));
        assert_eq!(worked.status, RunStatus::Completed, "#{n}\n{said}");
        assert_eq!(worked.commits, 1, "#{n}\n{said}");
        let task = record::read(&meta(&f, n).join("task.json"))
            .unwrap_or_else(|e| panic!("#{n}: {e:#}\n{said}"));
        // The worker finished and its gates (the fixture's defaults, in the fake sandbox) passed.
        assert_eq!(
            (task.stage, task.status),
            (Stage::Gating, Status::Passed),
            "#{n}\n{said}"
        );
        assert_eq!(
            task.worker
                .unwrap_or_else(|| panic!("#{n}: no worker in task.json\n{said}"))
                .sandbox,
            format!("sbxm-task-issue-{n}-claude"),
            "#{n}\n{said}"
        );
        // A git step's error holds git's stderr.
        let ahead =
            repo::commits_ahead(&meta(&f, n).join("repo.git"), "main", &format!("issue-{n}"))
                .unwrap_or_else(|e| panic!("#{n}: git: {e:#}\n{said}"));
        assert_eq!(ahead, 1, "#{n}\n{said}");
        assert_eq!(
            fs::read_to_string(meta(&f, n).join("result.md"))
                .unwrap_or_else(|e| panic!("#{n}: result.md: {e}\n{said}")),
            "done\n",
            "#{n}\n{said}"
        );
    }
    let mut created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    created.sort();
    assert_eq!(
        created,
        [
            "sbxm-task-issue-1-claude",
            "sbxm-task-issue-2-claude",
            "sbxm-task-issue-3-claude"
        ]
    );
}

// ---- issue 693: the failure message of the test above ----------------------------------------

#[test]
fn each_pick_failure_message_holds_the_workers_stdout_and_stderr() {
    let f = fixture();
    let printed = ExecOutput {
        stdout: CLAUDE_DONE.replace("PONG", "STDOUT_MARKER_693"),
        stderr: "STDERR_MARKER_693\n".into(),
        exit_code: Some(0),
    };
    let (backend, streams) = playing_printing(&f, printed);
    let github = github_with(&[1, 2]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2]);

    for report in &reports {
        let said = said(&f, report, &streams);
        assert!(said.contains("STDOUT_MARKER_693"), "{said}");
        assert!(said.contains("STDERR_MARKER_693"), "{said}");
    }
}

#[test]
fn the_issue_text_comes_from_github_once_per_pick() {
    let f = fixture();
    let (backend, github) = (playing(&f), github_with(&[4, 5]));
    let source = source(&f);

    pipeline::start(&ctx(&f, &source, &backend, &github), &[4, 5]);

    let calls = github.calls();
    assert!(calls.contains(&GhCall::Issue("o/r".into(), 4)), "{calls:?}");
    assert!(calls.contains(&GhCall::Issue("o/r".into(), 5)), "{calls:?}");
    assert_eq!(calls.len(), 2, "{calls:?}");
}

#[test]
fn one_pick_failing_does_not_stop_the_others() {
    let f = fixture();
    let backend = playing(&f).with_failing_create_for("sbxm-task-issue-2-claude");
    let github = github_with(&[1, 2, 3]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2, 3]);

    assert!(reports[0].result.is_ok());
    let message = format!("{:#}", reports[1].result.as_ref().unwrap_err());
    assert!(message.contains("sbxm-task-issue-2-claude"), "{message}");
    assert!(reports[2].result.is_ok());
    assert!(!meta(&f, 2).exists(), "the failed pick left no task folder");
    assert!(meta(&f, 1).join("task.json").is_file() && meta(&f, 3).join("task.json").is_file());
}

#[test]
fn an_issue_github_cannot_show_fails_only_that_pick() {
    let f = fixture();
    let backend = playing(&f);
    let github = github_with(&[1]);
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 9]);

    assert!(reports[0].result.is_ok());
    let message = format!("{:#}", reports[1].result.as_ref().unwrap_err());
    assert!(message.contains("issue 9"), "{message}");
    assert!(!meta(&f, 9).exists());
}

#[test]
fn the_picks_really_run_at_the_same_time() {
    let f = fixture();
    // Both agents' commands wait at a gate until two of them are waiting: only parallel
    // execution can get past it; a sequential start would hang, so the gate has a deadline.
    let (backend, gate) = playing(&f).with_exec_gate();
    let github = github_with(&[1, 2]);
    let source = source(&f);
    let releaser = std::thread::spawn({
        let gate = gate.clone();
        move || {
            gate.wait_for_blocked(2);
            gate.open();
        }
    });

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2]);

    releaser.join().unwrap();
    assert!(reports.iter().all(|r| r.result.is_ok()));
}

/// Pauses the first process probe that runs while `meta` is reserved and not yet locked: the
/// start that reserved it is then between its `create_dir` and its lock (issue 58 review M-2).
struct PauseAfterReservation {
    meta: std::path::PathBuf,
    paused: std::sync::atomic::AtomicBool,
    arrived: std::sync::Barrier,
    resume: std::sync::Barrier,
}

impl record::ProcessProbe for PauseAfterReservation {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        if self.meta.is_dir()
            && !self.meta.join("lock.json").exists()
            && !self.paused.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.arrived.wait();
            self.resume.wait();
        }
        Some(0)
    }
}

#[test]
fn a_start_that_loses_the_lock_leaves_the_winners_folder_alone() {
    let f = fixture();
    let (backend, github) = (playing(&f), github_with(&[41]));
    let source = source(&f);
    let meta = meta(&f, 41);
    let pausing = PauseAfterReservation {
        meta: meta.clone(),
        paused: false.into(),
        arrived: std::sync::Barrier::new(2),
        resume: std::sync::Barrier::new(2),
    };
    let plain = ctx(&f, &source, &backend, &github);
    let paused_ctx = pipeline::Ctx {
        probe: &pausing,
        ..ctx(&f, &source, &backend, &github)
    };

    std::thread::scope(|s| {
        let a = s.spawn(|| pipeline::prepare(&paused_ctx, &issue_text(41)));
        pausing.arrived.wait();
        // While start A is paused: its unfinished reservation is removed, and start B makes the
        // task again and keeps it, lock and all.
        fs::remove_dir(&meta).unwrap();
        let b = pipeline::prepare(&plain, &issue_text(41)).unwrap();
        pausing.resume.wait();

        let message = format!("{:#}", a.join().unwrap().unwrap_err());
        assert!(
            message.contains("task issue-41 is in use by another sbxm command"),
            "{message}"
        );
        assert!(meta.join("task.json").is_file(), "B's record survives");
        assert!(meta.join("repo.git").is_dir(), "B's repo.git survives");
        assert!(meta.join("lock.json").is_file(), "B's lock survives");
        drop(b);
    });
}
