//! Issue 463 (decision 305): `task loop` writes its log under the base dir, one file per run
//! (`<base>/.sbxm/loop-logs/<date>-<HHMMSS>-loop.md`), never in the checkout, and posts it at
//! the stop as a comment on the open `loop-log` issue, split between rows past 65,000
//! characters.

mod common;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, IssueRequest};
use sbxm::task::loop_log;

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// 2026-11-08T10:00:00Z.
const START: u64 = 1_794_132_000;

// --- The file's name ---

#[test]
fn the_log_is_named_by_the_loop_start_in_utc_with_seconds_under_the_base_dir() {
    let path = loop_log::path(Path::new("base"), START + 3_723);

    assert_eq!(
        path,
        Path::new("base")
            .join(".sbxm")
            .join("loop-logs")
            .join("2026-11-08-110203-loop.md")
    );
}

// --- Splitting a long log into comments ---

const TABLE: &str = "# Loop log\n\n| # | Issue |\n|---|---|\n";

/// A row of exactly `len` characters, its newline included.
fn row(number: usize, len: usize) -> String {
    let start = format!("| {number} | #");
    let pad = len - start.chars().count() - " |\n".len();
    format!("{start}{} |\n", "a".repeat(pad))
}

/// A log of `TABLE` and two rows whose single comment under `heading` is `total` characters.
fn log_of(heading: &str, total: usize) -> String {
    let fixed = heading.chars().count() + 2 + TABLE.chars().count();
    let first = row(1, 100);
    format!("{TABLE}{first}{}", row(2, total - fixed - 100))
}

#[test]
fn a_comment_of_65000_characters_is_posted_whole() {
    let heading = "Loop started 2026-11-08T10:00:00Z";
    let log = log_of(heading, 65_000);

    let comments = loop_log::comments(heading, &log);

    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0], format!("{heading}\n\n{log}"));
    assert_eq!(comments[0].chars().count(), 65_000);
}

#[test]
fn a_comment_of_65001_characters_is_split_between_rows_each_part_with_the_header() {
    let heading = "Loop started 2026-11-08T10:00:00Z";
    let log = log_of(heading, 65_001);
    let first = row(1, 100);

    let comments = loop_log::comments(heading, &log);

    assert_eq!(comments.len(), 2);
    assert_eq!(
        comments[0],
        format!("{heading} (part 1 of 2)\n\n{TABLE}{first}")
    );
    assert!(
        comments[1].starts_with(&format!("{heading} (part 2 of 2)\n\n{TABLE}| 2 | #")),
        "{}",
        &comments[1][..200]
    );
    assert!(comments[1].ends_with(" |\n"));
    assert!(comments.iter().all(|c| c.chars().count() <= 65_000));
}

#[test]
fn a_row_longer_than_a_comment_is_posted_alone_and_whole() {
    let heading = "Loop started";
    let (small, huge) = (row(1, 50), row(2, 70_000));
    let log = format!("{TABLE}{small}{huge}{}", row(3, 50));

    let comments = loop_log::comments(heading, &log);

    assert_eq!(comments.len(), 3);
    assert_eq!(
        comments[1],
        format!("{heading} (part 2 of 3)\n\n{TABLE}{huge}")
    );
    assert!(comments[2].starts_with(&format!("{heading} (part 3 of 3)\n\n{TABLE}| 3 |")));
}

// --- The loop ---

/// Every task's `run` and `resume` fails for `sbx` not running, so each task is parked after its
/// second failure with one log row; a task in `refused` is refused before any record.
struct Script {
    refused: Vec<u32>,
    clock: Cell<u64>,
}

impl Script {
    fn new(start: u64) -> Self {
        Self {
            refused: Vec::new(),
            clock: Cell::new(start),
        }
    }

    fn act(&self, issue: u32) -> Result<()> {
        self.clock.set(self.clock.get() + 300);
        if self.refused.contains(&issue) {
            return Err(anyhow!("the issue is not ready"));
        }
        Err(anyhow::Error::new(sbxm::headless::ExecUnavailable(
            "sbx exec failed".into(),
        )))
    }
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.act(issue)
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.act(issue)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of #{issue}"))
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
    fn logs(&self) -> PathBuf {
        self.env.base_dir().join(".sbxm").join("loop-logs")
    }

