//! The pure parts of `sbxm task loop` (issue 320, decision 245): which labeled issue is next, what
//! a `Depends on:` line names, whether a failure was a usage limit, the loop's log rows and the
//! read-only report of an empty queue.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::backend::Location;
use crate::github::{Issue, PrMergeState, PrState};
use crate::run::results::format_timestamp;
use crate::task::finish::{check_own_folder, next_fence};
use crate::task::record::{self, Kind, Record};

/// The label that puts an issue in the loop's queue (decision 245(b)); the loop removes it when a
/// task starts and never adds it (decision 4).
pub const READY_LABEL: &str = "ready-for-worker";

/// The label that puts a queued issue ahead of the others (issue 486): the loop takes these
/// first, in the queue's own order among them, and `sbxm status`'s queue shows them first.
pub const PRIORITY_LABEL: &str = "priority:high";

/// The key in a task record's free-form `hooks` (decisions 139, 146) marking a task the loop
/// finished: unless the task continued an open PR, its PR is one the loop opened, which counts
/// toward the loop's cap of PRs awaiting the user (decision 3).
pub const LOOP_HOOK: &str = "task_loop";

/// `<base>/.sbxm/task-loop.lock`: held while a loop runs (`task loop`), and read by `sbxm status`.
pub fn lock_path(base_dir: &Path) -> std::path::PathBuf {
    base_dir.join(".sbxm").join("task-loop.lock")
}

/// What [`lock_path`] holds: the loop's process and, for a loop started with `--issue` (issue
/// 477), its list in the user's order and its `--cap`. A lock without the list (a labeled-queue
/// loop, or one an older binary wrote) reads with an empty list, and an older binary reads this
/// one as its process alone, so either reads the other's lock.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockFile {
    #[serde(flatten)]
    pub process: record::Process,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u32>,
    /// What the loop did with its list so far (issue 493), rewritten after each issue. `None` in
    /// a labeled-queue loop's lock and in one an older binary wrote, which `sbxm status` reads
    /// from the task records instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// The children of a dead loop this loop adopted (issue 643): `sbxm status` lists each as this
    /// loop's while it runs, and a loop that takes this lock over once this one is gone adopts
    /// them in turn (review M-1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adopted: Vec<AdoptedChild>,
    /// When a `task loop --wait` last found no work and began its wait (issue 671): it looks
    /// again a `PAUSE_POLL` later, which `sbxm status` shows. Absent while it works, and in a
    /// lock an older binary wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_check: Option<String>,
    /// The line `sbxm status` shows while the loop's last retro check found a retro due (issue
    /// 714, decision 330(4)): `retro due: 12 PRs since 2026-11-16`. Absent when it isn't due, and
    /// in a lock an older binary wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retro_due: Option<String>,
}

/// A child a loop adopted (issue 643): its issue's task, and the owner its record names, the dead
/// loop that started it. A command that takes the task since records another owner, so the task
/// is no longer the adopted child's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdoptedChild {
    pub issue: u32,
    pub owner: record::Process,
}

impl AdoptedChild {
    /// Whether `rec` is this child's task, still owned by the loop that started it.
    pub fn is(&self, rec: &record::Record) -> bool {
        rec.number == self.issue
            && rec.id == format!("issue-{}", self.issue)
            && rec.owner() == Some(&self.owner)
    }
}

/// Where a loop started with `--issue` is in its list, as the loop itself records it (issue 493):
/// the tasks counted toward `--cap`, the issue it is on and what became of each other issue it
/// got to. An issue with no outcome and not current is still to come.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    pub counted: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u32>,
    #[serde(default)]
    pub outcomes: Vec<IssueOutcome>,
}

impl Progress {
    /// What became of `issue`, if the loop recorded anything.
    pub fn outcome(&self, issue: u32) -> Option<Outcome> {
        self.outcomes
            .iter()
            .find(|o| o.issue == issue)
            .map(|o| o.outcome)
    }

    /// Records `outcome` for `issue`, replacing the one it had. It goes last, so the outcomes are
    /// in the order the loop reached them: the held ones in the order it tries them again
    /// (review M-1).
    pub fn set(&mut self, issue: u32, outcome: Outcome) {
        self.outcomes.retain(|o| o.issue != issue);
        self.outcomes.push(IssueOutcome { issue, outcome });
    }

