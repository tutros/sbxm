//! Issue 714 (decision 330(4)): an idle `sbxm task loop --wait` alerts when a retro is due, 10
//! PRs merged since the end of the newest retro record's `Window:` line, the day after that end
//! being the first counted (UTC). The first time it comments once on the open `loop-log` issue
//! and shows a toast; then one toast a day until a newer record lands; its lock carries the line
//! `sbxm status` shows. The retro files and merged PRs are `FakeGitHub`'s, the notifier is
//! `FakeNotifier`.

use crate::common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::MergedPr;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::notify::FakeNotifier;
use sbxm::run::results::parse_timestamp;
use sbxm::task::queue::{LockFile, lock_path};
use sbxm::task::retro;

const REPO: &str = "o/r";
const DAY: u64 = 24 * 60 * 60;
const HOUR: u64 = 60 * 60;
/// The open `loop-log` issue.
const LOG_ISSUE: u32 = 50;
const RETRO: &str = "sdlc/retros/2026-11-16-retro.md";
const END: &str = "2026-11-16";

fn at(stamp: &str) -> u64 {
    parse_timestamp(stamp).unwrap()
}

/// The first second counted: the day after the window's end.
fn after_end() -> u64 {
    at("2026-11-17T00:00:00Z")
}

/// When the loop runs: two days after the window's end, mid-morning.
fn start() -> u64 {
    at("2026-11-18T10:00:00Z")
}

/// No phase is expected: the queue is empty, the loop only waits.
struct NoSteps;

impl Steps for NoSteps {
    fn run(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of {issue}"))
    }

    fn resume(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of {issue}"))
    }

    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }

    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }

    fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

fn fixture() -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    Fixture { env, repo_root }
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: true,
        }
    }
}

fn retro_text(end: &str) -> String {
    format!("# Retro {end}\n\nWindow: 2026-11-15 to {end} (UTC dates, both included).\n")
}

/// `n` PRs merged at `when`, numbered from 600.
fn merged(n: u32, when: u64) -> Vec<MergedPr> {
    (0..n)
        .map(|i| MergedPr {
            number: 600 + i,
            merged_at: when,
        })
        .collect()
}

/// GitHub whose `main` has the retro record [`RETRO`] (window ending [`END`]), with `prs` merged
/// and an open `loop-log` issue.
fn github(prs: Vec<MergedPr>) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .with_open_issues(vec![open_issue(LOG_ISSUE, &["loop-log"], "")])
        .with_repo_files(&[
            ("sdlc/retros/2026-11-14-retro.md", &retro_text("2026-11-14")),
            (RETRO, &retro_text(END)),
            ("sdlc/retros/README.md", "not a retro"),
        ])
        .with_merged_prs(prs)
}

struct Ran {
    result: Result<()>,
    out: String,
    warn: String,
    /// The lock's `retro_due` at each of the loop's sleeps.
    lines: Vec<Option<String>>,
}

/// Runs a `--wait` loop on an empty queue from `now`, stopped by the user after `waits` idle
/// waits (each a `PAUSE_POLL`). `between` runs at each sleep, with the number of sleeps so far.
fn run_loop(
    f: &Fixture,
    gh: &FakeGitHub,
    notifier: &FakeNotifier,
    now: u64,
    waits: usize,
    between: &dyn Fn(usize),
) -> Ran {
    let clock = Cell::new(now);
    let sleeps = Cell::new(0usize);
    let lines = RefCell::new(Vec::new());
    let base = f.base();
    let now = || clock.get();
    let sleep = |d: Duration| {
        lines.borrow_mut().push(retro_line(&base));
        sleeps.set(sleeps.get() + 1);
        clock.set(clock.get() + d.as_secs());
        between(sleeps.get());
    };
    let stop = || sleeps.get() >= waits;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run_notifying(
        &f.env.config_dir(),
        &f.opts(),
        &NoSteps,
        gh,
        &Probe,
        &env,
        notifier,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        lines: lines.into_inner(),
    }
}

fn retro_line(base: &Path) -> Option<String> {
    let text = fs::read_to_string(lock_path(base)).ok()?;
    serde_json::from_str::<LockFile>(&text).ok()?.retro_due
}

