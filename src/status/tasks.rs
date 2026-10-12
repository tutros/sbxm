//! The Tasks section of `sbxm status` (decision 171, issue 90): one row per task, read straight
//! from `task.json` and `run.log`. Never calls `sbx` (decision 171's point 3): a missing sandbox
//! belongs to the Sandboxes section (issue 92).

use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::run::results::parse_timestamp;
use crate::task::record::{self, ProcessProbe};

/// A missing value (no commits yet, no review yet, no PR, no `run.log`).
const DASH: &str = "-";

/// One task, as [`render`] shows it. Gathered by [`rows`]; `render` adds nothing further except
/// formatting, so it stays pure and testable with a passed-in clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub stage: &'static str,
    /// `"interrupted"` when the task says "working" (or any other running status) with no live
    /// process, else the status name, exactly as `sbxm task status` decides it.
    pub status: String,
    /// When the task entered `stage` (its last stage stamp in `task.json`); `None` only for a
    /// stamp this build can't parse (never happens for a stamp `sbxm` itself wrote).
    pub stage_entered_at: Option<u64>,
    pub commits_ahead: Option<u32>,
    /// From the last review recorded in `task.json` (`review.must_fix`), not re-derived from
    /// `review.md`.
    pub must_fix: Option<u32>,
    /// The PR's URL, in full, as `task.json` recorded it.
    pub pr: Option<String>,
    /// The last `run.log` line that isn't a header, without its timestamp.
    pub log_line: Option<String>,
}

/// Every task's row, in the same order `sbxm task status` lists them (`record::load_all`'s own
/// order). Reads only `task.json`, `repo.git` and `run.log`; never calls `sbx`.
pub fn rows(base: &Path, probe: &dyn ProcessProbe) -> Result<Vec<TaskRow>> {
    record::load_all(base)?
        .into_iter()
        .map(|record| {
            let dir = record::task_dir(base, &record.id);
            let status = if record.is_interrupted(probe) {
                "interrupted".to_owned()
            } else {
                record.status.name().to_owned()
            };
            let stage_entered_at = record
                .stages
                .last()
                .and_then(|stamp| parse_timestamp(&stamp.at));
            Ok(TaskRow {
                id: record.id.clone(),
                stage: record.stage.name(),
                status,
                stage_entered_at,
                commits_ahead: crate::commands::commits_ahead(base, &record),
                must_fix: record.review.as_ref().map(|r| r.must_fix),
                pr: record.pr.clone(),
                log_line: last_log_line(&dir.join("run.log")),
            })
        })
        .collect()
}

/// The last line of `run.log` that isn't a header (`# ...`), with its leading timestamp removed;
/// `None` if the file doesn't exist, or has no such line.
fn last_log_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|line| !line.starts_with("# "))?;
    Some(strip_timestamp(line).to_owned())
}

/// A `run.log` line is `"<timestamp> [<stage>] <text>"`; this drops the timestamp and the space
/// after it, keeping the stage bracket (decided: "shown without its timestamp").
fn strip_timestamp(line: &str) -> &str {
    line.split_once(' ').map_or(line, |(_, rest)| rest)
}

/// `45s`, `12m`, `1h05m`, `3d02h` (decided format).
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h{:02}m", secs / 3_600, secs % 3_600 / 60)
    } else {
        format!("{}d{:02}h", secs / 86_400, secs % 86_400 / 3_600)
    }
}

/// `text`, or cut to `max` characters with a trailing `…` if it's longer; empty if `max` is 0.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

const HEADER: [&str; 7] = ["ID", "STAGE", "STATUS", "TIME", "AHEAD", "FINDINGS", "PR"];

/// The fixed columns of one row (everything but the log line, which is last and variable).
fn fixed_cells(row: &TaskRow, now: u64) -> [String; 7] {
    [
        row.id.clone(),
        row.stage.to_owned(),
        row.status.clone(),
        row.stage_entered_at.map_or_else(
            || DASH.to_owned(),
            |at| format_duration(now.saturating_sub(at)),
        ),
        row.commits_ahead
            .map_or_else(|| DASH.to_owned(), |n| n.to_string()),
        row.must_fix
            .map_or_else(|| DASH.to_owned(), |n| n.to_string()),
        row.pr.clone().unwrap_or_else(|| DASH.to_owned()),
    ]
}

/// One line: the fixed cells padded to `widths`, then `log`, cut to fit `total_width` when given
/// (a terminal); shown in full when not (a pipe or a file, decided point 1).
fn push_row(
    out: &mut String,
    cells: &[String; 7],
    log: &str,
    widths: &[usize; 7],
    total_width: Option<usize>,
) {
    let mut line = String::new();
    for (cell, width) in cells.iter().zip(widths) {
        line.push_str(&format!("{cell:width$}  "));
    }
    match total_width {
        Some(total) => line.push_str(&truncate(log, total.saturating_sub(line.len()))),
        None => line.push_str(log),
    }
    out.push_str(&line);
    out.push('\n');
}

/// The Tasks table: pure and total over `rows`, given `now` (the clock) and `width` (the
/// terminal's column count, `None` when output isn't a terminal). No tasks prints the same line
/// `sbxm task status` does.
pub fn render(rows: &[TaskRow], now: u64, width: Option<usize>) -> String {
    if rows.is_empty() {
        return "No tasks yet; start one with `sbxm task start --issue <n>`.\n".to_owned();
    }
    let data: Vec<[String; 7]> = rows.iter().map(|row| fixed_cells(row, now)).collect();
    let mut widths = HEADER.map(str::len);
    for row in &data {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    let mut out = String::new();
    push_row(&mut out, &HEADER.map(str::to_owned), "LOG", &widths, width);
    for (cells, row) in data.iter().zip(rows) {
        let log = row.log_line.as_deref().unwrap_or(DASH);
        push_row(&mut out, cells, log, &widths, width);
    }
    out
}
