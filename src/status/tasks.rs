//! The Tasks section of `sbxm status` (decision 171, issue 90): one row per task, read straight
//! from `task.json` and `run.log`. Never calls `sbx` (decision 171's point 3): a missing sandbox
//! belongs to the Sandboxes section (issue 92).

use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::run::results::parse_timestamp;
use crate::task::record::{self, ProcessProbe};

use super::width_table::{EMOJI_PRESENTATION, WIDTHS};

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

/// Whether `c` falls in one of `ranges`' `(first, last, ..)` runs, which are in order.
fn in_runs<T>(ranges: &[T], c: char, bounds: impl Fn(&T) -> (u32, u32)) -> Option<&T> {
    let u = u32::from(c);
    let i = ranges.partition_point(|r| bounds(r).1 < u);
    ranges.get(i).filter(|r| bounds(r).0 <= u)
}

/// `c`'s width in terminal columns on its own, from the generated table (decision 185).
fn char_width(c: char) -> usize {
    in_runs(WIDTHS, c, |r| (r.0, r.1)).map_or(1, |r| usize::from(r.2))
}

/// `text`'s width in terminal columns (not its byte length or its count of Unicode scalar
/// values, either of which undercounts a wide character like `界` or `😀`), from the table
/// `scripts/gen-display-width` generates out of `unicode-width`'s (decision 185): every
/// character's own width, and U+FE0F widening an emoji to 2 columns. Other sequences (a ZWJ
/// emoji, a text-presentation selector) count as the sum of their characters, never less than
/// `unicode-width` measures them, so a row can't overflow. Every column measurement in
/// this module goes through it, padding and cutting alike.
fn display_width(text: &str) -> usize {
    let mut total = 0;
    let mut previous = None;
    for c in text.chars() {
        if c == '\u{FE0F}'
            && let Some(p) = previous
            && in_runs(EMOJI_PRESENTATION, p, |r| *r).is_some()
        {
            total += 2_usize.saturating_sub(char_width(p));
        }
        total += char_width(c);
        previous = Some(c);
    }
    total
}

/// `text`, or cut to at most `max` terminal columns with a trailing `…` (always 1 column) if it's
/// wider; empty if `max` is 0. The cut is checked with [`display_width`] itself, so a character
/// whose width depends on what follows it (a variation selector) can't push it past `max`.
fn truncate(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1;
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = char_width(c);
        if used + w > budget {
            break;
        }
        cut.push(c);
        used += w;
    }
    while display_width(&cut) > budget {
        cut.pop();
    }
    cut.push('…');
    cut
}

/// `cell` followed by spaces up to `width` terminal columns (`format!`'s own padding counts
/// characters, which leaves a wide character's column short).
fn pad(cell: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(cell));
    format!("{cell}{}", " ".repeat(fill))
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

/// The fixed cells padded to `widths`, each followed by `sep` spaces (2 is the normal, readable
/// gap; a terminal that's too narrow for even that falls back to 1, then 0, before giving up on
/// separating columns at all).
fn build_fixed(cells: &[String; 7], widths: &[usize; 7], sep: usize) -> String {
    let gap = " ".repeat(sep);
    let mut line = String::new();
    for (cell, width) in cells.iter().zip(widths) {
        line.push_str(&pad(cell, *width));
        line.push_str(&gap);
    }
    line
}

/// One line: the fixed cells padded to `widths`, then `log`, cut to fit `total_width` when given
/// (a terminal); shown in full when not (a pipe or a file, decided point 1). On a terminal too
/// narrow for the fixed cells at the normal 2-space gap, the gap is compacted (1 space, then none)
/// before the log loses any more than its own `…`; `log_is_real` marks a log worth that guarantee
/// (a task's actual last line, not the header's "LOG" label or the dash for no log yet).
fn push_row(
    out: &mut String,
    cells: &[String; 7],
    log: &str,
    log_is_real: bool,
    widths: &[usize; 7],
    total_width: Option<usize>,
) {
    let line = match total_width {
        None => build_fixed(cells, widths, 2) + log,
        Some(total) => {
            let mut fixed = build_fixed(cells, widths, 2);
            let mut available = total.saturating_sub(display_width(&fixed));
            for sep in [1, 0] {
                if available > 0 {
                    break;
                }
                fixed = build_fixed(cells, widths, sep);
                available = total.saturating_sub(display_width(&fixed));
            }
            if available > 0 {
                fixed + &truncate(log, available)
            } else if log_is_real {
                fixed + "…"
            } else {
                fixed
            }
        }
    };
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
    let mut widths = HEADER.map(display_width);
    for row in &data {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(display_width(cell));
        }
    }
    let mut out = String::new();
    push_row(
        &mut out,
        &HEADER.map(str::to_owned),
        "LOG",
        false,
        &widths,
        width,
    );
    for (cells, row) in data.iter().zip(rows) {
        let log_is_real = row.log_line.as_deref().is_some_and(|line| !line.is_empty());
        let log = row.log_line.as_deref().unwrap_or(DASH);
        push_row(&mut out, cells, log, log_is_real, &widths, width);
    }
    out
}
