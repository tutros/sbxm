//! Issue 538: `run.log` shows each phase as it starts (`worker started`, `gates started`, `review
//! round <n> started`, `fix round <n> started`), written when printed, and each review round's
//! line names its `review-<n>.md`. The log is opened as `task loop`'s `TaskSteps` opens it:
//! `task_log::open` naming the task, with the command's writers teed into it.

mod common;

use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Barrier, Mutex};

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, fixture_with, is_headless,
    issue_text, ok, open_issue, play, play_reviews, play_tasks, source,
};
use sbxm::backend::{ExecSpec, FakeBackend};
use sbxm::commands::{task_fix, task_log, task_review, task_run};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record;
use sbxm::task::repo::Identity;
use sbxm::task::review::pr_comment;
use sbxm::task::runlog::{RunLog, Tee};

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

/// A value that looks like an API key, to show it never reaches `run.log`.
const SECRET_MODEL: &str = "sk-abcdefghijklmnopqrstuvwx";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

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

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
}

fn options(f: &Fixture) -> task_run::Options {
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

/// `task run --issue 41` with its output teed into `issue-41`'s `run.log`, as the loop runs it.
/// Returns the screen output.
fn run_logged(f: &Fixture, opts: &task_run::Options, backend: &FakeBackend) -> String {
    let log: Arc<Mutex<RunLog>> =
        task_log::open(&f.env.config_dir(), vec!["issue-41".into()], None).unwrap();
    let (mut screen, mut warned) = (Vec::new(), Vec::new());
    {
        let mut out = Tee::new(&mut screen, Arc::clone(&log));
        let mut warn = Tee::new(&mut warned, log);
        task_run::run(
            &f.env.config_dir(),
            opts,
            None,
            backend,
            &github(),
            &Probe,
            &FakeHostRunner::default(),
            &mut out,
            &mut warn,
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
    }
    String::from_utf8(screen).unwrap()
}

fn run_log(f: &Fixture, id: &str) -> String {
    fs::read_to_string(record::task_dir(&f.env.base_dir(), id).join("run.log")).unwrap_or_default()
}

/// The phase start lines of `text`, in order, without their stamps.
fn starts(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.ends_with(" started") || l.contains(" started ("))
        .map(|l| l[l.find("issue-41:").expect(l)..].to_owned())
        .collect()
}

fn review_file(f: &Fixture, id: &str, round: u32) -> String {
    record::task_dir(&f.env.base_dir(), id)
        .join(format!("review-{round}.md"))
        .display()
        .to_string()
}

#[test]
fn the_header_and_worker_started_are_in_run_log_while_the_worker_runs() {
    let f = config();
    // The worker's headless run waits at the barrier twice: once to say it is running, once
    // for the test to have read the log.
    let barrier = Arc::new(Barrier::new(2));
    let held = Arc::new(AtomicBool::new(false));
    let blocking = {
        let (barrier, held) = (Arc::clone(&barrier), Arc::clone(&held));
        backend(&f, &[CLEAN]).with_exec_responder(move |sandbox, spec| {
            if sandbox.ends_with("-claude") && is_headless(spec) && !held.swap(true, SeqCst) {
                barrier.wait();
                barrier.wait();
            }
            None
        })
    };

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| run_logged(&f, &options(&f), &blocking));

        barrier.wait();
        let log = run_log(&f, "issue-41");
        barrier.wait();
        handle.join().unwrap();

        assert!(log.starts_with("# "), "the header first: {log}");
        assert!(log.contains("Starting issue-41 ..."), "{log}");
        assert!(
            log.contains("issue-41: worker started (claude, default model)"),
            "{log}"
        );
        assert!(
            !log.contains("worker claude (default model) completed"),
            "{log}"
        );
    });
}

#[test]
fn each_phase_start_shows_once_in_order_with_two_review_rounds_and_a_fix_round() {
    // One fix round in the budget: the second review's finding stops the task there.
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    );

    let screen = run_logged(&f, &options(&f), &backend(&f, &[ONE, ONE]));

    let expected = [
        "issue-41: worker started (claude, default model)",
        "issue-41: gates started",
        "issue-41: review round 1 started (codex, default model)",
        "issue-41: fix round 1 started",
        "issue-41: gates started",
        "issue-41: review round 2 started (codex, default model)",
    ];
    assert_eq!(starts(&screen), expected, "{screen}");
    assert_eq!(starts(&run_log(&f, "issue-41")), expected);
}

