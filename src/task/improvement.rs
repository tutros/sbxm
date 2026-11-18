//! The improvement review (decision 330(2) and (3), issue 715): it is due once [`THRESHOLD`] PRs
//! have merged since the date of the newest `sdlc/reviews/<date>-improvement-<slug>.md` on the
//! default branch, "since" being from the day after that date, in UTC, as for the retro alert
//! ([`super::retro`]); no such file is due too. Each review covers one area of
//! `sdlc/reviews/areas.md`, the one whose newest review is oldest (one never reviewed first, in
//! table order). The loop queues it as a spec job whose spec is [`spec`], the embedded
//! `prompts/improvement-review.md` with the area, the date and the run data ([`run_data`]) filled
//! in. Everything here is read through [`GitHubBackend`] or given; the loop acts on it.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

use super::jobs::{self, Job, Running};
use super::record::{self, Kind};
use super::retro::day_of;
use super::{prompts, queue};
use crate::github::{GitHubBackend, PrState};
use crate::run::results::{format_timestamp, parse_day, parse_timestamp};

/// The folder of the reviews, and of `areas.md`, on the default branch.
pub const DIR: &str = "sdlc/reviews";
/// The list of areas, in [`DIR`].
pub const AREAS: &str = "areas.md";
/// How many merged PRs make a review due.
pub const THRESHOLD: usize = 20;
/// The most run data a spec embeds, in bytes; the newest rows are kept.
pub const CAP: usize = 30 * 1024;
/// The start of the name of every spec file the loop writes: `improvement-<slug>-<date>.md`.
pub const SPEC_PREFIX: &str = "improvement-";
/// The start of a finding's heading in a review file, which the merge toast counts.
pub const FINDING: &str = "### IMP-";

/// The start of the task id of a spec job the loop queued (`spec-improvement-<slug>-<date>-…`).
const TASK_PREFIX: &str = "spec-improvement-";
const MARK: &str = "-improvement-";
const DAY: u64 = 24 * 60 * 60;
const TEMPLATE: &str = include_str!("../../prompts/improvement-review.md");
const TEMPLATE_NAME: &str = "improvement-review.md";
const KNOWN: [&str; 7] = [
    "slug", "date", "paths", "output", "since", "run_data", "left_out",
];

/// One row of `areas.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    pub slug: String,
    /// The Paths cell as written.
    pub paths: String,
}

/// The review files on the default branch: its name and the names in [`DIR`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub branch: String,
    pub names: Vec<String>,
}

/// Why a review is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    /// The date of the newest review, any area's; `None` when there is none.
    pub since: Option<String>,
    /// The PRs merged since then; `None` when there is no review.
    pub prs: Option<usize>,
}

/// The date and slug of a review file named `<YYYY-MM-DD>-improvement-<slug>.md`.
pub fn review_name(name: &str) -> Option<(&str, &str)> {
    let date = name.get(..10)?;
    parse_day(date)?;
    let slug = name[10..].strip_prefix(MARK)?.strip_suffix(".md")?;
    (!slug.is_empty()).then_some((date, slug))
}

/// The date of the newest review in `names`, whatever its slug.
pub fn newest(names: &[String]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| review_name(n))
        .map(|(date, _)| date.to_owned())
        .max()
}

/// The areas of `areas.md`'s `text`, in table order, or why it can't be used: no row, a row
/// with no paths, or a slug that isn't `[a-z0-9-]`.
pub fn areas(text: &str) -> std::result::Result<Vec<Area>, String> {
    let mut found = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| l.starts_with('|')) {
        let cells = cells(line);
        let Some(first) = cells.first() else {
            continue;
        };
        let separator = cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
        let slug = first.trim_matches('`').trim();
        if separator || slug.eq_ignore_ascii_case("slug") {
            continue;
        }
        let plain = !slug.is_empty()
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !plain {
            return Err(format!("has the slug {slug:?}, which isn't [a-z0-9-]"));
        }
        let paths = cells.get(1).copied().unwrap_or_default();
        if paths.is_empty() {
            return Err(format!("has no paths for the area {slug}"));
        }
        found.push(Area {
            slug: slug.to_owned(),
            paths: paths.to_owned(),
        });
    }
    if found.is_empty() {
        return Err("has no area".to_owned());
    }
    Ok(found)
}