    fn log_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(self.logs())
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        // The logs only, not the markers of the posted ones (issue 594).
        files.retain(|p| p.extension().is_some_and(|e| e == "md"));
        files.sort();
        files
    }
}

/// Every file under `dir`, relative, with its contents.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut all = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.extend(tree(&path));
        } else {
            all.push((path.clone(), fs::read(&path).unwrap()));
        }
    }
    all.sort();
    all
}

fn loop_log_issue(number: u32) -> Issue {
    open_issue(number, &["loop-log"], "")
}

fn github(mut issues: Vec<Issue>) -> FakeGitHub {
    issues.insert(0, open_issue(3, &[READY, "size:S"], ""));
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", &"a".repeat(40))
        .with_issue_text(issue_text(3))
        .with_next_issue_number(900)
        .with_open_issues(issues)
}

struct Run {
    result: Result<()>,
    out: String,
    warn: String,
}

fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub) -> Run {
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &|| false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: Some(1),
            repo: Some(REPO.into()),
            wait: false,
        },
        script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn creates(gh: &FakeGitHub) -> Vec<IssueRequest> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueCreate(_, request) => Some(request),
            _ => None,
        })
        .collect()
}

/// The issue and body of every `issue_comment`.
fn comments(gh: &FakeGitHub) -> Vec<(u32, String)> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::IssueComment(_, number, body) => Some((number, body)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_log_goes_to_one_file_under_the_base_dir_and_the_checkout_is_untouched() {
    let f = fixture();
    let before = tree(&f.repo_root);
    let gh = github(vec![loop_log_issue(40)]);

    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    let files = f.log_files();
    assert_eq!(
        files,
        [f.logs().join("2026-11-08-100000-loop.md")],
        "{}",
        run.out
    );
    let text = fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("| 1 | #3 "), "{text}");
    assert_eq!(tree(&f.repo_root), before);
}

#[test]
fn with_no_open_loop_log_issue_one_is_created_and_the_log_posted_to_it() {
    let f = fixture();
    let gh = github(Vec::new());

    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    assert_eq!(
        creates(&gh),
        [IssueRequest {
            title: "Loop logs since 2026-11-08".into(),
            body: "Logs of `sbxm task loop` runs, one comment per loop, until the next retro \
                   reads and closes this issue (issue 463)."
                .into(),
            labels: vec!["loop-log".into()],
        }]
    );
    let posted = comments(&gh);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, 900);
    let file = &f.log_files()[0];
    let body = &posted[0].1;
    let heading = body.lines().next().unwrap();
    assert!(heading.contains("2026-11-08T10:00:00Z"), "{heading}");
    assert!(heading.contains("--cap 1 reached"), "{heading}");
    assert!(heading.contains(&file.display().to_string()), "{heading}");
    assert!(body.ends_with(&fs::read_to_string(file).unwrap()), "{body}");
    assert_eq!(run.warn, "");
}

#[test]
fn with_one_open_loop_log_issue_the_log_is_posted_to_it_and_nothing_is_created() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);

    run_loop(&f, &Script::new(START), &gh).result.unwrap();

    assert_eq!(creates(&gh), []);
    let posted: Vec<u32> = comments(&gh).into_iter().map(|(n, _)| n).collect();
    assert_eq!(posted, [40]);
}

#[test]
fn with_two_open_loop_log_issues_the_log_goes_to_the_lower_number() {
    let f = fixture();
    let gh = github(vec![
        loop_log_issue(52),
        open_issue(7, &["bug"], ""),
        loop_log_issue(41),
    ]);

    run_loop(&f, &Script::new(START), &gh).result.unwrap();

    assert_eq!(creates(&gh), []);
    let posted: Vec<u32> = comments(&gh).into_iter().map(|(n, _)| n).collect();
    assert_eq!(posted, [41]);
}

#[test]
fn a_failed_create_is_one_warning_naming_the_file_and_the_loop_ends_as_before() {
    let f = fixture();
    let gh = github(Vec::new()).failing_issue_create_at(1);

    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    assert!(run.out.contains("--cap 1 reached"), "{}", run.out);
    assert_eq!(comments(&gh), []);
    let file = f.log_files()[0].display().to_string();
    assert_eq!(run.warn.lines().count(), 1, "{}", run.warn);
    assert!(run.warn.contains(&file), "{}", run.warn);
    assert!(run.warn.contains("issue create 1 fails"), "{}", run.warn);
}

