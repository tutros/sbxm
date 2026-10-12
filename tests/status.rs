//! `sbxm status [--watch]` (decision 171, issue 90): the Tasks section. Pure rendering is
//! golden-tested with a passed-in clock; `rows()` is tested against on-disk task fixtures like
//! `tests/task_status.rs`; the CLI and the `--watch` loop are tested at the edges only (a
//! non-terminal always prints once, `backend` is never called).

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::Env as TestEnv;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env};
use sbxm::status::tasks::{self, TaskRow};
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

const T0: u64 = 1_790_000_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn task(kind: Kind, number: u32, title: &str) -> Record {
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title,
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, record: &Record) {
    record::write(&record::task_dir(base, &record.id), record).unwrap();
}

fn working(number: u32) -> Record {
    let mut record = task(Kind::Issue, number, &format!("title {number}"));
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record
}

fn row(id: &str, stage: &'static str, status: &str) -> TaskRow {
    TaskRow {
        id: id.to_owned(),
        stage,
        status: status.to_owned(),
        stage_entered_at: None,
        commits_ahead: None,
        must_fix: None,
        pr: None,
        log_line: None,
    }
}

// ---- pure rendering (golden output, a passed-in clock) --------------------

#[test]
fn no_tasks_says_how_to_start_one_like_task_status_does() {
    assert_eq!(
        tasks::render(&[], 0, None),
        "No tasks yet; start one with `sbxm task start --issue <n>`.\n"
    );
}