/// The cells of a table row, trimmed, split on every `|` not escaped as `\|`.
fn cells(line: &str) -> Vec<&str> {
    let inner = line.strip_prefix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, ch) in inner.char_indices() {
        match ch {
            '\\' => escaped = !escaped,
            '|' if !escaped => {
                cells.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => escaped = false,
        }
    }
    let rest = inner[start..].trim();
    if !rest.is_empty() {
        cells.push(rest);
    }
    cells
}

/// The area reviewed next: the one whose newest review in `names` is oldest, one never reviewed
/// first; ties go to table order. A review of a slug not in `areas` counts for no area.
pub fn pick<'a>(areas: &'a [Area], names: &[String]) -> Option<&'a Area> {
    areas.iter().min_by_key(|area| {
        names
            .iter()
            .filter_map(|n| review_name(n))
            .filter(|(_, slug)| *slug == area.slug)
            .map(|(date, _)| date)
            .max()
    })
}

/// The review files on `repo`'s default branch.
pub fn listing(github: &dyn GitHubBackend, repo: &str) -> Result<Listing> {
    let branch = github.default_branch(repo)?;
    let names = github.dir_files(repo, &branch, DIR)?;
    Ok(Listing { branch, names })
}

/// Whether a review is due: `None` when it isn't.
pub fn due(github: &dyn GitHubBackend, repo: &str, listing: &Listing) -> Result<Option<Due>> {
    let Some(since) = newest(&listing.names) else {
        return Ok(Some(Due {
            since: None,
            prs: None,
        }));
    };
    let after = parse_day(&since).ok_or_else(|| anyhow!("{since} isn't a day"))? + DAY;
    let prs = github
        .prs_merged_since(repo, &since)?
        .iter()
        .filter(|pr| pr.merged_at >= after)
        .count();
    Ok((prs >= THRESHOLD).then_some(Due {
        since: Some(since),
        prs: Some(prs),
    }))
}

/// The area reviewed next, read from `areas.md` on the default branch: `Err` when the read
/// fails, `Ok(Err(why))` when `areas.md` is missing or can't be used.
pub fn next_area(
    github: &dyn GitHubBackend,
    repo: &str,
    listing: &Listing,
) -> Result<std::result::Result<Area, String>> {
    if !listing.names.iter().any(|n| n == AREAS) {
        return Ok(Err("is missing".to_owned()));
    }
    let text = github.file_text(repo, &listing.branch, &format!("{DIR}/{AREAS}"))?;
    Ok(areas(&text).map(|areas| {
        pick(&areas, &listing.names)
            .cloned()
            .expect("areas() returns at least one area")
    }))
}