    /// Makes `issue` the one the loop is on; an outcome it had (held, or skipped while it waited
    /// on a dependency) no longer holds.
    pub fn start(&mut self, issue: u32) {
        self.current = Some(issue);
        self.outcomes.retain(|o| o.issue != issue);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IssueOutcome {
    pub issue: u32,
    pub outcome: Outcome,
}

/// What became of one listed issue (issue 493).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Finished into a PR: counted toward `--cap`.
    Done,
    /// Left for the user, with or without a task record: counted toward `--cap`.
    Parked,
    /// Refused, waiting on a dependency, or taken by a command run by hand: not counted.
    Skipped,
    /// Its task's lock was held by a command run by hand: tried again after the loop's next task,
    /// not counted until then.
    Held,
}

/// Whether this task's PR is one a loop opened: a loop finished it, and it didn't continue an open
/// PR (decision 169), whose finish only pushes to it (review M-3, repeat). A continued PR a loop
/// did open still counts through the record of the task that opened it.
pub fn opened_by_loop(rec: &Record) -> bool {
    rec.continues.is_none() && rec.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true))
}

/// A PR a loop opened, as its task record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPr {
    /// The task's id, `issue-<n>` or a spec task's.
    pub task: String,
    pub url: String,
    /// The PR's number; `None` when the URL doesn't end in one.
    pub number: Option<u32>,
}

/// Every PR a loop opened for `repo` that a task record under `base_dir` names (review M-3), an
/// issue task's or a queued spec job's (issue 524): a task finished by hand, a PR task, another
/// repo's task, or a task folder that is not sbxm's own
/// doesn't count. In the order [`record::task_folders`] lists the tasks.
pub fn loop_prs(base_dir: &Path, repo: &str) -> Result<Vec<LoopPr>> {
    prs_where(base_dir, repo, opened_by_loop)
}

/// Every PR a loop finished a task into for `repo`, as [`loop_prs`] finds them but with the PRs a
/// loop task continued too (issue 530, answer 1(a)): those await the user's merge as well.
pub fn loop_finished_prs(base_dir: &Path, repo: &str) -> Result<Vec<LoopPr>> {
    prs_where(base_dir, repo, |rec| {
        rec.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true))
    })
}

fn prs_where(base_dir: &Path, repo: &str, keep: fn(&Record) -> bool) -> Result<Vec<LoopPr>> {
    let mut found = Vec::new();
    for dir in record::task_folders(base_dir)? {
        if check_own_folder(base_dir, &dir).is_err() {
            continue;
        }
        let Ok(rec) = record::read(&dir.join("task.json")) else {
            continue;
        };
        // A queued spec job's PR counts like an issue's (issue 524).
        if !matches!(rec.kind, Kind::Issue | Kind::Spec) || rec.repo != repo || !keep(&rec) {
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

/// The fix tasks of PR `number` (resolves and log-fixes, `fix-<n>-<head>`) of `repo` under
/// `base_dir`, in task folders that are sbxm's own; a record that can't be read is left out.
pub fn fixes_of(base_dir: &Path, repo: &str, number: u32) -> Vec<Record> {
    let prefix = format!("fix-{number}-");
    record::task_folders(base_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|id| id.starts_with(&prefix))
                && check_own_folder(base_dir, dir).is_ok()
        })
        .filter_map(|dir| record::read(&dir.join("task.json")).ok())
        .filter(|rec| rec.kind == Kind::Fix && rec.repo == repo)
        .collect()
}

/// The highest risk of PR `number`'s review history after the path rules (decision 298(3)): the
/// reviews of `own` (the task or tasks that opened it) and of every finished fix pushed onto it
/// ([`fixes_of`]), ranked by [`awaiting_rank`]; `None` with no review. A later narrow review
/// doesn't clear the risk an earlier one left, and the order the records are read in doesn't
/// matter. The one rule the loop arms by and `sbxm status` shows (issue 671, review M-2).
pub fn history_risk<'a>(
    base_dir: &Path,
    repo: &str,
    number: u32,
    own: impl IntoIterator<Item = &'a Record>,
) -> Option<crate::task::risk::Level> {
    let risk = |rec: &Record| rec.review.as_ref().map(|r| r.risk);
    fixes_of(base_dir, repo, number)
        .iter()
        .filter(|fix| fix.stage == record::Stage::Finished)
        .filter_map(risk)
        .chain(own.into_iter().filter_map(risk))
        .max_by_key(|&risk| awaiting_rank(risk))
}