#[test]
fn a_failed_comment_is_one_warning_naming_the_file_and_the_loop_ends_as_before() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]).failing_issue_comments_on(40);

    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    assert_eq!(comments(&gh).len(), 1, "not retried");
    let file = f.log_files()[0].display().to_string();
    assert_eq!(run.warn.lines().count(), 1, "{}", run.warn);
    assert!(run.warn.contains(&file), "{}", run.warn);
    assert!(
        run.warn.contains("comment on issue 40 fails"),
        "{}",
        run.warn
    );
}

#[test]
fn a_loop_that_stops_on_an_error_still_posts_and_keeps_its_error() {
    let f = fixture();
    // Two tasks parked in a row stop the loop with an error; no cap here.
    let mut gh = github(vec![loop_log_issue(40)]);
    gh = gh.with_issue_text(issue_text(4));
    gh.set_open_issues(vec![
        open_issue(3, &[READY, "size:S"], ""),
        open_issue(4, &[READY, "size:S"], ""),
        loop_log_issue(40),
    ]);
    let script = Script::new(START);
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        &script,
        &gh,
        &Probe,
        &Env {
            now: &now,
            sleep: &sleep,
            stop_requested: &|| false,
        },
        &mut out,
        &mut warn,
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("two tasks parked in a row"), "{err}");
    let posted = comments(&gh);
    assert_eq!(posted.len(), 1);
    let heading = posted[0].1.lines().next().unwrap().to_owned();
    assert!(heading.contains("two tasks parked in a row"), "{heading}");
    assert!(posted[0].1.contains("| 2 | #4 "), "{}", posted[0].1);
}

#[test]
fn two_loops_in_a_row_write_two_files_each_numbered_from_1() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);

    run_loop(&f, &Script::new(START), &gh).result.unwrap();
    // The first task's record stays; the second loop takes another issue.
    gh.set_open_issues(vec![
        open_issue(5, &[READY, "size:S"], ""),
        loop_log_issue(40),
    ]);
    let gh = gh.with_issue_text(issue_text(5));
    run_loop(&f, &Script::new(START + 3_600), &gh)
        .result
        .unwrap();

    let files = f.log_files();
    assert_eq!(
        files,
        [
            f.logs().join("2026-11-08-100000-loop.md"),
            f.logs().join("2026-11-08-110000-loop.md"),
        ]
    );
    let second = fs::read_to_string(&files[1]).unwrap();
    assert!(second.contains("| 1 | #5 "), "{second}");
    assert!(!second.contains("#3 "), "{second}");
    assert_eq!(comments(&gh).len(), 2);
}

#[test]
fn a_loop_that_runs_no_task_posts_nothing() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);
    let mut script = Script::new(START);
    script.refused.push(3);

    let run = run_loop(&f, &script, &gh);

    run.result.unwrap();
    assert!(run.out.contains("the queue is empty"), "{}", run.out);
    assert_eq!(comments(&gh), []);
    assert_eq!(creates(&gh), []);
    assert_eq!(f.log_files(), Vec::<PathBuf>::new());
}

// --- Issue 594: earlier logs a loop never posted are posted late at the next start ---

const LATE: &str = "Posted late by the loop started 2026-11-08T10:00:00Z: the loop that wrote it \
                    ended without its stop (Ctrl-C, a crash or a restart).";

/// An earlier loop's log, started `ago` seconds before `START`, with `rows` rows.
fn plant_earlier(f: &Fixture, ago: u64, rows: usize) -> PathBuf {
    fs::create_dir_all(f.logs()).unwrap();
    let path = loop_log::path(&f.env.base_dir(), START - ago);
    let rows: String = (1..=rows).map(|n| row(n, 40)).collect();
    fs::write(&path, format!("{TABLE}{rows}")).unwrap();
    path
}

/// A loop that runs no task of its own, so it posts nothing at its stop.
fn idle_script() -> Script {
    let mut script = Script::new(START);
    script.refused.push(3);
    script
}

#[test]
fn with_no_earlier_log_nothing_is_posted_late() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);

    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    let posted = comments(&gh);
    assert_eq!(posted.len(), 1, "only its own at the stop");
    assert!(!posted[0].1.contains("Posted late"), "{}", posted[0].1);
    assert_eq!(run.warn, "");
}