fn comments(gh: &FakeGitHub) -> Vec<(u32, String)> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueComment(_, n, body) if body.starts_with("Retro due") => Some((n, body)),
            _ => None,
        })
        .collect()
}

fn keys(notifier: &FakeNotifier) -> Vec<String> {
    notifier.shown().iter().map(|n| n.key.clone()).collect()
}

fn checks(gh: &FakeGitHub) -> usize {
    gh.calls()
        .iter()
        .filter(|c| matches!(c, GhCall::DirFiles(..)))
        .count()
}

/// Waits enough for two checks an hour apart: one at the first wait, one an hour later.
const TWO_CHECKS: usize = (HOUR / task_loop::PAUSE_POLL.as_secs()) as usize + 1;

// --- The threshold ---

#[test]
fn task_loop_retro_due_nine_merged_prs_are_not_due() {
    let f = fixture();
    let gh = github(merged(9, after_end()));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(checks(&gh), 1, "{}", ran.warn);
    assert!(notifier.shown().is_empty(), "{:?}", notifier.shown());
    assert!(comments(&gh).is_empty());
    assert_eq!(ran.lines, [None]);
    assert!(!ran.warn.contains("retro"), "{}", ran.warn);
}

#[test]
fn task_loop_retro_due_ten_merged_prs_are_due_with_a_toast_a_comment_and_a_status_line() {
    let f = fixture();
    let gh = github(merged(10, after_end()));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(
        comments(&gh),
        [(
            LOG_ISSUE,
            "Retro due: 10 PRs merged since 2026-11-16.".to_owned()
        )]
    );
    let shown = notifier.shown();
    assert_eq!(keys(&notifier), ["retro-due-2026-11-16-2026-11-18"]);
    assert_eq!(shown[0].title, "Retro due");
    assert!(shown[0].body.contains("10 PRs"), "{}", shown[0].body);
    assert_eq!(
        ran.lines,
        [Some("retro due: 10 PRs since 2026-11-16".to_owned())]
    );
    // Read from the newest record on the default branch.
    assert!(
        gh.calls()
            .contains(&GhCall::FileText(REPO.into(), "main".into(), RETRO.into()))
    );
}

#[test]
fn task_loop_retro_due_eleven_merged_prs_comment_once_over_two_checks() {
    let f = fixture();
    let gh = github(merged(11, after_end()));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), TWO_CHECKS, &|_| {});

    ran.result.unwrap();
    assert_eq!(checks(&gh), 2, "checked again an hour later");
    assert_eq!(comments(&gh).len(), 1, "{:?}", comments(&gh));
    assert_eq!(keys(&notifier), ["retro-due-2026-11-16-2026-11-18"]);
}

#[test]
fn task_loop_retro_due_checks_at_most_once_an_hour() {
    let f = fixture();
    let gh = github(merged(10, after_end()));
    let notifier = FakeNotifier::default();

    run_loop(&f, &gh, &notifier, start(), TWO_CHECKS - 1, &|_| {})
        .result
        .unwrap();

    assert_eq!(checks(&gh), 1);
}

// --- The window's end day ---

#[test]
fn task_loop_retro_due_a_pr_merged_on_the_windows_end_day_doesnt_count() {
    let f = fixture();
    let mut prs = merged(9, after_end());
    prs.extend(merged(1, after_end() - 1));
    let gh = github(prs);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(checks(&gh), 1, "{}", ran.warn);
    assert!(
        gh.calls()
            .contains(&GhCall::PrsMergedSince(REPO.into(), END.into())),
        "the PRs were counted"
    );
    assert!(notifier.shown().is_empty());
    assert!(comments(&gh).is_empty());
}

#[test]
fn task_loop_retro_due_a_pr_merged_the_day_after_counts() {
    let f = fixture();
    let mut prs = merged(9, after_end() + 5 * HOUR);
    prs.extend(merged(1, after_end()));
    let gh = github(prs);
    let notifier = FakeNotifier::default();

    run_loop(&f, &gh, &notifier, start(), 1, &|_| {})
        .result
        .unwrap();

    assert_eq!(keys(&notifier), ["retro-due-2026-11-16-2026-11-18"]);
    assert_eq!(comments(&gh).len(), 1);
}