/// How far `risk` keeps a PR from the loop's arming, for the highest of its review history: an
/// unknown risk fails the loop's arm checks as medium does, so it outranks low (where
/// [`Level`](crate::task::risk::Level)'s own order puts it below).
pub fn awaiting_rank(risk: crate::task::risk::Level) -> u8 {
    use crate::task::risk::Level;
    match risk {
        Level::Low => 0,
        Level::Unknown => 1,
        Level::Medium => 2,
        Level::High => 3,
    }
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

/// Whether an issue carries [`PRIORITY_LABEL`].
fn is_priority(labels: &[String]) -> bool {
    labels.iter().any(|l| l == PRIORITY_LABEL)
}

/// The queue (decision 2, 245(b)): the open issues labeled [`READY_LABEL`], those labeled
/// [`PRIORITY_LABEL`] first (issue 486), then within each group size S, then M, then L, then
/// unsized, and within a size the oldest first. Issue numbers grow with creation, so the oldest is
/// the lowest number.
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
    queue.sort_by_key(|i| (!is_priority(&i.labels), rank(i), i.number));
    queue.into_iter().map(|i| i.number).collect()
}

/// The queue as a free cloud slot takes it with `[resources] placement = "cloud-first"` (issue
/// 648): the issues of [`queue_order`], those labeled [`PRIORITY_LABEL`] still first (issue 486),
/// then within each group size L, then M, then S, then unsized, and within a size the oldest first.
pub fn largest_first(open: &[Issue]) -> Vec<u32> {
    let rank = |number: &u32| {
        let labels = open
            .iter()
            .find(|i| i.number == *number)
            .map(|i| i.labels.as_slice())
            .unwrap_or_default();
        let size = match size(labels) {
            Some("L") => 0,
            Some("M") => 1,
            Some("S") => 2,
            _ => 3,
        };
        (!is_priority(labels), size)
    };
    let mut queue = queue_order(open);
    queue.sort_by_key(|n| (rank(n), *n));
    queue
}

/// Every `#<n>` in an issue body's `**Depends on:**` field, over all its lines, in order.
pub fn depends_on(body: &str) -> Vec<u32> {
    super::select::field_numbers(body, "Depends on")
}

/// Whether an issue's body still has a `## Open points` heading (issue 511): a readied issue
/// renames it `## Answered`. Only a level-2 heading at a line's start counts, outside a fenced
/// code block, so a body that quotes the heading in prose or in an example is not held back.
pub fn has_open_points(body: &str) -> bool {
    // The fence that is open, by character and length (review M-1): a shorter fence or one of
    // the other character inside it is the example's content, not its end.
    let mut open = None;
    for line in body.lines() {
        let before = open;
        open = next_fence(open, line);
        if before.is_some() || open.is_some() {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ")
            && heading.trim().eq_ignore_ascii_case("open points")
        {
            return true;
        }
    }
    false
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
    /// Where its task ran (issue 615, decision 317(6)); `None` when unknown, shown as `-`.
    pub slot: Option<Location>,
    /// Seconds since the epoch.
    pub started: u64,
    pub ready: Option<u64>,
    pub worker_s: Option<u64>,
    pub reviewer_s: Option<u64>,
    /// The record's review round; shown only when `must_fix` is empty, since a gate failure can
    /// use a round before the first review and a review whose count can't be read sets none.
    pub rounds: Option<u32>,
    /// Each saved review's must-fix count, in order (issue 697): `None` for a round whose count
    /// can't be read, shown as `?`; their number is the cell's round count, as in `task facts`.
    pub must_fix: Vec<Option<u32>>,
    pub outcome: String,
    /// Each stop's cause and how long it held the task, in seconds.
    pub stops: Vec<(String, u64)>,
    /// From the previous task's ready (or its end, if it never got ready) to this one's start;
    /// `None` for the loop's first task.
    pub idle_before: Option<u64>,
}

const TITLE: &str = "# Loop log (decision 245(h))\n\n";

/// The line naming the loop's repository, `owner/name` after it (issue 602).
pub const REPOSITORY: &str = "Repository: ";

const HEADER: &str =
    "Times UTC. Written by `sbxm task loop`, one row per task; `unknown` where not measured.

| # | Issue | Size | Slot | Started | Ready (time to ready) | Worker / reviewer s | Rounds \
(must-fix) | Outcome | Stops (cause, duration) | Idle before |
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
    render_as(number, &format!("#{} {}", row.issue, cell(&row.title)), row)
}

