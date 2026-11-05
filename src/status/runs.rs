//! The Comparison runs section of `sbxm status` (decision 171(b), issue 91): one row per folder
//! in `<base>/.sbxm/runs/`, read from `run.json`, the pairs' `result.json` files and `run.log`.
//! The only `sbx` call is one `ls` for the whole section, to flag a pair whose sandbox is gone
//! before it finished; if that fails the flag says "unknown" and the rest still shows. A folder
//! that can't be read as a run is a row saying so, never an error.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::backend::SandboxBackend;
use crate::run::results::parse_timestamp;
use crate::status::tasks::{display_width, format_duration, last_log_line, pad, truncate};

/// A missing value.
const DASH: &str = "-";

/// More repeats than this in a `run.json` is a damaged file, not a run (the pairs would be
/// counted one by one).
const MAX_REPEAT: u64 = 10_000;

/// Whether a run's unfinished pairs still have their sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gone {
    /// Nothing to flag: the run is completed, or no unfinished pair of the running wave lacks
    /// its sandbox.
    No,
    /// This many pairs of the running wave have no sandbox in `sbx ls`.
    Pairs(u32),
    /// `sbx ls` failed, so it can't be told.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    /// `harness:model` of each contestant, in order.
    pub contestants: Vec<String>,
    pub done: u64,
    pub total: u64,
    pub started_at: Option<u64>,
    /// As `run.json` has it (`2026-09-30T14:13:20Z`); `None` until the run is marked completed.
    pub completed_at: Option<String>,
    pub log_line: Option<String>,
    pub gone: Gone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Run(RunInfo),
    /// Why the folder isn't a readable run (no `run.json`, not JSON, no contestants...).
    Malformed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: String,
    pub state: RunState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runs {
    Rows(Vec<RunRow>),
    /// `<base>/.sbxm/runs/` exists but can't be listed.
    Unreadable(String),
}

/// Reads every run, asking `backend` for its sandboxes at most once (and not at all when no run
/// has an unfinished pair to check).
pub fn read(base: &Path, backend: &dyn SandboxBackend) -> Runs {
    read_with(base, &|| {
        backend
            .list()
            .ok()
            .map(|all| all.into_iter().map(|s| s.name).collect())
    })
}

/// [`read`] with the sandbox names behind `live`: `None` when the list couldn't be read.
pub fn read_with(base: &Path, live: &dyn Fn() -> Option<HashSet<String>>) -> Runs {
    let dir = base.join(".sbxm").join("runs");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Runs::Rows(Vec::new()),
        Err(err) => return Runs::Unreadable(err.to_string()),
    };
    let mut folders: Vec<(String, std::path::PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            path.is_dir()
                .then(|| (entry.file_name().to_string_lossy().into_owned(), path))
        })
        .collect();
    folders.sort();
    let mut names: Option<Option<HashSet<String>>> = None;
    let rows = folders
        .into_iter()
        .map(|(id, path)| {
            let state = match read_run(&id, &path) {
                Err(why) => RunState::Malformed(why),
                Ok((mut info, wave)) => {
                    if info.completed_at.is_none() && !wave.is_empty() {
                        let known = names.get_or_insert_with(live);
                        info.gone = match known {
                            None => Gone::Unknown,
                            Some(set) => match wave.iter().filter(|n| !set.contains(*n)).count() {
                                0 => Gone::No,
                                n => Gone::Pairs(n as u32),
                            },
                        };
                    }
                    RunState::Run(info)
                }
            };
            RunRow { id, state }
        })
        .collect();
    Runs::Rows(rows)
}

