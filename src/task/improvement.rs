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

use anyhow::{Result, anyhow};

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

/// Whether a spec job of `file` waits in the queue or runs.
pub fn queued_spec(base_dir: &Path, file: &str) -> bool {
    let files = jobs::pending(base_dir)
        .unwrap_or_default()
        .into_iter()
        .chain(jobs::left_running(base_dir).unwrap_or_default());
    files
        .into_iter()
        .any(|path| matches!(spec_job(&path), Some(queued) if queued == file))
}

/// The file of the spec job at `path`; `None` for any other job, or one that can't be read.
fn spec_job(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let value = Running::read(&text).read.job?;
    match jobs::parse(&value.to_string()).ok()? {
        Job::Spec { file } => Some(file),
        _ => None,
    }
}

/// Whether an improvement job of `repo` is in flight under `base_dir`: a spec job of an
/// [`SPEC_PREFIX`] file queued or running, or a live spec task of one whose PR GitHub says is
/// open. A job or record that can't be read counts as none.
pub fn in_flight(base_dir: &Path, repo: &str, github: &dyn GitHubBackend) -> Result<bool> {
    let files = jobs::pending(base_dir)?
        .into_iter()
        .chain(jobs::left_running(base_dir)?);
    for path in files {
        if spec_job(&path).is_some_and(|file| file.starts_with(SPEC_PREFIX)) {
            return Ok(true);
        }
    }
    for dir in record::task_folders(base_dir)? {
        let Ok(rec) = record::read(&dir.join("task.json")) else {
            continue;
        };
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

/// One loop's log: when the loop started and the file's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopLog {
    pub start: u64,
    pub text: String,
}

/// The run data a spec embeds, at most [`CAP`] bytes, and how many rows were left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunData {
    pub text: String,
    pub left_out: usize,
}

/// One row of the run data, with when it happened.
struct TimedRow {
    at: u64,
    /// 0: a task row; 1: a loop-log row.
    part: usize,
    line: String,
}

/// The run data since `since` (`None`: all): `facts`, the Markdown table of `sbxm task facts`,
/// and the table rows of `logs` from the start of that day (UTC), each prefixed with its loop's
/// start. When it is longer than
/// [`CAP`] bytes, the oldest rows of either part are left out until it fits.
pub fn run_data(facts: &str, logs: &[LoopLog], since: Option<&str>) -> RunData {
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
            log_header = format!("| Loop started {head}\n|---{}\n", lines[sep]);
        }
        for line in table_rows(&log.text) {
            let at = cells(line)
                .get(4)
                .and_then(|clock| clock_at(log.start, clock))
                .unwrap_or(log.start);
            // A loop that started before the window logs into it too: only its rows from then.
            if at < from {
                continue;
            }
            rows.push(TimedRow {
                at,
                part: 1,
                line: format!("| {stamp} {line}\n"),
            });
        }
    }
    let facts_title = format!(
        "### Tasks (`sbxm task facts --since {}`)\n\n",
        since.unwrap_or("1970-01-01")
    );
    let logs_title = format!("\n### Loop-log rows ({window}, each with its loop's start)\n\n");
    let fixed = facts_title.len() + facts_header.len() + logs_title.len() + log_header.len();
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
    let mut text = facts_title;
    text.push_str(&facts_header);
    text.push_str(&parts[0]);
    text.push_str(&logs_title);
    if log_header.is_empty() {
        text.push_str("No loop log of this repository in the window.\n");
    } else {
        text.push_str(&log_header);
        text.push_str(&parts[1]);
    }
    RunData { text, left_out }
}

/// The table rows of `text`: the `|` lines after its first separator line.
fn table_rows(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .skip_while(|l| !l.starts_with("|---"))
        .skip(1)
        .filter(|l| l.starts_with('|'))
}

/// When a row's `HH:MM:SS` clock happened, in a loop that started at `start`: the first such
/// time at or after the start, so a row past midnight falls on the next day.
fn clock_at(start: u64, clock: &str) -> Option<u64> {
    let mut parts = clock.split(':').map(|p| p.parse::<u64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    let of_day = h * 3600 + m * 60 + s;
    let start_of_day = start % DAY;
    Some(start - start_of_day + of_day + if of_day < start_of_day { DAY } else { 0 })
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