#[test]
fn an_unposted_earlier_log_with_rows_is_posted_once_with_the_late_marker() {
    let f = fixture();
    let earlier = plant_earlier(&f, 86_400, 2);
    let gh = github(vec![loop_log_issue(40)]);

    let run = run_loop(&f, &idle_script(), &gh);

    run.result.unwrap();
    let posted = comments(&gh);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, 40);
    let body = &posted[0].1;
    assert!(body.starts_with(&format!("{LATE}\n")), "{body}");
    assert!(body.contains("2026-11-07T10:00:00Z"), "{body}");
    assert!(body.contains(&earlier.display().to_string()), "{body}");
    assert!(
        body.ends_with(&fs::read_to_string(&earlier).unwrap()),
        "{body}"
    );
    assert!(loop_log::marker(&earlier).exists());
    assert_eq!(run.warn, "");

    run_loop(&f, &idle_script(), &gh).result.unwrap();

    assert_eq!(comments(&gh).len(), 1, "not posted twice");
}

#[test]
fn an_earlier_log_without_rows_is_not_posted_and_is_marked() {
    let f = fixture();
    let earlier = plant_earlier(&f, 600, 0);
    let gh = github(vec![loop_log_issue(40)]);

    run_loop(&f, &idle_script(), &gh).result.unwrap();

    assert_eq!(comments(&gh), []);
    assert_eq!(creates(&gh), []);
    assert!(loop_log::marker(&earlier).exists());
}

#[test]
fn a_log_posted_at_its_stop_is_marked_and_not_posted_again() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);
    run_loop(&f, &Script::new(START - 3_600), &gh)
        .result
        .unwrap();
    assert!(loop_log::marker(&f.log_files()[0]).exists());

    run_loop(&f, &idle_script(), &gh).result.unwrap();

    assert_eq!(comments(&gh).len(), 1, "only the stop's post");
}

#[test]
fn several_unposted_logs_are_posted_oldest_first_one_comment_each_before_any_task() {
    let f = fixture();
    let middle = plant_earlier(&f, 3_600, 1);
    let oldest = plant_earlier(&f, 86_400, 1);
    let newest = plant_earlier(&f, 60, 3);
    let gh = github(vec![loop_log_issue(40)]);

    run_loop(&f, &Script::new(START), &gh).result.unwrap();

    let posted = comments(&gh);
    assert_eq!(posted.len(), 4, "three late, then its own");
    for (comment, log) in posted.iter().zip([&oldest, &middle, &newest]) {
        assert!(comment.1.starts_with(LATE), "{}", comment.1);
        assert!(
            comment.1.contains(&log.display().to_string()),
            "{}",
            comment.1
        );
    }
    assert!(!posted[3].1.contains("Posted late"), "{}", posted[3].1);
    assert!(posted[3].1.contains("| 1 | #3 "), "{}", posted[3].1);
}

#[test]
fn a_failed_late_post_is_a_warning_the_log_stays_unposted_and_the_next_start_posts_it() {
    let f = fixture();
    let earlier = plant_earlier(&f, 86_400, 1);
    let gh = github(vec![loop_log_issue(40)]).failing_issue_comments_on(40);

    let run = run_loop(&f, &idle_script(), &gh);

    run.result.unwrap();
    assert!(run.out.contains("the queue is empty"), "{}", run.out);
    assert_eq!(run.warn.lines().count(), 1, "{}", run.warn);
    assert!(
        run.warn.contains(&earlier.display().to_string()),
        "{}",
        run.warn
    );
    assert!(
        run.warn.contains("comment on issue 40 fails"),
        "{}",
        run.warn
    );
    assert!(!loop_log::marker(&earlier).exists());

    gh.set_issue_comments_failing(40, false);
    let run = run_loop(&f, &idle_script(), &gh);

    run.result.unwrap();
    let posted = comments(&gh);
    assert_eq!(posted.len(), 2, "the failed one, then the retry");
    assert!(posted[1].1.starts_with(LATE), "{}", posted[1].1);
    assert!(loop_log::marker(&earlier).exists());
    assert_eq!(run.warn, "");
}

