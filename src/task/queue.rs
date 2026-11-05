//! The pure parts of `sbxm task loop` (issue 320, decision 245): which labeled issue is next, what
//! a `Depends on:` line names, whether a failure was a usage limit, the loop's log rows and the
//! read-only report of an empty queue.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::github::{Issue, PrMergeState, PrState};
use crate::run::results::format_timestamp;
use crate::task::finish::check_own_folder;
use crate::task::record::{self, Kind, Record};

/// The label that puts an issue in the loop's queue (decision 245(b)); the loop removes it when a
/// task starts and never adds it (decision 4).
pub const READY_LABEL: &str = "ready-for-worker";

/// The key in a task record's free-form `hooks` (decisions 139, 146) marking a task the loop
/// finished: unless the task continued an open PR, its PR is one the loop opened, which counts
/// toward the loop's cap of PRs awaiting the user (decision 3).
pub const LOOP_HOOK: &str = "task_loop";

/// Whether this task's PR is one a loop opened: a loop finished it, and it didn't continue an open
/// PR (decision 169), whose finish only pushes to it (review M-3, repeat). A continued PR a loop
/// did open still counts through the record of the task that opened it.
pub fn opened_by_loop(rec: &Record) -> bool {
    rec.continues.is_none() && rec.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true))
}

/// A PR a loop opened, as its task record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPr {
    /// The task's id, `issue-<n>`.
    pub task: String,
    pub url: String,
    /// The PR's number; `None` when the URL doesn't end in one.
    pub number: Option<u32>,
}

/// Every PR a loop opened for `repo` that a task record under `base_dir` names (review M-3): a
/// task finished by hand, a PR task, another repo's task, or a task folder that is not sbxm's own
/// doesn't count. In the order [`record::task_folders`] lists the tasks.
pub fn loop_prs(base_dir: &Path, repo: &str) -> Result<Vec<LoopPr>> {
    let mut found = Vec::new();
    for dir in record::task_folders(base_dir)? {
        if check_own_folder(base_dir, &dir).is_err() {
            continue;
        }
        let Ok(rec) = record::read(&dir.join("task.json")) else {
            continue;
        };
        if rec.kind != Kind::Issue || rec.repo != repo || !opened_by_loop(&rec) {
            continue;
        }
        let Some(url) = rec.pr.clone() else {
            continue;
        };
        found.push(LoopPr {
            task: rec.id.clone(),
            number: pr_number(&url),
            url,
        });
    }
    Ok(found)
}

/// Whether a PR awaits the user (decision 3, 293): it is open and has no auto-merge armed. The
/// one rule behind the loop's cap and the Loop section of `sbxm status`.
pub fn awaits_user(state: PrMergeState) -> bool {
    state.state == PrState::Open && !state.auto_merge
}

/// An issue's size from its `size:S`, `size:M` or `size:L` label.
pub fn size(labels: &[String]) -> Option<&'static str> {
    labels.iter().find_map(|label| match label.as_str() {
        "size:S" => Some("S"),
        "size:M" => Some("M"),
        "size:L" => Some("L"),
        _ => None,
    })
}

/// The queue (decision 2, 245(b)): the open issues labeled [`READY_LABEL`], size S, then M, then
/// L, then unsized, and within a size the oldest first. Issue numbers grow with creation, so the
/// oldest is the lowest number.
pub fn queue_order(open: &[Issue]) -> Vec<u32> {
    let rank = |issue: &Issue| match size(&issue.labels) {
        Some("S") => 0,
        Some("M") => 1,
        Some("L") => 2,
        _ => 3,
    };
    let mut queue: Vec<&Issue> = open
        .iter()
        .filter(|i| i.labels.iter().any(|l| l == READY_LABEL))
        .collect();
    queue.sort_by_key(|i| (rank(i), i.number));
    queue.into_iter().map(|i| i.number).collect()
}

