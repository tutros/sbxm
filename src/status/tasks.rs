//! The Tasks section of `sbxm status` (decision 171, issue 90): one row per task, read straight
//! from `task.json`, `repo.git` and `run.log`. Never calls `sbx` (decision 171's point 3): a
//! missing sandbox belongs to the Sandboxes section (issue 92).

use std::fs;
use std::path::Path;

use anyhow::Result;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::run::results::parse_timestamp;
use crate::task::lock;
use crate::task::next;
use crate::task::queue::LOOP_HOOK;
use crate::task::record::{self, ProcessProbe, Stage, Status};

/// A missing value (no commits yet, no review yet, no PR, no `run.log`).
const DASH: &str = "-";

/// One task, as [`render`] shows it. Gathered by [`rows`]; `render` adds nothing further except
/// formatting, so it stays pure and testable with a passed-in clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    /// The title `task.json` stored (issue 246); empty for a record without one.
    pub title: String,
    pub stage: &'static str,
    /// `"interrupted"` when the task says "working" (or any other running status) with no live
    /// process, else the status name, exactly as `sbxm task status` decides it.
    pub status: String,
    /// What the TIME column shows (issue 484).
    pub time: TaskTime,
    pub commits_ahead: Option<u32>,
    /// From the last review recorded in `task.json` (`review.must_fix`), not re-derived from
    /// `review.md`.
    pub must_fix: Option<u32>,
    /// The PR's URL, in full, as `task.json` recorded it.
    pub pr: Option<String>,
    /// Where the task runs (issue 671): `local`, or `cloud` with its sandbox still there in
    /// brackets; `None` for an older record that has no location field.
    pub location: Option<String>,
    /// Set when the task is parked (issue 671): what to run next, shown under the table.
    pub parked: Option<Parked>,
    /// The last `run.log` line that isn't a header, without its timestamp.
    pub log_line: Option<String>,
}

/// A task the loop parked at `ready` (issue 671): it took the task to `ready` and claimed it
/// for `task finish` (its marker, [`LOOP_HOOK`]), and the finish left no PR (the workflow-file
/// guard refused it, say), so it waits on the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    /// The command to run next, as `task run` names it for a task at `ready`.
    pub next: String,
    /// The checkout the task was started in, where that command reads its `sbxm-task.toml`;
    /// `None` in an older record.
    pub checkout: Option<String>,
}

/// `record`'s [`Parked`], or `None` when it isn't parked: a task at `ready` the loop claimed,
/// with no PR, whose lock no live command holds (a finish running on it isn't parked).
fn parked(record: &record::Record, dir: &Path, probe: &dyn ProcessProbe) -> Option<Parked> {
    let claimed = record.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true));
    if record.stage != Stage::Ready
        || record.status != Status::Ok
        || record.pr.is_some()
        || !claimed
        || lock::held_alive(dir, probe)
    {
        return None;
    }
    Some(Parked {
        next: next::ready(record, &next::record_flag(record))?,
        checkout: record.checkout.clone(),
    })
}

/// The TIME column of one task (issue 484).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTime {
    /// A task still going: when it entered its stage (its last stage stamp in `task.json`), shown
    /// as the time since then; `None` only for a stamp this build can't parse (never happens for
    /// a stamp `sbxm` itself wrote).
    InStage(Option<u64>),
    /// A task that has ended (`finished`, stopped at `ready`, or `failed`): its total run time in
    /// seconds, from its first `prepared` stamp to the stamp of the stage it ended in (a failed
    /// task's `ended_at`, issue 492), fixed whatever the clock says; `None` when the record lacks
    /// either time (an older `task.json`).
    Took(Option<u64>),
}

/// `record`'s [`TaskTime`]: a total for a task that is `finished`, `ready` with `stopped` set (it
/// ended there with something unresolved) or `failed`, else the time in its current stage.
fn task_time(record: &record::Record) -> TaskTime {
    let at = |stamp: Option<&record::Stamp>| stamp.and_then(|stamp| parse_timestamp(&stamp.at));
    let end = if record.status == Status::Failed {
        record.ended_at.as_deref().and_then(parse_timestamp)
    } else if record.stage == Stage::Finished
        || (record.stage == Stage::Ready && record.stopped.is_some())
    {
        at(record.stages.iter().rev().find(|s| s.stage == record.stage))
    } else {
        return TaskTime::InStage(at(record.stages.last()));
    };
    let start = at(record.stages.iter().find(|s| s.stage == Stage::Prepared));
    TaskTime::Took(start.zip(end).map(|(start, end)| end.saturating_sub(start)))
}

