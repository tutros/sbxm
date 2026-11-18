//! The retro alert (decision 330(4), issue 714): a retro is due once [`THRESHOLD`] PRs have merged
//! since the end of the `Window: <start> to <end>` line of the newest `sdlc/retros/*-retro.md` on
//! the default branch, "since" being from the day after that end, in UTC (the `sdlc-retro`
//! skill's rule). No record, or a newest record with no `Window:` line that can be read, is due
//! too. Everything is read through [`GitHubBackend`]; no local checkout is involved. The loop
//! acts on it ([`check`]); `sbxm status` shows the line the loop records ([`Due::status_line`]).

use anyhow::{Result, anyhow};

use crate::github::GitHubBackend;
use crate::run::results::parse_day;

/// The folder of the retro records on the default branch.
pub const DIR: &str = "sdlc/retros";
/// The file name a retro record ends with.
pub const SUFFIX: &str = "-retro.md";
/// How many merged PRs make a retro due.
pub const THRESHOLD: usize = 10;
/// How long one check is reused: the loop checks at most once an hour.
pub const CHECK_EVERY: u64 = 60 * 60;

const DAY: u64 = 24 * 60 * 60;

/// Why a retro is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Due {
    /// `prs` PRs merged since `end`, the newest record's window end.
    Prs { prs: usize, end: String },
    /// No retro record on `branch`.
    NoRecord { branch: String },
    /// The newest record, at `file`, has no `Window:` line that can be read.
    NoWindow { file: String },
}

impl Due {
    /// What the alert is about, the same for as long as it stays due: the window's end, `none`,
    /// or the record's path. A notification key and the comment's identity are made of it.
    pub fn about(&self) -> &str {
        match self {
            Self::Prs { end, .. } => end,
            Self::NoRecord { .. } => "none",
            Self::NoWindow { file } => file,
        }
    }

    /// The day's notification key: `retro-due-<end>-<YYYY-MM-DD>`, one toast a day.
    pub fn key(&self, now: u64) -> String {
        format!("retro-due-{}-{}", self.about(), day_of(now))
    }

    /// The comment on the open `loop-log` issue, posted once.
    pub fn comment(&self) -> String {
        match self {
            Self::Prs { prs, end } => format!("Retro due: {prs} PRs merged since {end}."),
            Self::NoRecord { branch } => {
                format!("Retro due: no retro record (`{DIR}/*{SUFFIX}`) on `{branch}`.")
            }
            Self::NoWindow { file } => format!(
                "Retro due: `{file}` has no `Window: <start> to <end>` line that can be read, so \
                 it counts as no retro record."
            ),
        }
    }

    /// Whether `body`, a comment already posted, is this alert's: the PR count may have grown
    /// since, so only what it is about is compared.
    pub fn commented_in(&self, body: &str) -> bool {
        match self {
            Self::Prs { end, .. } => {
                body.starts_with("Retro due: ")
                    && body.ends_with(&format!(" PRs merged since {end}."))
            }
            _ => body == self.comment(),
        }
    }

    /// The line `sbxm status` shows under the loop's lines.
    pub fn status_line(&self) -> String {
        match self {
            Self::Prs { prs, end } => format!("retro due: {prs} PRs since {end}"),
            Self::NoRecord { branch } => format!("retro due: no retro record on {branch}"),
            Self::NoWindow { file } => format!("retro due: {file} has no readable Window: line"),
        }
    }
}

/// `YYYY-MM-DD` of `secs`, in UTC.
pub fn day_of(secs: u64) -> String {
    let stamp = crate::run::results::format_timestamp(secs);
    stamp
        .split_once('T')
        .map_or(stamp.clone(), |(d, _)| d.to_owned())
}

/// The end date of the first `Window: <start> to <end>` line of `text`; `None` when there is
/// none or its end isn't a `YYYY-MM-DD` calendar date.
pub fn window_end(text: &str) -> Option<String> {
    let line = text
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("Window:"))?;
    let (_, rest) = line.split_once(" to ")?;
    let end = rest.trim_start().get(..10)?;
    parse_day(end)?;
    Some(end.to_owned())
}

/// Whether a retro is due for `repo`, read from its default branch and its merged PRs: `None`
/// when it isn't.
pub fn check(github: &dyn GitHubBackend, repo: &str) -> Result<Option<Due>> {
    let branch = github.default_branch(repo)?;
    // `YYYY-MM-DD-retro.md`: the newest sorts last.
    let newest = github
        .dir_files(repo, &branch, DIR)?
        .into_iter()
        .filter(|name| name.ends_with(SUFFIX))
        .max();
    let Some(name) = newest else {
        return Ok(Some(Due::NoRecord { branch }));
    };
    let file = format!("{DIR}/{name}");
    let Some(end) = window_end(&github.file_text(repo, &branch, &file)?) else {
        return Ok(Some(Due::NoWindow { file }));
    };
    let after = parse_day(&end).ok_or_else(|| anyhow!("{end} isn't a day"))? + DAY;
    let prs = github
        .prs_merged_since(repo, &end)?
        .iter()
        .filter(|pr| pr.merged_at >= after)
        .count();
    Ok((prs >= THRESHOLD).then_some(Due::Prs { prs, end }))
}
