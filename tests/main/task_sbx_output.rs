//! Issue 114: the output lines of `sbx create` and `sbx rm` (the pull and install lines) reach the
//! `run.log` of the task whose sandbox it is, and only that task's, after this invocation's header
//! and the lines printed before. `FakeBackend` prints them (`{name}` is the sandbox's name).

use crate::common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, backend, ctx, fixture, fixture_with, issue_text,
    ok, open_issue, play_reviews, play_tasks, source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::{task_log, task_run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Kind, NewTask, Process, Record};
use sbxm::task::repo::Identity;
use sbxm::task::runlog::{self, RunLog, Tee};

const PULL: [&str; 3] = [
    "{name}: RESOLVE SETUP",
    "{name}: PREPARE IMAGE",
    "{name}: CONFIGURE AGENT",
];

const T0: u64 = 1_790_000_000; // 2026-09-21T14:13:20Z

fn clock() -> Box<dyn Fn() -> u64 + Send> {
    let now = Arc::new(AtomicU64::new(T0));
    Box::new(move || now.fetch_add(1, Ordering::SeqCst))
}

fn log(base: &Path, ids: &[&str], identity: Option<Process>) -> RunLog {
    RunLog::new(
        base,
        "# header".to_owned(),
        ids.iter().map(|s| (*s).to_owned()).collect(),
        identity,
        clock(),
        Box::new(|_| "working".to_owned()),
        Box::new(|_| {}),
    )
}

fn read(base: &Path, id: &str) -> String {
    fs::read_to_string(record::task_dir(base, id).join("run.log")).unwrap_or_default()
}

fn record_of_process(number: u32, process: Process) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        process,
    )
}

/// The lines of `text` without their stamps.
fn unstamped(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| match l.find("] ") {
            Some(at) if !l.starts_with('#') => l[at + 2..].to_owned(),
            _ => l.to_owned(),
        })
        .collect()
}

// --- `RunLog::line_for`: a line for one task only ---

/// `task start` discovers its task once its folder and record exist: an `sbx` line that comes
/// before any other line for it still follows the header and the lines printed before.
#[test]
fn a_task_line_comes_after_the_header_and_the_lines_printed_before() {
    let base = tempfile::tempdir().unwrap();
    let me = Process {
        pid: 7,
        started_at: "2026-09-21T14:00:00Z".into(),
    };
    let mut log = log(base.path(), &[], Some(me.clone()));
    log.line("Starting issue-5 ...");
    record::write(
        &record::task_dir(base.path(), "issue-5"),
        &record_of_process(5, me),
    )
    .unwrap();

    assert!(log.line_for("issue-5", "PREPARE IMAGE"));
    log.line("issue-5: worker started");

    assert_eq!(
        read(base.path(), "issue-5"),
        "# header\n\
         2026-09-21T14:13:20Z [-] Starting issue-5 ...\n\
         2026-09-21T14:13:21Z [working] PREPARE IMAGE\n\
         2026-09-21T14:13:22Z [working] issue-5: worker started\n"
    );
}

/// Boundary case 3: two tasks at once; each log holds only its own `sbx` lines, while a line the
/// command prints still goes to both.
#[test]
fn each_tasks_log_holds_only_its_own_task_lines() {
    let base = tempfile::tempdir().unwrap();
    for id in ["issue-5", "issue-6"] {
        fs::create_dir_all(record::task_dir(base.path(), id)).unwrap();
    }
    let mut log = log(base.path(), &["issue-5", "issue-6"], None);

    assert!(log.line_for("issue-5", "five: PREPARE IMAGE"));
    assert!(log.line_for("issue-6", "six: PREPARE IMAGE"));
    log.line("both");

    assert_eq!(
        unstamped(&read(base.path(), "issue-5")),
        ["# header", "five: PREPARE IMAGE", "both"]
    );
    assert_eq!(
        unstamped(&read(base.path(), "issue-6")),
        ["# header", "six: PREPARE IMAGE", "both"]
    );
}

/// A task the command neither names nor owns is not its to write: the caller logs it itself.
#[test]
fn a_line_for_a_task_the_log_does_not_own_is_refused() {
    let base = tempfile::tempdir().unwrap();
    fs::create_dir_all(record::task_dir(base.path(), "issue-6")).unwrap();
    let mut log = log(base.path(), &["issue-5"], None);

    assert!(!log.line_for("issue-6", "PREPARE IMAGE"));

    assert!(
        !record::task_dir(base.path(), "issue-6")
            .join("run.log")
            .exists()
    );
}

#[test]
fn a_task_line_is_masked_like_any_other() {
    let base = tempfile::tempdir().unwrap();
    fs::create_dir_all(record::task_dir(base.path(), "issue-5")).unwrap();
    let mut log = log(base.path(), &["issue-5"], None);

    log.line_for("issue-5", "token ghp_abcdefghijklmnopqrstuvwx");

    let text = read(base.path(), "issue-5");
    assert!(text.contains("token [redacted]"), "{text}");
    assert!(!text.contains("ghp_"), "{text}");
}

// --- `runlog::task_line`: through the command's log, or appended as is ---

#[test]
fn with_no_routed_log_a_task_line_is_appended_stamped_and_masked() {
    let base = tempfile::tempdir().unwrap();
    let dir = record::task_dir(base.path(), "issue-5");
    fs::create_dir_all(&dir).unwrap();

    runlog::task_line(&dir, "PREPARE IMAGE sk-abcdefghijklmnopqrstuvwx");

    let text = fs::read_to_string(dir.join("run.log")).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.ends_with(" [-] PREPARE IMAGE [redacted]\n"), "{text}");
}

