//! `sbxm task facts --since <date> [--until <date>] [--json]` (issue 593): one row per task
//! started in the window, the facts a retro gathers (`sdlc-retro` step 2), read from the task
//! records (`.sbxm/tasks/*`) and the archive (`.sbxm/task-archive/*`) only, never from GitHub.
//! Read-only: it never writes, locks or changes a record, and never follows a link out of the
//! base dir. A fact the records don't hold prints `-` (Markdown) or `null` (JSON).
//!
//! A task is placed in the window by its first stamp: from the whole `--since` day to the whole
//! `--until` day (UTC, both inclusive), or to now without `--until`. One task is its id and that
//! start: an archive that copies a live task is the same task (the live
//! record wins), while an archive of an earlier task of the same issue is a row of its own.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::config::GlobalConfig;
use crate::run::results::{format_timestamp, parse_timestamp};
use crate::task::finish::check_own_folder;
use crate::task::record::{self, Kind, Record, RunInfo, Stage, Status};
use crate::task::{merged, queue, review};

pub struct Options {
    /// The first day, `YYYY-MM-DD`.
    pub since: String,
    /// The last day, `YYYY-MM-DD`; `None`: up to now.
    pub until: Option<String>,
    /// A JSON array instead of the Markdown table.
    pub json: bool,
}

/// One task's facts; `None` is a fact its records don't hold.
#[derive(Debug, Serialize)]
struct Row {
    task: String,
    /// The issue an issue task works on; `None` for any other kind.
    issue: Option<u32>,
    started: String,
    /// From the `size:` label in the `issue.md` an issue task saved, live or archived (an archive
    /// made before issue 698 keeps none).
    size: Option<&'static str>,
    review_rounds: usize,
    /// Each saved review round's must-fix count, in order; `None` for one without a readable count.
    must_fix: Vec<Option<u32>>,
    /// From the task's start to its first `ready`.
    wall_to_ready_s: Option<u64>,
    worker_s: Option<u64>,
    worker_cost_usd: Option<f64>,
    reviewer_s: Option<u64>,
    reviewer_cost_usd: Option<f64>,
    outcome: String,
    pr: Option<String>,
}

const COLUMNS: [&str; 13] = [
    "task",
    "issue",
    "started",
    "size",
    "review rounds",
    "must-fix per round",
    "wall to ready (s)",
    "worker (s)",
    "worker cost",
    "reviewer (s)",
    "reviewer cost",
    "outcome",
    "PR",
];

/// Prints the facts of every task started in the window to `out`, one warning line per record
/// that can't be read to `warn`. `now`, in seconds since the epoch, ends a window without
/// `--until`. The dates are checked before anything is read.
pub fn run(
    config_dir: &Path,
    opts: &Options,
    now: u64,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let since = check_date("--since", &opts.since)?;
    let until = opts
        .until
        .as_deref()
        .map(|d| check_date("--until", d))
        .transpose()?;
    if let Some(until) = until
        && since > until
    {
        bail!("--since {since} is after --until {until}");
    }
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;

    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let live = folders(&base_dir, &record::tasks_root(&base_dir), warn)?;
    let archived = folders(&base_dir, &merged::archive_root(&base_dir), warn)?;
    for dir in live.into_iter().chain(archived) {
        let path = dir.join("task.json");
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => {
                writeln!(warn, "warning: {} skipped: it is a link", path.display())?;
                continue;
            }
            // A folder without a record (a task that never got written) holds no task.
            Err(_) => continue,
        }
        let task = match read(&path) {
            Ok(task) => task,
            Err(why) => {
                writeln!(warn, "warning: {} skipped: {why}", path.display())?;
                continue;
            }
        };
        let Some(start) = task.stages.first().and_then(|s| parse_timestamp(&s.at)) else {
            writeln!(
                warn,
                "warning: {} skipped: it has no start time",
                path.display()
            )?;
            continue;
        };
        let started = format_timestamp(start);
        let day = &started[..10];
        // The window ends with the whole of an explicit --until day, else at `now`.
        let before_end = match until {
            Some(until) => day <= until,
            None => start <= now,
        };
        if day < since || !before_end {
            continue;
        }
        // Live folders come first, so a live record wins over its archived copy.
        if !seen.insert((task.id.clone(), started.clone())) {
            continue;
        }
        rows.push(row(&task, &dir, started));
    }
    rows.sort_by(|a, b| (&a.started, &a.task).cmp(&(&b.started, &b.task)));

    if opts.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&rows)?)?;
    } else {
        write_table(out, &rows)?;
    }
    out.flush()?;
    Ok(())
}

/// `value` if it is a real calendar day written `YYYY-MM-DD`.
fn check_date<'a>(flag: &str, value: &'a str) -> Result<&'a str> {
    let real = parse_timestamp(&format!("{value}T00:00:00Z"))
        .is_some_and(|secs| format_timestamp(secs).starts_with(&format!("{value}T")));
    if !real {
        bail!("{flag} {value} isn't a date; write it as YYYY-MM-DD (UTC), e.g. 2026-11-10");
    }
    Ok(value)
}