#[test]
fn each_review_round_line_names_its_file_a_clean_one_too() {
    let f = config();

    let screen = run_logged(&f, &options(&f), &backend(&f, &[ONE, CLEAN]));

    // A narrow clean round 2 after the fix, then the confirmatory full round 3.
    for (round, said) in [
        (1, "1 must-fix finding(s)"),
        (2, "0 must-fix finding(s) (narrow)"),
        (3, "0 must-fix finding(s)"),
    ] {
        let line = format!(
            "issue-41: review round {round} by codex (default model): {said} — {}",
            review_file(&f, "issue-41", round)
        );
        assert!(screen.contains(&line), "{line}\n{screen}");
        assert!(run_log(&f, "issue-41").contains(&line), "{line}");
    }
}

#[test]
fn secret_masking_applies_to_the_phase_start_lines() {
    let f = config();
    let opts = task_run::Options {
        worker_model: Some(SECRET_MODEL.into()),
        ..options(&f)
    };

    let screen = run_logged(&f, &opts, &backend(&f, &[CLEAN]));

    assert!(
        screen.contains(&format!(
            "issue-41: worker started (claude, {SECRET_MODEL})"
        )),
        "the screen shows the line as printed: {screen}"
    );
    let log = run_log(&f, "issue-41");
    assert!(
        log.contains("issue-41: worker started (claude, [redacted])"),
        "{log}"
    );
    assert!(!log.contains(SECRET_MODEL), "{log}");
}

// ---- `task fix --pr N`: its narrow review's line names its file ----

fn fix_config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn pr7(head: &str) -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
        base_moved: false,
    }
}

fn posted_review(head: &str) -> PrComment {
    let text = format!(
        "Must-fix findings: 1\nRisk: medium\n\nScope: `0000000..{head}`\nIssues: pending\n\n\
         ## Must fix\n\n### M-1 - The lock is never released\n\n**Where:** `src/lock.rs:10`\n\
         **What happens:** it stays held\n**Why it matters:** decision 1\n**Fix:** release it\n"
    );
    PrComment {
        author: "tutros".into(),
        body: pr_comment(7, "", &text),
    }
}

const FIX_CLEAN: &str =
    "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing.\n";