#[test]
fn with_no_open_loop_log_issue_a_late_post_creates_one_as_the_stop_does() {
    let f = fixture();
    plant_earlier(&f, 86_400, 1);
    plant_earlier(&f, 60, 1);
    let gh = github(Vec::new());

    run_loop(&f, &idle_script(), &gh).result.unwrap();

    let created = creates(&gh);
    assert_eq!(created.len(), 1, "one issue for both");
    assert_eq!(created[0].title, "Loop logs since 2026-11-07");
    assert_eq!(created[0].labels, ["loop-log"]);
    let posted: Vec<u32> = comments(&gh).into_iter().map(|(n, _)| n).collect();
    assert_eq!(posted, [900, 900]);
}

// --- Review M-2: loops that start in the same second ---

#[test]
fn a_loop_starting_in_the_second_of_an_earlier_log_takes_the_next_free_name() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);
    run_loop(&f, &Script::new(START), &gh).result.unwrap();
    gh.set_open_issues(vec![
        open_issue(5, &[READY, "size:S"], ""),
        loop_log_issue(40),
    ]);
    let gh = gh.with_issue_text(issue_text(5));

    // The second loop's clock starts at the same second as the first's.
    let run = run_loop(&f, &Script::new(START), &gh);

    run.result.unwrap();
    let files = f.log_files();
    assert_eq!(
        files,
        [
            f.logs().join("2026-11-08-100000-loop.md"),
            f.logs().join("2026-11-08-100001-loop.md"),
        ]
    );
    let first = fs::read_to_string(&files[0]).unwrap();
    assert!(!first.contains("#5 "), "{first}");
    let second = fs::read_to_string(&files[1]).unwrap();
    assert!(second.contains("| 1 | #5 "), "{second}");
    assert!(!second.contains("#3 "), "{second}");
    let posted = comments(&gh);
    assert_eq!(posted.len(), 2);
    assert!(
        posted[1].1.contains("2026-11-08T10:00:01Z"),
        "{}",
        posted[1].1
    );
    assert!(!posted[1].1.contains("#3 "), "{}", posted[1].1);
}

#[test]
fn a_loop_that_runs_no_task_never_posts_an_earlier_loops_log() {
    let f = fixture();
    let gh = github(vec![loop_log_issue(40)]);
    run_loop(&f, &Script::new(START), &gh).result.unwrap();
    gh.set_open_issues(vec![
        open_issue(5, &[READY, "size:S"], ""),
        loop_log_issue(40),
    ]);
    let gh = gh.with_issue_text(issue_text(5));
    let mut script = Script::new(START);
    script.refused.push(5);

    let run = run_loop(&f, &script, &gh);

    run.result.unwrap();
    assert_eq!(comments(&gh).len(), 1, "only the first loop's");
    assert_eq!(f.log_files().len(), 1);
}

/// Plants earlier loops' logs for the `n` seconds from `START` on.
fn plant_logs(f: &Fixture, n: u64) {
    fs::create_dir_all(f.logs()).unwrap();
    for s in 0..n {
        fs::write(loop_log::path(&f.env.base_dir(), START + s), "earlier\n").unwrap();
    }
}

#[test]
fn with_four_names_taken_the_loop_uses_the_fifth_second() {
    let f = fixture();
    plant_logs(&f, 4);
    let gh = github(vec![loop_log_issue(40)]);

    run_loop(&f, &Script::new(START), &gh).result.unwrap();

    let file = f.logs().join("2026-11-08-100004-loop.md");
    assert!(fs::read_to_string(file).unwrap().contains("| 1 | #3 "));
    assert_eq!(comments(&gh).len(), 1);
}

#[test]
fn with_five_names_taken_the_loop_refuses_before_any_task_and_posts_nothing() {
    let f = fixture();
    plant_logs(&f, 5);
    let gh = github(vec![loop_log_issue(40)]);

    let run = run_loop(&f, &Script::new(START), &gh);

    let err = format!("{:#}", run.result.unwrap_err());
    assert!(err.contains("loop-logs"), "{err}");
    assert_eq!(f.log_files().len(), 5);
    assert!(
        f.log_files()
            .iter()
            .all(|p| fs::read_to_string(p).unwrap() == "earlier\n")
    );
    assert_eq!(gh.calls(), []);
    assert!(!loop_lock(&f).exists(), "the lock is released");
}

fn loop_lock(f: &Fixture) -> PathBuf {
    f.env.base_dir().join(".sbxm").join("task-loop.lock")
}