/// The folders under `root` in name order, each checked as `sbxm status` checks the archive:
/// `root` and the folder are plain folders really under `base_dir`, never links. A link, or a
/// folder whose real place is elsewhere, is passed over with a warning; a plain file is no task.
fn folders(base_dir: &Path, root: &Path, warn: &mut dyn Write) -> Result<Vec<PathBuf>> {
    if fs::symlink_metadata(root).is_err() {
        return Ok(Vec::new());
    }
    if let Some(why) = not_own(base_dir, root) {
        writeln!(warn, "warning: {} skipped: {why}", root.display())?;
        return Ok(Vec::new());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            writeln!(warn, "warning: {} skipped: {e}", root.display())?;
            return Ok(Vec::new());
        }
    };
    let mut dirs = Vec::new();
    for dir in entries.flatten().map(|entry| entry.path()) {
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() && !meta.file_type().is_symlink() {
            continue;
        }
        match not_own(base_dir, &dir) {
            None => dirs.push(dir),
            Some(why) => writeln!(warn, "warning: {} skipped: {why}", dir.display())?,
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Why `dir` isn't read as a folder of `base_dir`: a link, or a real place elsewhere.
fn not_own(base_dir: &Path, dir: &Path) -> Option<String> {
    if fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Some("it is a link".to_owned());
    }
    check_own_folder(base_dir, dir)
        .err()
        .map(|_| format!("it isn't really under {}", base_dir.display()))
}

/// Whether `path` is a file, not a link to one.
fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// The record at `path`, or why it can't be read, on one line.
fn read(path: &Path) -> Result<Record, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("not a task record ({e})"))
}

fn row(task: &Record, dir: &Path, started: String) -> Row {
    let must_fix = review::saved_must_fix(dir);
    let ready = task
        .stages
        .iter()
        .find(|s| s.stage == Stage::Ready)
        .and_then(|s| parse_timestamp(&s.at));
    let worker = task.worker.as_ref().and_then(|a| a.run.as_ref());
    let reviewer = task.reviewer.as_ref().and_then(|a| a.run.as_ref());
    Row {
        task: task.id.clone(),
        issue: (task.kind == Kind::Issue).then_some(task.number),
        size: if task.kind == Kind::Issue {
            size(dir)
        } else {
            None
        },
        review_rounds: must_fix.len(),
        wall_to_ready_s: ready
            .zip(parse_timestamp(&started))
            .map(|(ready, start)| ready.saturating_sub(start)),
        started,
        must_fix,
        worker_s: worker.map(|r| r.duration_s),
        worker_cost_usd: worker.and_then(cost),
        reviewer_s: reviewer.map(|r| r.duration_s),
        reviewer_cost_usd: reviewer.and_then(cost),
        outcome: outcome(task),
        pr: task.pr.clone(),
    }
}

fn cost(run: &RunInfo) -> Option<f64> {
    run.usage.get("cost_usd").and_then(Value::as_f64)
}

/// The size label on the `labels:` line of the `issue.md` (`gh issue view`'s text) in `dir`.
fn size(dir: &Path) -> Option<&'static str> {
    let path = dir.join("issue.md");
    if !plain_file(&path) {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let labels = text.lines().find_map(|l| l.strip_prefix("labels:"))?;
    let labels: Vec<String> = labels.split(',').map(|l| l.trim().to_owned()).collect();
    queue::size(&labels).ok()
}

/// Where the task stands: its stage, with its status unless that is `ok`, and why it stopped
/// when it did.
fn outcome(task: &Record) -> String {
    let mut text = task.stage.name().to_owned();
    if task.status != Status::Ok {
        text.push(' ');
        text.push_str(task.status.name());
    }
    if let Some(stopped) = task.stopped {
        text.push_str(&format!(" ({})", stopped.name()));
    }
    text
}

fn write_table(out: &mut dyn Write, rows: &[Row]) -> Result<()> {
    writeln!(out, "| {} |", COLUMNS.join(" | "))?;
    writeln!(out, "|{}", "---|".repeat(COLUMNS.len()))?;
    for row in rows {
        let must_fix = if row.must_fix.is_empty() {
            "-".to_owned()
        } else {
            row.must_fix
                .iter()
                .map(|n| dash(n.as_ref()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let cells = [
            row.task.clone(),
            dash(row.issue.as_ref()),
            row.started.clone(),
            dash(row.size.as_ref()),
            row.review_rounds.to_string(),
            must_fix,
            dash(row.wall_to_ready_s.as_ref()),
            dash(row.worker_s.as_ref()),
            usd(row.worker_cost_usd),
            dash(row.reviewer_s.as_ref()),
            usd(row.reviewer_cost_usd),
            row.outcome.clone(),
            dash(row.pr.as_ref()),
        ];
        let cells: Vec<String> = cells.iter().map(|c| cell(c)).collect();
        writeln!(out, "| {} |", cells.join(" | "))?;
    }
    Ok(())
}

fn dash(value: Option<&impl ToString>) -> String {
    value.map_or_else(|| "-".to_owned(), ToString::to_string)
}

fn usd(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |usd| format!("${usd:.2}"))
}

/// `text` as one table cell: a `|` would end the cell and a line break the row.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}