/// Whether a spec job of `file` waits in the queue or runs. An error when the queue can't be
/// read, so a caller never takes "can't tell" for "none".
pub fn queued_spec(base_dir: &Path, file: &str) -> Result<bool> {
    for path in queue_files(base_dir)? {
        if spec_job(&path)?.is_some_and(|queued| queued == file) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The job files waiting in the queue or running.
fn queue_files(base_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = jobs::pending(base_dir)?;
    files.extend(jobs::left_running(base_dir)?);
    Ok(files)
}

/// The file of the spec job at `path`; `None` for any other job, a file that isn't a job (the
/// loop refuses it), or one gone meanwhile (the loop took it). Any other read is an error.
fn spec_job(path: &Path) -> Result<Option<String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    let Some(value) = Running::read(&text).read.job else {
        return Ok(None);
    };
    Ok(match jobs::parse(&value.to_string()) {
        Ok(Job::Spec { file }) => Some(file),
        _ => None,
    })
}

/// Whether an improvement job of `repo` is in flight under `base_dir`: a spec job of an
/// [`SPEC_PREFIX`] file queued or running, or a live spec task of one whose PR GitHub says is
/// open. A job file, or an improvement task's record, that can't be read is an error, not
/// "none": the check is tried again later rather than queue a second job. A task folder with no
/// record holds no task.
pub fn in_flight(base_dir: &Path, repo: &str, github: &dyn GitHubBackend) -> Result<bool> {
    for path in queue_files(base_dir)? {
        if spec_job(&path)?.is_some_and(|file| file.starts_with(SPEC_PREFIX)) {
            return Ok(true);
        }
    }
    for dir in record::task_folders(base_dir)? {
        // A task's folder is named by its id: only an improvement task's own record matters.
        let ours = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(TASK_PREFIX));
        let path = dir.join("task.json");
        if !ours || fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let rec = record::read(&path)?;
        if rec.kind != Kind::Spec || rec.repo != repo || !rec.id.starts_with(TASK_PREFIX) {
            continue;
        }
        let Some(pr) = rec.pr.as_deref().and_then(queue::pr_number) else {
            continue;
        };
        if github.pr(repo, pr)?.state == PrState::Open {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The spec file's name: `improvement-<slug>-<date>.md`.
pub fn spec_name(slug: &str, date: &str) -> String {
    format!("{SPEC_PREFIX}{slug}-{date}.md")
}

/// The review file the spec asks for: `sdlc/reviews/<date>-improvement-<slug>.md`.
pub fn output(slug: &str, date: &str) -> String {
    format!("{DIR}/{date}{MARK}{slug}.md")
}

/// The day of `now`, `YYYY-MM-DD` in UTC.
pub fn today(now: u64) -> String {
    day_of(now)
}

/// How many findings a review file's `text` has: its headings that start with [`FINDING`].
pub fn findings(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with(FINDING)).count()
}

/// One loop's log: when the loop started, when its file was last written, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopLog {
    pub start: u64,
    /// The log file's modified time: no row of it started later.
    pub end: u64,
    pub text: String,
}

/// A full time a record holds for a log row's item: `key` is what the row's Issue cell starts
/// with (`#<issue or PR>`, or a job's label, `job spec <file>`), `at` a stamp of its task's
/// record or its job's result. Used to tell the day of a row whose clock fits several days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub key: String,
    pub at: u64,
}

/// The run data a spec embeds, at most [`CAP`] bytes, and how many rows were left out. It is
/// only built from sources read whole: a source that skipped something fails the check instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunData {
    pub text: String,
    pub left_out: usize,
}

/// When a loop-log row started: a time, or, when neither the log's span nor a record tells the
/// day, the first and last times its clock could be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    At(u64),
    Unknown { first: u64, last: u64 },
}

impl When {
    /// The latest it can be: what the window and the cap go by.
    fn latest(self) -> u64 {
        match self {
            Self::At(at) => at,
            Self::Unknown { last, .. } => last,
        }
    }
}

/// One row of the run data, with when it happened.
struct TimedRow {
    at: u64,
    /// 0: a task row; 1: a loop-log row.
    part: usize,
    line: String,
}