// --- No record ---

#[test]
fn task_loop_retro_due_no_retro_file_is_due() {
    let f = fixture();
    let gh = github(Vec::new()).with_repo_files(&[("sdlc/retros/README.md", "x")]);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(
        comments(&gh),
        [(
            LOG_ISSUE,
            "Retro due: no retro record (`sdlc/retros/*-retro.md`) on `main`.".to_owned()
        )]
    );
    assert_eq!(keys(&notifier), ["retro-due-none-2026-11-18"]);
    assert_eq!(
        ran.lines,
        [Some("retro due: no retro record on main".to_owned())]
    );
}

#[test]
fn task_loop_retro_due_a_retro_file_with_no_window_line_is_due_and_the_comment_names_it() {
    let f = fixture();
    let gh = github(merged(20, after_end())).with_repo_files(&[
        ("sdlc/retros/2026-11-14-retro.md", &retro_text("2026-11-14")),
        (RETRO, "# Retro 2026-11-16\n\nNo window here.\n"),
    ]);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    let posted = comments(&gh);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].1.contains(RETRO), "{}", posted[0].1);
    assert_eq!(notifier.shown().len(), 1);
    assert_eq!(
        ran.lines,
        [Some(format!(
            "retro due: {RETRO} has no readable Window: line"
        ))]
    );
}

#[test]
fn task_loop_retro_due_a_window_line_whose_end_isnt_a_date_is_due() {
    let f = fixture();
    let gh = github(Vec::new()).with_repo_files(&[(RETRO, "Window: 2026-11-15 to yesterday\n")]);
    let notifier = FakeNotifier::default();

    run_loop(&f, &gh, &notifier, start(), 1, &|_| {})
        .result
        .unwrap();

    let posted = comments(&gh);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].1.contains(RETRO), "{}", posted[0].1);
}

// --- Calendar dates in the window's end ---

/// What `retro::check` says of a newest record whose window ends on `end`, with 12 PRs merged
/// long after any of the ends tried.
fn due_for_end(end: &str) -> Option<retro::Due> {
    let gh = FakeGitHub::default()
        .with_default_branch("main")
        .with_repo_files(&[(RETRO, &format!("Window: 2024-01-01 to {end}\n"))])
        .with_merged_prs(merged(12, at("2027-06-01T00:00:00Z")));
    retro::check(&gh, REPO).unwrap()
}

#[test]
fn task_loop_retro_due_a_window_end_that_isnt_a_calendar_day_counts_as_no_window() {
    for end in [
        "2026-02-30",
        "2026-02-29",
        "2026-04-31",
        "2026-13-01",
        "2026-11-00",
    ] {
        assert_eq!(
            due_for_end(end),
            Some(retro::Due::NoWindow {
                file: RETRO.to_owned()
            }),
            "{end}"
        );
    }
}

#[test]
fn task_loop_retro_due_month_ends_and_a_leap_day_are_window_ends() {
    for end in [
        "2026-01-31",
        "2026-02-28",
        "2024-02-29",
        "2026-04-30",
        "2026-12-31",
    ] {
        assert_eq!(
            due_for_end(end),
            Some(retro::Due::Prs {
                prs: 12,
                end: end.to_owned()
            }),
            "{end}"
        );
    }
}

// --- Restarts and days ---

#[test]
fn task_loop_retro_due_a_restart_the_same_day_shows_no_second_toast_or_comment() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    let first = FakeNotifier::default();
    run_loop(&f, &gh, &first, start(), 1, &|_| {})
        .result
        .unwrap();
    assert_eq!(first.shown().len(), 1);

    let again = FakeNotifier::default();
    run_loop(&f, &gh, &again, start() + 2 * HOUR, 1, &|_| {})
        .result
        .unwrap();

    assert!(again.shown().is_empty(), "{:?}", again.shown());
    assert_eq!(comments(&gh).len(), 1, "{:?}", comments(&gh));
}