#[test]
fn every_missing_value_shows_a_dash() {
    let r = row("issue-7", "working", "running");
    let text = tasks::render(&[r], 100, None);

    insta::assert_snapshot!(text, @r"
    ID       STAGE    STATUS   TIME  AHEAD  FINDINGS  PR  LOG
    issue-7  working  running  -     -      -         -   -
    ");
}

#[test]
fn present_values_fill_every_column() {
    let r = TaskRow {
        id: "issue-7".to_owned(),
        stage: "reviewing",
        status: "running".to_owned(),
        stage_entered_at: Some(40),
        commits_ahead: Some(2),
        must_fix: Some(1),
        pr: Some("https://github.com/o/r/pull/9".to_owned()),
        log_line: Some("[reviewing] checking the diff".to_owned()),
    };
    let text = tasks::render(&[r], 100, None);

    insta::assert_snapshot!(text, @r"
    ID       STAGE      STATUS   TIME  AHEAD  FINDINGS  PR                             LOG
    issue-7  reviewing  running  1m    2      1         https://github.com/o/r/pull/9  [reviewing] checking the diff
    ");
}

#[test]
fn an_interrupted_status_shows_as_such() {
    let r = row("issue-41", "working", "interrupted");
    let text = tasks::render(&[r], 0, None);
    assert!(text.contains("interrupted"), "{text}");
}

#[test]
fn rows_are_each_on_their_own_line() {
    let a = row("issue-1", "working", "running");
    let b = row("issue-2", "ready", "ok");
    let text = tasks::render(&[a, b], 0, None);
    assert_eq!(text.lines().count(), 3, "{text}"); // header + 2 rows
}

#[test]
fn time_formats_seconds_minutes_hours_and_days() {
    let cases: [(u64, &str); 4] = [
        (45, "45s"),
        (12 * 60, "12m"),
        (3_665, "1h01m"),
        (3 * 86_400 + 2 * 3_600, "3d02h"),
    ];
    for (elapsed, expect) in cases {
        let mut r = row("issue-1", "working", "running");
        r.stage_entered_at = Some(0);
        let text = tasks::render(&[r], elapsed, None);
        let line = text.lines().nth(1).unwrap();
        let time_cell = line.split_whitespace().nth(3).unwrap();
        assert_eq!(time_cell, expect, "{line}");
    }
}

#[test]
fn a_terminal_width_cuts_only_the_log_column_with_an_ellipsis() {
    let mut r = row("issue-1", "working", "running");
    r.log_line =
        Some("a very long diagnostic line that will not fit on a narrow terminal".to_owned());
    let full = tasks::render(&[r.clone()], 0, None);
    let full_line = full.lines().nth(1).unwrap();
    assert!(full_line.ends_with(r.log_line.as_deref().unwrap()));

    let cut = tasks::render(&[r], 0, Some(70));
    let cut_line = cut.lines().nth(1).unwrap();
    assert_eq!(cut_line.chars().count(), 70, "{cut_line:?}");
    assert!(cut_line.ends_with('…'), "{cut_line:?}");
    assert!(
        full_line.starts_with(cut_line.trim_end_matches('…')),
        "{cut_line:?} vs {full_line:?}"
    );
}

#[test]
fn a_non_terminal_never_cuts_the_log_column() {
    let mut r = row("issue-1", "working", "running");
    r.log_line = Some("x".repeat(500));
    let text = tasks::render(&[r], 0, None);
    assert!(text.lines().nth(1).unwrap().ends_with(&"x".repeat(500)));
}

#[test]
fn a_narrow_width_still_produces_one_line_per_row() {
    let r = row("issue-1", "working", "running");
    let text = tasks::render(&[r], 0, Some(1));
    assert_eq!(text.lines().count(), 2, "{text}");
}

#[test]
fn a_full_row_at_terminal_width_still_shows_an_ellipsis_instead_of_losing_the_log() {
    let r = TaskRow {
        id: "issue-7".to_owned(),
        stage: "reviewing",
        status: "running".to_owned(),
        stage_entered_at: Some(60),
        commits_ahead: Some(2),
        must_fix: Some(1),
        pr: Some("https://github.com/tutros/sbxm/pull/999".to_owned()),
        log_line: Some("still working".to_owned()),
    };
    let text = tasks::render(&[r], 120, Some(80));
    let line = text.lines().nth(1).unwrap();

    assert!(
        line.chars().count() <= 80,
        "{line:?} is {} columns wide",
        line.chars().count()
    );
    assert!(line.ends_with('…'), "the log must not vanish: {line:?}");
}

/// Terminal columns of `line`, counted by hand for the characters these tests use (so the
/// expectation doesn't come from the width table under test): CJK ideographs and emoji take 2
/// cells, `❤` followed by the emoji variation selector too (alone it's 1), anything else 1.
fn cells(line: &str) -> usize {
    line.replace("\u{2764}\u{FE0F}", "😀")
        .chars()
        .map(|c| match c {
            '\u{4E00}'..='\u{9FFF}' | '\u{1F300}'..='\u{1FAFF}' => 2,
            '\u{0300}'..='\u{036F}' => 0,
            _ => 1,
        })
        .sum()
}

/// One log made of `log` repeated until it's wider than the terminal, cut at width 80: the
/// line fits in 80 cells and still ends with the `…`.
fn assert_wide_log_fits(log: &str) {
    let mut r = row("issue-1", "working", "running");
    r.log_line = Some(log.repeat(60));
    let text = tasks::render(&[r], 0, Some(80));
    let line = text.lines().nth(1).unwrap();

    assert!(line.ends_with('…'), "{line:?}");
    assert!(
        cells(line) <= 80,
        "{line:?} occupies {} terminal columns, wider than the requested 80",
        cells(line)
    );
}

#[test]
fn a_terminal_width_measures_display_columns_not_unicode_scalar_values() {
    assert_wide_log_fits("界");
}

#[test]
fn an_emoji_log_respects_the_terminal_display_width() {
    assert_wide_log_fits("😀");
}

#[test]
fn an_emoji_with_a_variation_selector_respects_the_terminal_display_width() {
    assert_wide_log_fits("\u{2764}\u{FE0F}");
}

#[test]
fn a_combining_mark_takes_no_terminal_column() {
    let mut r = row("issue-1", "working", "running");
    r.log_line = Some("e\u{301}".repeat(30));
    let uncut = tasks::render(&[r.clone()], 0, None);
    let line = uncut.lines().nth(1).unwrap();

    let text = tasks::render(&[r], 0, Some(cells(line)));

    assert_eq!(
        text, uncut,
        "a line exactly as wide as the terminal was cut"
    );
}

#[test]
fn a_wide_character_in_a_fixed_column_keeps_the_log_column_aligned() {
    let mut wide = row("issue-1", "working", "running");
    wide.pr = Some("界界".to_owned());
    wide.log_line = Some("one".to_owned());
    let mut plain = row("issue-2", "working", "running");
    plain.pr = Some("x".to_owned());
    plain.log_line = Some("two".to_owned());
    let text = tasks::render(&[wide, plain], 0, None);
    let starts: Vec<usize> = text
        .lines()
        .map(|line| {
            let (before, _) = line.rsplit_once("  ").unwrap();
            cells(before) + 2
        })
        .collect();

    assert!(
        starts.windows(2).all(|w| w[0] == w[1]),
        "the LOG column starts at different columns {starts:?}:\n{text}"
    );
}

// ---- gathering rows from task.json/run.log ---------------------------------

#[test]
fn an_interrupted_running_task_is_flagged_without_ever_reading_a_process_pid_that_matches() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41));

    let rows = tasks::rows(base.path(), &Probe(None)).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "interrupted");
}