/// The run data since `since` (`None`: all): `facts`, the Markdown table of `sbxm task facts`
/// and the table rows of `logs` that started on that day (UTC) or
/// later, each with its loop's start and its own day. A row's day comes from its clock between
/// its loop's start and its log's last write, and when that spans more than a day, from the
/// nearest of `anchors` for its item; a row neither tells is kept with its day shown unknown,
/// never guessed. When the whole is longer than [`CAP`] bytes, the oldest rows of either part are
/// left out until it fits.
pub fn run_data(facts: &str, logs: &[LoopLog], anchors: &[Anchor], since: Option<&str>) -> RunData {
    let window = since.map_or_else(|| "all".to_owned(), |d| format!("since {d}"));
    let from = since.and_then(parse_day).unwrap_or(0);
    let mut rows = Vec::new();
    let facts_lines: Vec<&str> = facts.lines().collect();
    let facts_header = facts_lines
        .iter()
        .take_while(|l| !l.starts_with("|---"))
        .chain(facts_lines.iter().find(|l| l.starts_with("|---")))
        .map(|l| format!("{l}\n"))
        .collect::<String>();
    for line in table_rows(facts) {
        let at = cells(line)
            .get(2)
            .and_then(|s| parse_timestamp(s))
            .unwrap_or(0);
        rows.push(TimedRow {
            at,
            part: 0,
            line: format!("{line}\n"),
        });
    }
    let mut log_header = String::new();
    for log in logs {
        let stamp = format_timestamp(log.start);
        let lines: Vec<&str> = log.text.lines().collect();
        if let Some(sep) = lines.iter().position(|l| l.starts_with("|---")) {
            let head = sep.checked_sub(1).map_or("", |i| lines[i]);
            log_header = format!("| Loop started | Day {head}\n|---|---{}\n", lines[sep]);
        }
        for line in table_rows(&log.text) {
            let when = when(log, line, anchors);
            // A loop that started before the window logs into it too: only its rows from then.
            if when.latest() < from {
                continue;
            }
            let day = match when {
                When::At(at) => day_of(at),
                When::Unknown { first, last } => {
                    format!("unknown ({} to {})", day_of(first), day_of(last))
                }
            };
            rows.push(TimedRow {
                at: when.latest(),
                part: 1,
                line: format!("| {stamp} | {day} {line}\n"),
            });
        }
    }
    let facts_title = format!(
        "### Tasks (`sbxm task facts --since {}`)\n\n",
        since.unwrap_or("1970-01-01")
    );
    let logs_title = format!("\n### Loop-log rows ({window}, each with its loop's start)\n\n");
    let render = |parts: &[String; 2]| -> String {
        let mut text = facts_title.clone();
        text.push_str(&facts_header);
        text.push_str(&parts[0]);
        text.push_str(&logs_title);
        if log_header.is_empty() {
            text.push_str("No loop log of this repository in the window.\n");
        } else {
            text.push_str(&log_header);
            text.push_str(&parts[1]);
        }
        text
    };
    // The text around the rows, measured as it is rendered; each row adds its own bytes.
    let fixed = render(&[String::new(), String::new()]).len();
    // Newest first, kept while they fit; a stable sort keeps a part's own order on a tie.
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((rows[i].at, i)));
    let mut kept = vec![false; rows.len()];
    let mut size = fixed;
    let mut left_out = 0;
    for i in order {
        if left_out == 0 && size + rows[i].line.len() <= CAP {
            size += rows[i].line.len();
            kept[i] = true;
        } else {
            left_out += 1;
        }
    }
    let mut parts = [String::new(), String::new()];
    let mut by_time: Vec<usize> = (0..rows.len()).filter(|&i| kept[i]).collect();
    by_time.sort_by_key(|&i| (rows[i].at, i));
    for i in by_time {
        parts[rows[i].part].push_str(&rows[i].line);
    }
    RunData {
        text: render(&parts),
        left_out,
    }
}

/// How far a row's start may be from its task's or job's start and still be the same run: the
/// loop logs when it starts a task, the record is written moments later, after any slot wait.
const ANCHOR_SLACK: u64 = 3 * 60 * 60;

/// When the loop-log row `line` of `log` started (see [`run_data`]).
fn when(log: &LoopLog, line: &str, anchors: &[Anchor]) -> When {
    let unknown = When::Unknown {
        first: log.start,
        last: log.end.max(log.start),
    };
    let cells = cells(line);
    let Some(first) = cells.get(4).and_then(|clock| clock_at(log.start, clock)) else {
        return unknown;
    };
    let candidates: Vec<u64> = (0..)
        .map(|k| first + k * DAY)
        .take_while(|&t| t <= log.end)
        .collect();
    match candidates.as_slice() {
        [] => unknown,
        [only] => When::At(*only),
        [.., last] => {
            let key = cells.get(1).map(|item| item_key(item)).unwrap_or_default();
            let near = anchors
                .iter()
                .filter(|a| !key.is_empty() && a.key == key)
                .map(|a| a.at)
                .collect::<Vec<_>>();
            // A day is told only when exactly one candidate is a start's: two runs of the same
            // item on different days, or a start far from every candidate, tell none.
            let matching: Vec<u64> = candidates
                .iter()
                .copied()
                .filter(|&t| near.iter().any(|&a| a.abs_diff(t) <= ANCHOR_SLACK))
                .collect();
            match matching.as_slice() {
                [only] => When::At(*only),
                _ => When::Unknown { first, last: *last },
            }
        }
    }
}