/// [`render`] with `item` in the Issue column.
fn render_as(number: usize, item: &str, row: &Row) -> String {
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
    // The saved reviews decide the cell when there are any (review M-1 of issue 697): a review
    // whose count can't be read is saved, but the record gets no review from it.
    let rounds = if row.must_fix.is_empty() {
        row.rounds.map_or_else(|| "-".to_owned(), |r| r.to_string())
    } else {
        let counts: Vec<String> = row
            .must_fix
            .iter()
            .map(|c| c.map_or_else(|| "?".to_owned(), |c| c.to_string()))
            .collect();
        format!("{} ({})", counts.len(), counts.join(", "))
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
        "| {number} | {item} | {} | {} | {} | {ready} | {durations} | {rounds} | {} | {} | {} |\n",
        row.size.as_deref().unwrap_or("unknown"),
        row.slot.map_or("-", Location::as_str),
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

/// Whether the log `text` has a row for issue `issue`.
pub fn has_row(text: &str, issue: u32) -> bool {
    rows_in(text, issue).1
}

/// Appends `row` to the log at `path` (decision 245(h)), with the header naming `repo` first when
/// the file is new. Returns `false`, writing nothing, when the issue already has a row there: a
/// loop run again after being killed doesn't log a task twice.
pub fn append_row(path: &Path, repo: &str, row: &Row) -> Result<bool> {
    add_row(path, repo, row, true)
}

/// Appends a resolve's row (decision 298), even when its PR already has one: each resolve of a PR
/// gets its own.
pub fn append_resolve_row(path: &Path, repo: &str, row: &Row) -> Result<()> {
    add_row(path, repo, row, false).map(|_| ())
}

/// Appends a queued job's row (issue 525), its Issue column naming the job (`job review #465`)
/// instead of an issue; `row.issue` is unused. Each job gets its own row.
pub fn append_job_row(path: &Path, repo: &str, job: &str, row: &Row) -> Result<()> {
    add_any(path, repo, Some(&cell(job)), row, false).map(|_| ())
}

/// Appends a note of the loop's own (issue 714: a retro check that failed) as a row of the
/// table whose `#` cell is `-` and whose Issue cell is `note`, the other cells empty: the table
/// stays whole, and it is no task row, so it is never numbered, counted, or a reason to post the
/// log.
pub fn append_note(path: &Path, repo: &str, note: &str) -> Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut add = start_of_log(&text, repo);
    add.push_str(&format!("| - | {} |{}\n", cell(note), " |".repeat(9)));
    write_log(path, &add)
}

/// What goes before a new row of a log whose text so far is `text`: the title, repository and
/// header for a new log, a line end for one that lacks it.
fn start_of_log(text: &str, repo: &str) -> String {
    let mut add = String::new();
    if text.is_empty() {
        add.push_str(TITLE);
        add.push_str(&format!("{REPOSITORY}{}\n\n", cell(repo)));
        add.push_str(HEADER);
    } else if !text.ends_with('\n') {
        add.push('\n');
    }
    add
}

/// Appends `add` to the log at `path`, making its folder first.
fn write_log(path: &Path, add: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(add.as_bytes()))
        .with_context(|| format!("cannot write {}", path.display()))
}

fn add_row(path: &Path, repo: &str, row: &Row, once: bool) -> Result<bool> {
    add_any(path, repo, None, row, once)
}

fn add_any(path: &Path, repo: &str, item: Option<&str>, row: &Row, once: bool) -> Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let (count, logged) = rows_in(&text, row.issue);
    if once && logged {
        return Ok(false);
    }
    let mut add = start_of_log(&text, repo);
    add.push_str(&match item {
        Some(item) => render_as(count + 1, item, row),
        None => render(count + 1, row),
    });
    write_log(path, &add)?;
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
