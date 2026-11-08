//! Where `task loop`'s log lives and how it reaches GitHub (issue 463, decision 305): one file
//! per loop under `<base>/.sbxm/loop-logs/`, never in a checkout, posted at the stop as a comment
//! on the open issue labeled `loop-log`, split between rows when it is longer than one comment.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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

/// Posts the log at `log` to the open `loop-log` issue with the lowest number, creating one
/// titled by the loop's start date when there is none. Nothing is retried.
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
    let open = github.issues_open(repo)?;
    let found = open
        .iter()
        .filter(|i| i.labels.iter().any(|l| l == LABEL))
        .map(|i| i.number)
        .min();
    let number = match found {
        Some(number) => number,
        None => {
            let date = start.split_once('T').map_or(start.as_str(), |(d, _)| d);
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
    for comment in comments(&heading, &text) {
        github.issue_comment(repo, number, &comment)?;
    }
    Ok(())
}