/// The key of a log row's Issue cell an [`Anchor`] is looked up by: `#<n>` of `#<n> <title>`,
/// else the whole cell (a job's label).
fn item_key(item: &str) -> String {
    match item.strip_prefix('#') {
        Some(rest) => {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            format!("#{digits}")
        }
        None => item.to_owned(),
    }
}

/// The table rows of `text`: the `|` lines after its first separator line.
fn table_rows(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .skip_while(|l| !l.starts_with("|---"))
        .skip(1)
        .filter(|l| l.starts_with('|'))
}

/// The first time at or after `start` whose clock is a row's `HH:MM:SS`.
fn clock_at(start: u64, clock: &str) -> Option<u64> {
    let mut parts = clock.split(':').map(|p| p.parse::<u64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || h > 23 || m > 59 || s > 59 {
        return None;
    }
    let of_day = h * 3600 + m * 60 + s;
    let start_of_day = start % DAY;
    Some(start - start_of_day + of_day + if of_day < start_of_day { DAY } else { 0 })
}

/// The [`Anchor`]s under `base_dir` for `repo`'s loop-log rows: each task's start, the first
/// stamp of its record, live or archived, keyed by what its row names: an issue task by its
/// issue (`#<n>`), a fix task (a resolve or a log-fix) by its PR; and each finished job's start,
/// keyed by its label. A later stage is never an anchor: it isn't when a row started. They only
/// help tell a row's day: a record or result that can't be read gives none, and such a row is
/// then shown with its day unknown.
pub fn anchors(base_dir: &Path, repo: &str) -> Vec<Anchor> {
    let mut found = Vec::new();
    let mut dirs = record::task_folders(base_dir).unwrap_or_default();
    if let Ok(entries) = fs::read_dir(super::merged::archive_root(base_dir)) {
        dirs.extend(entries.flatten().map(|e| e.path()));
    }
    for dir in dirs {
        let Ok(rec) = record::read(&dir.join("task.json")) else {
            continue;
        };
        if rec.repo != repo {
            continue;
        }
        let key = match rec.kind {
            Kind::Issue => format!("#{}", rec.number),
            Kind::Fix => match rec.pr.as_deref().and_then(queue::pr_number) {
                Some(pr) => format!("#{pr}"),
                None => continue,
            },
            // A review or spec task runs as a job: its row is the job's, anchored below.
            _ => continue,
        };
        if let Some(at) = rec.stages.first().and_then(|s| parse_timestamp(&s.at)) {
            found.push(Anchor { key, at });
        }
    }
    let done = fs::read_dir(jobs::done_dir(base_dir))
        .map(|entries| entries.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default();
    for path in done {
        let Some(result) = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<jobs::JobResult>(&text).ok())
        else {
            continue;
        };
        let Some(job) = result
            .read
            .job
            .as_ref()
            .and_then(|value| jobs::parse(&value.to_string()).ok())
        else {
            continue;
        };
        if let Some(at) = parse_timestamp(&result.started_at) {
            found.push(Anchor {
                key: job.label(),
                at,
            });
        }
    }
    found
}

/// The spec of `area`'s review on `date`, the embedded `prompts/improvement-review.md` filled in.
pub fn spec(area: &Area, date: &str, since: Option<&str>, data: &RunData) -> Result<String> {
    let since_text = since.map_or_else(
        || "the first task (no improvement review yet)".to_owned(),
        str::to_owned,
    );
    let left_out = if data.left_out == 0 {
        format!("All the run data since {since_text} is below.")
    } else {
        format!(
            "The run data is capped at 30 KB: the {} oldest rows were left out, the newest kept.",
            data.left_out
        )
    };
    prompts::render_known(
        TEMPLATE_NAME,
        TEMPLATE,
        &KNOWN,
        &[
            ("slug", &area.slug),
            ("date", date),
            ("paths", &area.paths),
            ("output", &output(&area.slug, date)),
            ("since", &since_text),
            ("run_data", &data.text),
            ("left_out", &left_out),
        ],
    )
}