#[test]
fn task_loop_retro_due_the_next_day_shows_one_toast_and_no_comment() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    run_loop(&f, &gh, &FakeNotifier::default(), start(), 1, &|_| {})
        .result
        .unwrap();

    let next_day = FakeNotifier::default();
    run_loop(&f, &gh, &next_day, start() + DAY, TWO_CHECKS, &|_| {})
        .result
        .unwrap();

    assert_eq!(keys(&next_day), ["retro-due-2026-11-16-2026-11-19"]);
    assert_eq!(comments(&gh).len(), 1, "{:?}", comments(&gh));
}

#[test]
fn task_loop_retro_due_a_running_loop_shows_one_toast_on_the_next_day() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    let notifier = FakeNotifier::default();
    // Starts at 23:30: the check an hour later is on the next day.
    let late = at("2026-11-18T23:30:00Z");

    run_loop(&f, &gh, &notifier, late, TWO_CHECKS, &|_| {})
        .result
        .unwrap();

    assert_eq!(
        keys(&notifier),
        [
            "retro-due-2026-11-16-2026-11-18",
            "retro-due-2026-11-16-2026-11-19"
        ]
    );
    assert_eq!(comments(&gh).len(), 1);
}

// --- A new record ---

#[test]
fn task_loop_retro_due_a_new_record_stops_the_toasts_and_clears_the_status_line() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    let notifier = FakeNotifier::default();
    let newer = retro_text("2026-11-18");
    // The new record lands while the loop waits, before its next check.
    let land = |sleeps: usize| {
        if sleeps == 1 {
            gh.set_repo_files(&[
                (RETRO, &retro_text(END)),
                ("sdlc/retros/2026-11-18-retro.md", &newer),
            ]);
        }
    };

    let ran = run_loop(&f, &gh, &notifier, start(), TWO_CHECKS + 1, &land);

    ran.result.unwrap();
    assert_eq!(checks(&gh), 2);
    assert_eq!(notifier.shown().len(), 1, "only the first check's toast");
    assert_eq!(
        ran.lines.first(),
        Some(&Some("retro due: 12 PRs since 2026-11-16".to_owned()))
    );
    assert_eq!(ran.lines.last(), Some(&None), "{:?}", ran.lines);

    let next_day = FakeNotifier::default();
    run_loop(&f, &gh, &next_day, start() + DAY, 1, &|_| {})
        .result
        .unwrap();
    assert!(next_day.shown().is_empty(), "{:?}", next_day.shown());
}

// --- A failing read ---

#[test]
fn task_loop_retro_due_a_failing_read_logs_one_line_and_the_loop_goes_on() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    gh.set_retro_reads_failing(Some("HTTP 502: Bad Gateway"));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), TWO_CHECKS - 1, &|_| {});

    ran.result.unwrap();
    let lines: Vec<&str> = ran.warn.lines().filter(|l| l.contains("retro")).collect();
    assert_eq!(lines.len(), 1, "{}", ran.warn);
    assert!(lines[0].contains("HTTP 502"), "{}", lines[0]);
    assert!(ran.out.contains("stopped on request"), "{}", ran.out);
    assert!(notifier.shown().is_empty());
    assert!(comments(&gh).is_empty());
}

#[test]
fn task_loop_retro_due_a_failed_read_is_tried_again_an_hour_later() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    gh.set_retro_reads_failing(Some("HTTP 502: Bad Gateway"));
    let notifier = FakeNotifier::default();
    let recover = |sleeps: usize| {
        if sleeps == 1 {
            gh.set_retro_reads_failing(None);
        }
    };

    run_loop(&f, &gh, &notifier, start(), TWO_CHECKS, &recover)
        .result
        .unwrap();

    assert_eq!(checks(&gh), 2);
    assert_eq!(notifier.shown().len(), 1);
    assert_eq!(comments(&gh).len(), 1);
}

#[test]
fn task_loop_retro_due_a_failed_comment_is_one_line_and_the_toast_still_shows() {
    let f = fixture();
    let gh = github(merged(12, after_end()));
    gh.set_issue_comments_failing(LOG_ISSUE, true);
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, start(), 1, &|_| {});

    ran.result.unwrap();
    let lines: Vec<&str> = ran.warn.lines().filter(|l| l.contains("retro")).collect();
    assert_eq!(lines.len(), 1, "{}", ran.warn);
    assert_eq!(notifier.shown().len(), 1);
}
