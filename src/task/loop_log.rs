//! Where `task loop`'s log lives and how it reaches GitHub (issue 463, decision 305): one file
//! per loop under `<base>/.sbxm/loop-logs/`, never in a checkout, posted at the stop as a comment
//! on the open issue labeled `loop-log`, split between rows when it is longer than one comment.
//! A posted log gets a marker next to it; at its start a loop posts, late, every earlier log
//! with rows that has none, so the log of a loop ended without its stop isn't lost (issue 594).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::github::{GitHubBackend, IssueRequest};
use crate::run::results::format_timestamp;

/// The label of the issue the loop's logs are posted to, one issue per retro window.
pub const LABEL: &str = "loop-log";

/// The longest comment text posted whole; GitHub refuses a comment over 65,536 characters.
pub const COMMENT_MAX: usize = 65_000;

const ISSUE_BODY: &str = "Logs of `sbxm task loop` runs, one comment per loop, until the next \
                          retro reads and closes this issue (issue 463).";

/// `<base>/.sbxm/loop-logs/<date>-<HHMMSS>-loop.md`, named by the loop's start in UTC.
pub fn path(base_dir: &Path, started_at: u64) -> PathBuf {
    let stamp = format_timestamp(started_at);
    let (date, time) = stamp.split_once('T').unwrap_or((&stamp, ""));
    let time: String = time.chars().filter(char::is_ascii_digit).collect();
    base_dir
        .join(".sbxm")
        .join("loop-logs")
        .join(format!("{date}-{time}-loop.md"))
}

/// The comments that post `log` under `heading`: one when it fits in [`COMMENT_MAX`]
/// characters, else parts split between table rows (never inside one), each repeating the
/// heading, marked `(part i of n)`, and the log's header up to its table's separator line. A row
/// too long for a part on its own is posted alone.
pub fn comments(heading: &str, log: &str) -> Vec<String> {
    let whole = format!("{heading}\n\n{log}");
    if whole.chars().count() <= COMMENT_MAX {
        return vec![whole];
    }
    let lines: Vec<&str> = log.split_inclusive('\n').collect();
    let header_end = lines
        .iter()
        .position(|line| line.starts_with("|---"))
        .map_or(0, |i| i + 1);
    let header = lines[..header_end].concat();
    let rows = &lines[header_end..];
    let widest = format!(" (part {0} of {0})", rows.len()).chars().count();
    let budget =
        COMMENT_MAX.saturating_sub(heading.chars().count() + 2 + header.chars().count() + widest);
    let mut parts: Vec<String> = Vec::new();
    let mut part = String::new();
    let mut len = 0;
    for row in rows {
        let row_len = row.chars().count();
        if !part.is_empty() && len + row_len > budget {
            parts.push(std::mem::take(&mut part));
            len = 0;
        }
        part.push_str(row);
        len += row_len;
    }
    if !part.is_empty() {
        parts.push(part);
    }
    let n = parts.len();
    parts
        .iter()
        .enumerate()
        .map(|(i, rows)| format!("{heading} (part {} of {n})\n\n{header}{rows}", i + 1))
        .collect()
}

/// The marker of a log that was posted, or had no rows to post (issue 594):
/// `<date>-<HHMMSS>-loop.posted` next to the log.
pub fn marker(log: &Path) -> PathBuf {
    log.with_extension("posted")
}

/// Records that `log` needs no post any more.
pub fn mark_posted(log: &Path) -> Result<()> {
    let marker = marker(log);
    fs::write(&marker, "posted\n").with_context(|| format!("cannot write {}", marker.display()))
}

/// Posts the log at `log` to the open `loop-log` issue with the lowest number, creating one
/// titled by the loop's start date when there is none. Nothing is retried; the caller marks it
/// posted ([`mark_posted`]).
pub fn post(
    github: &dyn GitHubBackend,
    repo: &str,
    log: &Path,
    started_at: u64,
    stop: &str,
) -> Result<()> {
    let text = fs::read_to_string(log).with_context(|| format!("cannot read {}", log.display()))?;
    let start = format_timestamp(started_at);
    let heading = format!(
        "Loop started {start}, stopped: {stop}. Local file: `{}`",
        log.display()
    );
    post_text(github, repo, &heading, &text, &start)
}

/// The most issues read when looking for the logs already on GitHub, as `task file-findings`
/// reads them.
const ISSUE_LIST_LIMIT: u32 = 1000;