#[test]
fn a_live_process_shows_the_real_status() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].status, "running");
}

#[test]
fn must_fix_comes_from_the_last_recorded_review() {
    let base = tempfile::tempdir().unwrap();
    let mut record = working(1);
    record.finish(Status::Completed).unwrap();
    record.review = Some(record::ReviewResult {
        round: 1,
        must_fix: 3,
        full: true,
        repeat: false,
        risk: sbxm::task::risk::Level::Medium,
        risk_reasons: vec![],
    });
    save(base.path(), &record);

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].must_fix, Some(3));
}

#[test]
fn no_review_yet_is_a_missing_must_fix_count() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].must_fix, None);
}

#[test]
fn the_pr_url_is_carried_through_in_full() {
    let base = tempfile::tempdir().unwrap();
    let mut record = working(1);
    record.pr = Some("https://github.com/o/r/pull/12".to_owned());
    save(base.path(), &record);

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(
        rows[0].pr,
        Some("https://github.com/o/r/pull/12".to_owned())
    );
}

#[test]
fn no_pr_yet_is_a_missing_value() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].pr, None);
}

#[test]
fn the_last_non_header_run_log_line_is_shown_without_its_timestamp() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));
    fs::write(
        record::task_dir(base.path(), "issue-1").join("run.log"),
        "# 2026-09-21T14:13:20Z task status | exe sbxm | sha256 x | commit y | pid 1\n\
         2026-09-21T14:13:20Z [working] starting\n\
         2026-09-21T14:14:00Z [working] still going\n",
    )
    .unwrap();

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].log_line, Some("[working] still going".to_owned()));
}

#[test]
fn a_run_log_with_only_a_header_has_no_line_to_show() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));
    fs::write(
        record::task_dir(base.path(), "issue-1").join("run.log"),
        "# 2026-09-21T14:13:20Z task status | exe sbxm | sha256 x | commit y | pid 1\n",
    )
    .unwrap();

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].log_line, None);
}

#[test]
fn no_run_log_yet_is_a_missing_value() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].log_line, None);
}

#[test]
fn rows_are_in_the_same_order_task_status_lists_them() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41));
    save(base.path(), &working(7));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["issue-7", "issue-41"]
    );
}

#[test]
fn a_task_without_a_repo_yet_shows_no_commit_count() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41));

    let rows = tasks::rows(base.path(), &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].commits_ahead, None);
}

// ---- the command: writes nothing, the backend is the fake, the watch loop -