/// Every `#<n>` on an issue body's `**Depends on:**` line, in order.
pub fn depends_on(body: &str) -> Vec<u32> {
    super::select::field_numbers(body, "Depends on")
}

/// Whether a failure's text is a harness's usage limit (decision 6): Claude Code's "usage limit
/// reached" and Codex's "hit your usage limit" both carry these two words.
pub fn is_usage_limit(text: &str) -> bool {
    text.to_lowercase().contains("usage limit")
}

/// Whether a failed phase stopped on a usage limit: said in its error, or in a line of its output
/// that reports a failure (`issue-3: worker claude failed: …`). The rest of the output is left
/// out: an issue's own words about a usage limit, printed on the way, are not one.
pub fn failure_says_usage_limit(error: &str, output: &str) -> bool {
    is_usage_limit(error)
        || output
            .lines()
            .any(|line| line.to_lowercase().contains("failed") && is_usage_limit(line))
}

/// The number of a PR from its URL (`…/pull/<n>`), as `task finish` records it.
pub fn pr_number(url: &str) -> Option<u32> {
    let (_, tail) = url.rsplit_once("/pull/")?;
    tail.trim_end_matches('/').parse().ok()
}

/// `59s`, `9m15s`, `1h02m05s`: the log's durations.
pub fn span(secs: u64) -> String {
    let (h, m, s) = (secs / 3_600, secs % 3_600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// One task of the loop, as its log row shows it (decision 245(h)); `None` where nothing was
/// measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub issue: u32,
    pub title: String,
    pub size: Option<String>,
    /// Seconds since the epoch.
    pub started: u64,
    pub ready: Option<u64>,
    pub worker_s: Option<u64>,
    pub reviewer_s: Option<u64>,
    pub rounds: Option<u32>,
    pub must_fix: Option<u32>,
    pub outcome: String,
    /// Each stop's cause and how long it held the task, in seconds.
    pub stops: Vec<(String, u64)>,
    /// From the previous task's ready (or its end, if it never got ready) to this one's start;
    /// `None` for the loop's first task.
    pub idle_before: Option<u64>,
}

const HEADER: &str = "# Loop log (decision 245(h))

Times UTC. Written by `sbxm task loop`, one row per task; `unknown` where not measured.

| # | Issue | Size | Started | Ready (time to ready) | Worker / reviewer s | Rounds (must-fix) | \
Outcome | Stops (cause, duration) | Idle before | User interventions |
|---|---|---|---|---|---|---|---|---|---|---|
";

/// A table cell: one line, with `|` escaped.
fn cell(text: &str) -> String {
    text.replace(['\r', '\n'], " ").replace('|', "\\|")
}

/// `14:59:18` of a timestamp.
fn clock(secs: u64) -> String {
    let stamp = format_timestamp(secs);
    stamp
        .split_once('T')
        .map_or(stamp.clone(), |(_, t)| t.trim_end_matches('Z').to_owned())
}

fn render(number: usize, row: &Row) -> String {
    let opt = |v: Option<u64>| v.map_or_else(|| "-".to_owned(), |v| v.to_string());
    let ready = row.ready.map_or_else(
        || "-".to_owned(),
        |ready| {
            format!(
                "{} ({})",
                clock(ready),
                span(ready.saturating_sub(row.started))
            )
        },
    );
    let durations = match (row.worker_s, row.reviewer_s) {
        (None, None) => "-".to_owned(),
        (w, r) => format!("{} / {}", opt(w), opt(r)),
    };
    let rounds = match (row.rounds, row.must_fix) {
        (Some(rounds), Some(must_fix)) => format!("{rounds} ({must_fix})"),
        (Some(rounds), None) => rounds.to_string(),
        _ => "-".to_owned(),
    };
    let stops = if row.stops.is_empty() {
        "none".to_owned()
    } else {
        row.stops
            .iter()
            .map(|(cause, secs)| format!("{cause}, {}", span(*secs)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!(
        "| {number} | #{} {} | {} | {} | {ready} | {durations} | {rounds} | {} | {} | {} | unknown |\n",
        row.issue,
        cell(&row.title),
        row.size.as_deref().unwrap_or("unknown"),
        clock(row.started),
        cell(&row.outcome),
        cell(&stops),
        row.idle_before.map_or_else(|| "-".to_owned(), span),
    )
}

/// The number of the rows already in `text`, and whether one is `issue`'s.
fn rows_in(text: &str, issue: u32) -> (usize, bool) {
    let prefix = format!("#{issue} ");
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| {
            line.split('|')
                .nth(1)
                .is_some_and(|n| n.trim().parse::<usize>().is_ok())
        })
        .collect();
    let logged = rows.iter().any(|line| {
        line.split('|')
            .nth(2)
            .is_some_and(|c| c.trim_start().starts_with(&prefix))
    });
    (rows.len(), logged)
}

/// Appends `row` to the log at `path` (decision 245(h)), with the header first when the file is
/// new. Returns `false`, writing nothing, when the issue already has a row there: a loop run
/// again after being killed doesn't log a task twice.
pub fn append_row(path: &Path, row: &Row) -> Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let (count, logged) = rows_in(&text, row.issue);
    if logged {
        return Ok(false);
    }
    let mut add = String::new();
    if text.is_empty() {
        add.push_str(HEADER);
    } else if !text.ends_with('\n') {
        add.push('\n');
    }
    add.push_str(&render(count + 1, row));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(add.as_bytes()))
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(true)
}

/// A labeled issue the loop passed over, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub issue: u32,
    pub reason: String,
}