/// Posts, oldest first, every earlier loop log under `base_dir` with rows and no marker, each
/// headed by the late note naming this loop's start, and marks it posted; one without rows is
/// only marked, and one already on GitHub (a `loop-log` issue's comment names its file, as the
/// stop of a loop from before the markers posted it; review M-1) is marked without a comment.
/// `own` (this loop's log) is never posted here. Returns the errors, each naming its log: a log
/// that fails stays unmarked for the next start and the later ones are still tried (review M-2).
pub fn post_late(
    github: &dyn GitHubBackend,
    repo: &str,
    base_dir: &Path,
    own: &Path,
    started_at: u64,
) -> Vec<anyhow::Error> {
    let mut failed = Vec::new();
    let logs = match unmarked_logs(base_dir, own) {
        Ok(logs) => logs,
        Err(e) => return vec![e],
    };
    // The logs with rows, read before anything is asked of GitHub.
    let mut due: Vec<(PathBuf, String)> = Vec::new();
    for log in logs {
        let read = fs::read_to_string(&log)
            .with_context(|| format!("cannot read {}", log.display()))
            .and_then(|text| {
                if has_rows(&text) {
                    due.push((log.clone(), text));
                    Ok(())
                } else {
                    mark_posted(&log)
                }
            });
        if let Err(e) = read {
            failed.push(e.context(format!("{} stays unposted", log.display())));
        }
    }
    if due.is_empty() {
        return failed;
    }
    let on_github = match posted_comments(github, repo) {
        Ok(bodies) => bodies,
        Err(e) => {
            failed.push(e.context(
                "cannot tell which earlier logs are already on GitHub, so none was posted",
            ));
            return failed;
        }
    };
    let now = format_timestamp(started_at);
    for (log, text) in due {
        let name = log
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let named = format!("{name}`");
        let result = if on_github.iter().any(|body| body.contains(&named)) {
            mark_posted(&log)
        } else {
            post_one_late(github, repo, &log, &text, &now)
        };
        if let Err(e) = result {
            failed.push(e.context(format!("{} stays unposted", log.display())));
        }
    }
    failed
}

/// The earlier logs under `base_dir` without a marker, oldest first, `own` left out.
fn unmarked_logs(base_dir: &Path, own: &Path) -> Result<Vec<PathBuf>> {
    let dir = base_dir.join(".sbxm").join("loop-logs");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut logs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let name = entry.file_name();
        let is_log = name.to_str().is_some_and(|n| n.ends_with("-loop.md"));
        if is_log && entry.file_type().is_ok_and(|t| t.is_file()) {
            logs.push(entry.path());
        }
    }
    // `<date>-<HHMMSS>` names sort by start.
    logs.sort();
    logs.retain(|l| l.as_path() != own && fs::symlink_metadata(marker(l)).is_err());
    Ok(logs)
}

/// Every comment on the repository's `loop-log` issues, open or closed.
fn posted_comments(github: &dyn GitHubBackend, repo: &str) -> Result<Vec<String>> {
    let issues = github.issues_all(repo, ISSUE_LIST_LIMIT)?;
    if issues.len() >= ISSUE_LIST_LIMIT as usize {
        bail!(
            "the issue list has {} issues, the most read at once",
            issues.len()
        );
    }
    let mut bodies = Vec::new();
    for issue in issues
        .iter()
        .filter(|i| i.labels.iter().any(|l| l == LABEL))
    {
        bodies.extend(github.issue_comment_bodies(repo, issue.number)?);
    }
    Ok(bodies)
}

fn post_one_late(
    github: &dyn GitHubBackend,
    repo: &str,
    log: &Path,
    text: &str,
    now: &str,
) -> Result<()> {
    let start = start_of(log);
    let heading = format!(
        "Posted late by the loop started {now}: the loop that wrote it ended without its \
         stop (Ctrl-C, a crash or a restart).\n\nLoop started {start}. Local file: `{}`",
        log.display()
    );
    post_text(github, repo, &heading, text, &start)?;
    mark_posted(log)
}

/// Whether `text` has a table row: a line whose first cell is a number.
fn has_rows(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with('|')
            && line
                .split('|')
                .nth(1)
                .is_some_and(|n| n.trim().parse::<usize>().is_ok())
    })
}

/// `2026-11-08T10:00:00Z` from a log named `2026-11-08-100000-loop.md`, else its file name.
fn start_of(log: &Path) -> String {
    let name = log
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix("-loop.md").unwrap_or(&name);
    match stem.rsplit_once('-') {
        Some((date, time)) if time.len() == 6 && time.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{date}T{}:{}:{}Z", &time[..2], &time[2..4], &time[4..])
        }
        _ => name,
    }
}

/// Posts `text` under `heading` to the open `loop-log` issue with the lowest number, creating
/// one titled by the date of `start` when there is none.
fn post_text(
    github: &dyn GitHubBackend,
    repo: &str,
    heading: &str,
    text: &str,
    start: &str,
) -> Result<()> {
    let found = github
        .issues_open_labeled(repo, LABEL)?
        .iter()
        .map(|i| i.number)
        .min();
    let number = match found {
        Some(number) => number,
        None => {
            let date = start.split_once('T').map_or(start, |(d, _)| d);
            github.issue_create(
                repo,
                &IssueRequest {
                    title: format!("Loop logs since {date}"),
                    body: ISSUE_BODY.to_owned(),
                    labels: vec![LABEL.to_owned()],
                },
            )?
        }
    };
    for comment in comments(heading, text) {
        github.issue_comment(repo, number, &comment)?;
    }
    Ok(())
}