fn real_env(terminal: bool, width: usize, iterations: Option<usize>) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width,
        iterations,
    }
}

#[test]
fn the_command_writes_nothing_to_disk() {
    let env = TestEnv::new();
    save(&env.base_dir(), &working(1));
    let before: Vec<_> = walk(&env.base_dir());

    let backend = FakeBackend::default();
    let mut out = Vec::new();
    status::run(
        &env.base_dir(),
        &backend,
        &Probe(Some(T0)),
        &real_env(false, 80, None),
        None,
        &mut out,
    )
    .unwrap();

    let after: Vec<_> = walk(&env.base_dir());
    assert_eq!(before, after);
}

fn walk(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut found = Vec::new();
    for entry in walkdir(dir) {
        if entry.is_file() {
            found.push((
                entry.clone(),
                fs::read_to_string(&entry).unwrap_or_default(),
            ));
        }
    }
    found.sort();
    found
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walkdir(&path));
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn the_backend_is_never_called() {
    let env = TestEnv::new();
    save(&env.base_dir(), &working(1));
    let backend = FakeBackend::default();
    let mut out = Vec::new();

    status::run(
        &env.base_dir(),
        &backend,
        &Probe(Some(T0)),
        &real_env(false, 80, None),
        None,
        &mut out,
    )
    .unwrap();

    assert!(backend.log().is_empty());
}

#[test]
fn without_watch_it_prints_once_whether_or_not_it_is_a_terminal() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));
    let backend = FakeBackend::default();

    for terminal in [false, true] {
        let mut out = Vec::new();
        status::run(
            base.path(),
            &backend,
            &Probe(Some(T0)),
            &real_env(terminal, 80, None),
            None,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(!text.contains('\x1B'), "{text:?}");
    }
}

#[test]
fn watch_on_a_non_terminal_prints_once_and_returns() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));
    let backend = FakeBackend::default();
    let env = Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {
            panic!("must not sleep on a non-terminal");
        }),
        terminal: false,
        width: 80,
        iterations: None,
    };
    let mut out = Vec::new();

    status::run(
        base.path(),
        &backend,
        &Probe(Some(T0)),
        &env,
        Some(Duration::from_secs(2)),
        &mut out,
    )
    .unwrap();

    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
}

#[test]
fn watch_on_a_terminal_clears_the_screen_and_sleeps_between_frames() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1));
    let backend = FakeBackend::default();
    let sleep_calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&sleep_calls);
    let env = Env {
        now: Box::new(|| T0),
        sleep: Box::new(move |d| {
            counted.fetch_add(1, Ordering::SeqCst);
            assert_eq!(d, Duration::from_secs(2));
        }),
        terminal: true,
        width: 80,
        iterations: Some(3),
    };
    let mut out = Vec::new();

    status::run(
        base.path(),
        &backend,
        &Probe(Some(T0)),
        &env,
        Some(Duration::from_secs(2)),
        &mut out,
    )
    .unwrap();

    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.matches("\x1B[2J\x1B[H").count(), 3, "{text:?}");
    assert_eq!(
        sleep_calls.load(Ordering::SeqCst),
        2,
        "3 frames means 2 sleeps between them, then the loop stops"
    );
}

// ---- the CLI --------------------------------------------------------------

#[test]
fn the_cli_prints_no_tasks_with_an_empty_base_dir() {
    let env = TestEnv::new();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["status"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("No tasks yet"));
}

#[test]
fn the_cli_lists_a_task_and_exits_promptly_even_with_watch() {
    let env = TestEnv::new();
    save(&env.base_dir(), &working(1));

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["status", "--watch"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("issue-1"), "{stdout}");
}

#[test]
fn interval_without_watch_is_refused() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["status", "--interval", "5"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--watch"), "{stderr}");
}

#[test]
fn interval_below_one_second_is_refused() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["status", "--watch", "--interval", "0"])
        .output()
        .unwrap();

    assert!(!output.status.success());
}