/// One run folder: its info, and the sandbox names of the unsaved pairs in the lowest repeat
/// that still has any (repeats run as sequential waves, so later ones haven't started).
fn read_run(id: &str, dir: &Path) -> Result<(RunInfo, Vec<String>), String> {
    let text = fs::read_to_string(dir.join("run.json")).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "no run.json".to_owned(),
        _ => format!("cannot read run.json: {e}"),
    })?;
    let json: Value =
        serde_json::from_str(&text).map_err(|_| "run.json is not valid JSON".to_owned())?;
    let contestants: Vec<String> = json
        .get("contestants")
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty())
        .ok_or("run.json has no contestants")?
        .iter()
        .map(|c| {
            let field = |key| c.get(key).and_then(Value::as_str).unwrap_or("?");
            format!("{}:{}", field("harness"), field("model"))
        })
        .collect();
    let repeat = json
        .get("repeat")
        .and_then(Value::as_u64)
        .filter(|r| (1..=MAX_REPEAT).contains(r))
        .ok_or("run.json has no valid repeat")?;
    let total = contestants.len() as u64 * repeat;
    let saved = |i: usize, r: u64| {
        dir.join(i.to_string())
            .join(r.to_string())
            .join("result.json")
            .is_file()
    };
    let mut done = 0;
    let mut wave = Vec::new();
    for r in 0..repeat {
        let missing: Vec<usize> = (0..contestants.len()).filter(|&i| !saved(i, r)).collect();
        done += (contestants.len() - missing.len()) as u64;
        if wave.is_empty() {
            wave = missing
                .into_iter()
                .map(|i| format!("sbxm-run-{id}-{i}-{r}"))
                .collect();
        }
    }
    let info = RunInfo {
        contestants,
        done,
        total,
        started_at: json
            .get("started_at")
            .and_then(Value::as_str)
            .and_then(parse_timestamp),
        completed_at: json
            .get("completed_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        log_line: last_log_line(&dir.join("run.log")),
        gone: Gone::No,
    };
    Ok((info, wave))
}

const HEADER: [&str; 7] = [
    "RUN",
    "CONTESTANTS",
    "PAIRS",
    "TIME",
    "COMPLETED",
    "FLAG",
    "LOG",
];

fn cells(row: &RunRow, now: u64) -> [String; 7] {
    let id = row.id.clone();
    let dash = || DASH.to_owned();
    match &row.state {
        RunState::Malformed(why) => [
            id,
            dash(),
            dash(),
            dash(),
            dash(),
            format!("malformed: {why}"),
            dash(),
        ],
        RunState::Run(info) => {
            let end = info
                .completed_at
                .as_deref()
                .and_then(parse_timestamp)
                .unwrap_or(now);
            [
                id,
                info.contestants.join(", "),
                format!("{}/{}", info.done, info.total),
                info.started_at
                    .map_or_else(dash, |s| format_duration(end.saturating_sub(s))),
                info.completed_at.clone().unwrap_or_else(dash),
                match info.gone {
                    Gone::No => String::new(),
                    Gone::Pairs(1) => "sandbox gone".to_owned(),
                    Gone::Pairs(n) => format!("sandbox gone ({n} pairs)"),
                    Gone::Unknown => "sandbox unknown".to_owned(),
                },
                info.log_line.clone().unwrap_or_else(dash),
            ]
        }
    }
}

/// The section, after a blank line: a table of runs, one line saying there are none, or one line
/// saying the folder can't be read. Each line is cut to `width` on a terminal.
pub fn render(runs: &Runs, now: u64, width: Option<usize>) -> String {
    let lines: Vec<String> = match runs {
        Runs::Unreadable(error) => vec![format!("Comparison runs couldn't be read: {error}")],
        Runs::Rows(rows) if rows.is_empty() => vec!["No comparison runs.".to_owned()],
        Runs::Rows(rows) => {
            let data: Vec<[String; 7]> = rows.iter().map(|r| cells(r, now)).collect();
            let mut widths = HEADER.map(display_width);
            for row in &data {
                for (w, cell) in widths.iter_mut().zip(row) {
                    *w = (*w).max(display_width(cell));
                }
            }
            std::iter::once(HEADER.map(str::to_owned))
                .chain(data)
                .map(|cells| {
                    let mut line = String::new();
                    for (cell, w) in cells[..6].iter().zip(&widths) {
                        line.push_str(&pad(cell, *w));
                        line.push_str("  ");
                    }
                    (line + &cells[6]).trim_end().to_owned()
                })
                .collect()
        }
    };
    let mut out = String::from("\n");
    for line in lines {
        match width {
            Some(width) => out.push_str(&truncate(&line, width)),
            None => out.push_str(&line),
        }
        out.push('\n');
    }
    out
}