#[test]
fn a_task_line_for_a_folder_that_does_not_exist_writes_nothing() {
    let base = tempfile::tempdir().unwrap();
    let dir = record::task_dir(base.path(), "issue-5");

    runlog::task_line(&dir, "PREPARE IMAGE");

    assert!(!dir.exists());
}

#[test]
fn a_routed_log_takes_its_own_tasks_lines_until_the_route_is_dropped() {
    let base = tempfile::tempdir().unwrap();
    let dir = record::task_dir(base.path(), "issue-5");
    fs::create_dir_all(&dir).unwrap();
    let log = Arc::new(Mutex::new(log(base.path(), &["issue-5"], None)));

    {
        let _route = runlog::route(&log);
        runlog::task_line(&dir, "routed");
    }
    runlog::task_line(&dir, "appended");

    let lines = unstamped(&read(base.path(), "issue-5"));
    assert_eq!(lines, ["# header", "routed", "appended"]);
}

// --- the task commands pass their task's log ---

fn playing(f: &Fixture) -> FakeBackend {
    backend()
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
        .with_create_output(&PULL)
}

/// Boundary cases 1 and 3: `task start --workers 2`; each task's log holds its own sandbox's
/// pull and install lines, in order, and none of the other's.
#[test]
fn two_tasks_started_at_once_each_log_their_own_sandboxes_lines() {
    let f = fixture();
    let backend = playing(&f);
    let github = [1, 2].iter().fold(FakeGitHub::default(), |gh, n| {
        gh.with_issue_text(issue_text(*n))
    });
    let source = source(&f);

    let reports = pipeline::start(&ctx(&f, &source, &backend, &github), &[1, 2]);

    assert!(reports.iter().all(|r| r.result.is_ok()));
    for (own, other) in [(1, 2), (2, 1)] {
        let text = read(&f.env.base_dir(), &format!("issue-{own}"));
        let pulled: Vec<String> = unstamped(&text)
            .into_iter()
            .filter(|l| l.contains("sbxm-task-"))
            .collect();
        let sandbox = format!("sbxm-task-issue-{own}-claude");
        assert_eq!(
            pulled,
            PULL.map(|l| l.replace("{name}", &sandbox)),
            "{text}"
        );
        assert!(!text.contains(&format!("issue-{other}-claude")), "{text}");
    }
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

fn run_options(f: &Fixture) -> task_run::Options {
    task_run::Options {
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
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    }
}

/// `task run --issue 41` with its output teed into `issue-41`'s `run.log` and that log routed, as
/// `main` runs it. Returns the command's result.
fn run_logged(f: &Fixture, backend: &FakeBackend) -> anyhow::Result<()> {
    let log: Arc<Mutex<RunLog>> =
        task_log::open(&f.env.config_dir(), vec!["issue-41".into()], None).unwrap();
    let _route = runlog::route(&log);
    let (mut screen, mut warned) = (Vec::new(), Vec::new());
    let mut out = Tee::new(&mut screen, Arc::clone(&log));
    let mut warn = Tee::new(&mut warned, Arc::clone(&log));
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")]);
    let result = task_run::run(
        &f.env.config_dir(),
        &run_options(f),
        None,
        backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    // As `main` ends a task command: its error goes to the log too.
    if let Err(e) = &result {
        writeln!(warn, "Error: {e:#}").unwrap();
    }
    result
}

fn reviewing(f: &Fixture) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
        .with_create_output(&PULL)
        .with_remove_output(&["{name}: removed"])
}

fn reviewed() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

/// Boundary cases 1 and 2: the worker's and the reviewer's creates and removals are in the log,
/// after the header, in order.
#[test]
fn a_task_runs_creates_and_removals_are_in_its_log_in_order() {
    let f = reviewed();

    run_logged(&f, &reviewing(&f)).unwrap_or_else(|e| panic!("{e:#}"));

    let text = read(&f.env.base_dir(), "issue-41");
    assert!(text.starts_with("# "), "the header first: {text}");
    let sbx: Vec<String> = unstamped(&text)
        .into_iter()
        .filter(|l| l.starts_with("sbxm-task-"))
        .collect();
    let (worker, reviewer) = (
        "sbxm-task-issue-41-claude",
        "sbxm-task-issue-41-review-codex",
    );
    let mut expected: Vec<String> = PULL.map(|l| l.replace("{name}", worker)).to_vec();
    // The worker's sandbox goes once its phase ends (issue 341).
    expected.push(format!("{worker}: removed"));
    expected.extend(PULL.map(|l| l.replace("{name}", reviewer)));
    expected.push(format!("{reviewer}: removed"));
    assert_eq!(sbx, expected, "{text}");
}

/// Boundary case 5: the reviewer's create fails; the lines it printed are in the log, before the
/// error, which is returned as before.
#[test]
fn a_failed_creates_lines_are_in_the_log_before_its_error() {
    let f = reviewed();
    let backend = reviewing(&f)
        .with_failing_create_matching("-codex")
        .with_create_failure_text("sbx create failed: no space left");

    let err = run_logged(&f, &backend).unwrap_err();

    assert!(format!("{err:#}").contains("no space left"), "{err:#}");
    let text = read(&f.env.base_dir(), "issue-41");
    let pulled = text
        .find("sbxm-task-issue-41-review-codex: CONFIGURE AGENT")
        .expect(&text);
    let failed = text.find("no space left").expect(&text);
    assert!(pulled < failed, "{text}");
}