#[test]
fn a_fix_tasks_review_line_names_its_file() {
    let f = fix_config();
    let head = add_pr_head(&f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    let id = record::fix_id(7, &head).unwrap();
    let github = FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(pr7(&head))
        .with_pr_comments(7, vec![posted_review(&head)]);
    let worker = play(
        &f.env.base_dir().join("tasks").join(&id),
        "main",
        "feature-x",
        Play {
            commits: vec!["src/lock.rs".into()],
            result_md: Some(b"M-1 fixed\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![FIX_CLEAN.to_owned()]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        });
    let log: Arc<Mutex<RunLog>> =
        task_log::open(&f.env.config_dir(), vec![id.clone()], None).unwrap();
    let mut screen = Vec::new();
    {
        let mut out = Tee::new(&mut screen, Arc::clone(&log));
        let mut warn = Tee::new(Vec::new(), log);
        task_fix::run(
            &f.env.config_dir(),
            &task_fix::Options {
                repo_root: f.env.tmp.path().join("target-repo"),
                pr: 7,
                repo: Some("o/r".into()),
                base: None,
                clone_source: Some(f.origin.to_str().unwrap().to_owned()),
                identity: Some(Identity {
                    name: "Dev".into(),
                    email: "dev@example.com".into(),
                }),
                disk: Default::default(),
                check_run: None,
                location: None,
            },
            &backend,
            &github,
            &Probe,
            &FakeHostRunner::default(),
            &mut out,
            &mut warn,
        )
        .unwrap_or_else(|e| panic!("{e:#}"));
    }
    let screen = String::from_utf8(screen).unwrap();

    let file = review_file(&f, &id, 1);
    let line = screen
        .lines()
        .find(|l| l.contains(": review round 1 by codex"))
        .unwrap_or_else(|| panic!("{screen}"));
    assert!(line.ends_with(&format!("— {file}")), "{line}");
    assert!(run_log(&f, &id).contains(line), "{line}");
    assert!(
        screen.contains(&format!("{id}: worker started (claude, default model)")),
        "{screen}"
    );
}

// ---- `task review --pr N`: its gates and its reviewer say when they start (review M-1) ----

fn pr_review_github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(PrInfo {
            number: 7,
            head_ref: "feature-x".into(),
            head_oid: "0000000000000000000000000000000000000000".into(),
            is_cross_repository: false,
            head_repo: String::new(),
            state: PrState::Open,
            title: "Add the x feature".into(),
            body: "Adds x.\n\nFixes #4".into(),
            closing_issues: vec![4],
            base_moved: false,
        })
        .with_issue_text(issue_text(4))
}

fn pr_review_backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(play_reviews(
            &f.env.base_dir(),
            "pr-7",
            reviews.iter().map(|s| (*s).to_owned()).collect(),
        ))
}

/// `task review --pr 7` teed into `pr-7`'s `run.log`, as the loop runs it; the result and the
/// screen output.
fn review_pr_logged(f: &Fixture, backend: &FakeBackend) -> (anyhow::Result<()>, String) {
    let log: Arc<Mutex<RunLog>> =
        task_log::open(&f.env.config_dir(), vec!["pr-7".into()], None).unwrap();
    let mut screen = Vec::new();
    let result = {
        let mut out = Tee::new(&mut screen, Arc::clone(&log));
        let mut warn = Tee::new(Vec::new(), log);
        task_review::run(
            &f.env.config_dir(),
            &task_review::Options {
                repo_root: f.env.tmp.path().join("target-repo"),
                target: task_review::Target::Pr(7),
                repo: Some("o/r".into()),
                base: None,
                clone_source: Some(source(f)),
                reviewer_harness: None,
                reviewer_model: None,
                reviewer_time_limit: None,
                time_limit: None,
                profile: None,
                resolved_spec_id: None,
                force: false,
                disk: Default::default(),
                location: None,
                identity: None,
            },
            backend,
            &pr_review_github(),
            &Probe,
            &FakeHostRunner::default(),
            &mut out,
            &mut warn,
        )
    };
    (result, String::from_utf8(screen).unwrap())
}

/// `run.log` of `pr-7` read while the first exec `held` picks out waits at a barrier.
fn pr_log_while(held: impl Fn(&ExecSpec) -> bool + Send + Sync + 'static) -> String {
    let f = config();
    add_pr_head(&f, 7);
    let barrier = Arc::new(Barrier::new(2));
    let once = Arc::new(AtomicBool::new(false));
    let blocking = {
        let (barrier, once) = (Arc::clone(&barrier), Arc::clone(&once));
        pr_review_backend(&f, &[CLEAN]).with_exec_responder(move |_, spec| {
            if held(spec) && !once.swap(true, SeqCst) {
                barrier.wait();
                barrier.wait();
            }
            None
        })
    };
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| review_pr_logged(&f, &blocking));
        barrier.wait();
        let log = run_log(&f, "pr-7");
        barrier.wait();
        let (result, screen) = handle.join().unwrap();
        result.unwrap_or_else(|e| panic!("{e:#}: {screen}"));
        log
    })
}

#[test]
fn a_pr_reviews_gates_start_line_is_in_run_log_while_its_gate_runs() {
    let log = pr_log_while(|spec| spec.argv.iter().any(|a| a.contains("cargo test")));

    assert!(log.starts_with("# "), "the header first: {log}");
    assert!(log.contains("pr-7: gates started"), "{log}");
    assert!(!log.contains("review round 1 started"), "{log}");
}

#[test]
fn a_pr_reviews_review_start_line_is_in_run_log_once_while_its_reviewer_runs() {
    let log = pr_log_while(is_headless);

    let line = "pr-7: review round 1 started (codex, default model)";
    assert_eq!(log.matches(line).count(), 1, "{log}");
    let gates = log.find("pr-7: gates started").expect(&log);
    assert!(gates < log.find(line).unwrap(), "{log}");
    assert!(!log.contains("must-fix finding(s)"), "{log}");
}

#[test]
fn a_retried_pr_review_says_its_review_starts_but_no_gates() {
    let f = config();
    add_pr_head(&f, 7);
    let (first, _) = review_pr_logged(&f, &pr_review_backend(&f, &["Looks fine.\n"]));
    assert!(first.is_err(), "the first reviewer wrote no usable review");

    let (result, screen) = review_pr_logged(&f, &pr_review_backend(&f, &[CLEAN]));

    result.unwrap_or_else(|e| panic!("{e:#}: {screen}"));
    assert!(
        screen.contains("pr-7: review round 2 started (codex, default model)"),
        "{screen}"
    );
    assert!(!screen.contains("gates started"), "{screen}");
}