/// What the empty queue's report is made from (decision 245(c)): read-only, from what the loop
/// already read.
pub struct Report<'a> {
    pub open: &'a [Issue],
    pub skipped: &'a [Skipped],
}

/// The questions an issue's body asks: its lines that end with a question mark, list markers
/// dropped, at most five.
fn questions(body: &str) -> Vec<String> {
    body.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '+'])
                .trim_start_matches("[ ]")
                .trim()
        })
        .filter(|line| line.ends_with('?'))
        .take(5)
        .map(str::to_owned)
        .collect()
}

/// The report of an empty queue (decision 245(c)): the labeled issues the loop passed over and
/// why, then the almost-ready ones (sized, not labeled) with the questions blocking each.
pub fn report(report: &Report) -> String {
    let title = |number: u32| {
        report
            .open
            .iter()
            .find(|i| i.number == number)
            .map_or_else(String::new, |i| format!(" {}", i.title))
    };
    let mut text = String::new();
    if !report.skipped.is_empty() {
        text.push_str("labeled ready-for-worker but passed over:\n");
        for skip in report.skipped {
            text.push_str(&format!(
                "  #{}{}: {}\n",
                skip.issue,
                title(skip.issue),
                skip.reason
            ));
        }
    }
    let almost: Vec<&Issue> = report
        .open
        .iter()
        .filter(|i| !i.labels.iter().any(|l| l == READY_LABEL))
        .filter(|i| size(&i.labels).is_some())
        .collect();
    if almost.is_empty() {
        text.push_str("no almost-ready issues (sized, not labeled ready-for-worker)\n");
        return text;
    }
    text.push_str("almost ready (sized, not labeled ready-for-worker), and what blocks each:\n");
    for issue in almost {
        let size = size(&issue.labels).unwrap_or("?");
        text.push_str(&format!(
            "  #{} {} (size:{size})\n",
            issue.number, issue.title
        ));
        let mut blockers = Vec::new();
        if issue.labels.iter().any(|l| l == "question") {
            blockers.push("labeled question".to_owned());
        }
        blockers.extend(questions(&issue.body));
        if blockers.is_empty() {
            blockers.push(
                "no open question found in its body; label it ready-for-worker once its open \
                 points are answered"
                    .to_owned(),
            );
        }
        for blocker in blockers {
            text.push_str(&format!("    - {blocker}\n"));
        }
    }
    text
}