/// Every task's row, in the same order `sbxm task status` lists them (`record::load_all`'s own
/// order). Reads only `task.json`, `repo.git` and `run.log`; never calls `sbx`.
pub fn rows(base: &Path, probe: &dyn ProcessProbe) -> Result<Vec<TaskRow>> {
    record::load_all(base)?
        .into_iter()
        .map(|record| {
            let dir = record::task_dir(base, &record.id);
            let parked = parked(&record, &dir, probe);
            let status = if record.is_interrupted(probe) && !lock::held_alive(&dir, probe) {
                "interrupted".to_owned()
            } else if parked.is_some() {
                "parked".to_owned()
            } else {
                record.status.name().to_owned()
            };
            Ok(TaskRow {
                id: record.id.clone(),
                title: record.title.clone(),
                stage: record.stage.name(),
                status,
                time: task_time(&record),
                commits_ahead: crate::commands::commits_ahead(base, &record),
                must_fix: record.review.as_ref().map(|r| r.must_fix),
                pr: record.pr.clone(),
                location: location(&dir, &record),
                parked,
                log_line: last_log_line(&dir.join("run.log")),
            })
        })
        .collect()
}

/// What the WHERE column shows for `record` (issue 671): `local`, `cloud (<sandbox>)` for the
/// cloud sandbox it has now (its last one not removed, else the one being created), or `cloud`
/// with none. `None` when its `task.json` in `dir` has no `location` field (a record from before
/// milestone 4, which reads as local), so the column shows a dash rather than a guess.
fn location(dir: &Path, record: &record::Record) -> Option<String> {
    let text = fs::read_to_string(dir.join("task.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("location")?;
    let sandbox = record
        .sandboxes
        .iter()
        .rev()
        .find(|s| s.removed_at.is_none())
        .map(|s| &s.name)
        .or(record.creating.as_ref().map(|c| &c.name));
    Some(match (record.location, sandbox) {
        (crate::backend::Location::Cloud, Some(name)) => format!("cloud ({name})"),
        (location, _) => location.as_str().to_owned(),
    })
}

/// The last line of `run.log` that isn't a header (`# ...`), with its leading timestamp removed;
/// `None` if the file doesn't exist, or has no such line.
pub(crate) fn last_log_line(path: &Path) -> Option<String> {
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
pub fn format_duration(secs: u64) -> String {
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

/// `text`'s width in terminal columns (not its byte length or its count of Unicode scalar
/// values, either of which undercounts a wide character like `界` or `😀`), from the Unicode
/// width tables `unicode-width` carries, emoji presentation sequences included. Every column
/// measurement in this module goes through it, padding and cutting alike.
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `text`, or cut to at most `max` terminal columns with a trailing `…` (always 1 column) if it's
/// wider; empty if `max` is 0. The cut is checked with [`display_width`] itself, so a character
/// whose width depends on what follows it (a variation selector) can't push it past `max`.
pub(crate) fn truncate(text: &str, max: usize) -> String {
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
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
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
pub(crate) fn pad(cell: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(cell));
    format!("{cell}{}", " ".repeat(fill))
}

const HEADER: [&str; 8] = [
    "ID", "STAGE", "STATUS", "TIME", "AHEAD", "FINDINGS", "PR", "WHERE",
];

/// The fixed columns of one row (everything but the log line, which is last and variable).
fn fixed_cells(row: &TaskRow, now: u64) -> [String; 8] {
    [
        row.id.clone(),
        row.stage.to_owned(),
        row.status.clone(),
        match row.time {
            TaskTime::InStage(at) => at.map(|at| now.saturating_sub(at)),
            TaskTime::Took(secs) => secs,
        }
        .map_or_else(|| DASH.to_owned(), format_duration),
        row.commits_ahead
            .map_or_else(|| DASH.to_owned(), |n| n.to_string()),
        row.must_fix
            .map_or_else(|| DASH.to_owned(), |n| n.to_string()),
        row.pr.clone().unwrap_or_else(|| DASH.to_owned()),
        row.location.clone().unwrap_or_else(|| DASH.to_owned()),
    ]
}

/// The fixed cells padded to `widths`, each followed by `sep` spaces (2 is the normal, readable
/// gap; a terminal that's too narrow for even that falls back to 1, then 0, before giving up on
/// separating columns at all).
fn build_fixed<const N: usize>(cells: &[String; N], widths: &[usize; N], sep: usize) -> String {
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
pub(crate) fn push_row<const N: usize>(
    out: &mut String,
    cells: &[String; N],
    log: &str,
    log_is_real: bool,
    widths: &[usize; N],
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

/// The fewest columns the title is shown in on a terminal (issue 246): with less left, the column
/// is dropped and the ID alone names the task.
const MIN_TITLE: usize = 10;

/// The title column's width, or `None` to leave it out: off a terminal (`total_width` `None`) as
/// wide as the longest title, never cut; on one, at most the space left after the other fixed
/// columns, the title's own gap and one column for the log's `…`, and dropped below
/// [`MIN_TITLE`]. Left out too when no task has a title (a record written without one).
fn title_width(rows: &[TaskRow], fixed: &[usize; 8], total_width: Option<usize>) -> Option<usize> {
    let longest = rows
        .iter()
        .map(|row| display_width(row.title.trim()))
        .max()
        .filter(|&w| w > 0)?
        .max(display_width(TITLE));
    let Some(total) = total_width else {
        return Some(longest);
    };
    let others = fixed.iter().map(|w| w + 2).sum::<usize>();
    let left = total.saturating_sub(others + 2 + 1);
    (left >= MIN_TITLE).then(|| longest.min(left))
}

const TITLE: &str = "TITLE";

/// The cells of one line with the title (cut to `width`) after the ID.
fn with_title(cells: &[String; 8], title: &str, width: usize) -> [String; 9] {
    let [id, stage, status, time, ahead, findings, pr, place] = cells.clone();
    [
        id,
        truncate(title, width),
        stage,
        status,
        time,
        ahead,
        findings,
        pr,
        place,
    ]
}

/// The Tasks table: pure and total over `rows`, given `now` (the clock) and `width` (the
/// terminal's column count, `None` when output isn't a terminal). No tasks prints the same line
/// `sbxm task status` does. A title column follows the ID (issue 246; see [`title_width`]).
pub fn render(rows: &[TaskRow], now: u64, width: Option<usize>) -> String {
    if rows.is_empty() {
        return crate::hint::named("No tasks yet; start one with `sbxm task start --issue <n>`.\n");
    }
    let data: Vec<[String; 8]> = rows.iter().map(|row| fixed_cells(row, now)).collect();
    let mut widths = HEADER.map(display_width);
    for row in &data {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(display_width(cell));
        }
    }
    let titles = title_width(rows, &widths, width);
    let line =
        |out: &mut String, cells: &[String; 8], title: &str, log: &str, real: bool| match titles {
            Some(tw) => {
                let [id, stage, status, time, ahead, findings, pr, place] = widths;
                let widths = [id, tw, stage, status, time, ahead, findings, pr, place];
                push_row(
                    out,
                    &with_title(cells, title, tw),
                    log,
                    real,
                    &widths,
                    width,
                );
            }
            None => push_row(out, cells, log, real, &widths, width),
        };
    let mut out = String::new();
    line(&mut out, &HEADER.map(str::to_owned), TITLE, "LOG", false);
    for (cells, row) in data.iter().zip(rows) {
        let log_is_real = row.log_line.as_deref().is_some_and(|line| !line.is_empty());
        let log = row.log_line.as_deref().unwrap_or(DASH);
        let title = match row.title.trim() {
            "" => DASH,
            title => title,
        };
        line(&mut out, cells, title, log, log_is_real);
    }
    parked_lines(rows, &mut out);
    out
}

/// One line per parked task after the table (issue 671): the command to run next, in backticks
/// so the run hints name the executable, and the checkout it reads. The first checkout also goes
/// to the block that says where to run the frame's commands, with `SBXM_CONFIG_DIR`.
fn parked_lines(rows: &[TaskRow], out: &mut String) {
    let mut named = false;
    for row in rows {
        let Some(parked) = &row.parked else { continue };
        let next = match parked.next.find("sbxm ") {
            Some(at) => format!("{}`{}`", &parked.next[..at], &parked.next[at..]),
            None => parked.next.clone(),
        };
        let place = match parked.checkout.as_deref() {
            Some(dir) => {
                if !named {
                    crate::hint::name_dir(Path::new(dir));
                    named = true;
                }
                format!(" (in {dir})")
            }
            None => String::new(),
        };
        out.push_str(&crate::hint::named(&format!(
            "{} is parked; next: {next}{place}\n",
            row.id
        )));
    }
}
