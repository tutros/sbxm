//! The medium-risk auto-merge trial (issue 550, decision 316(4)): `task loop` arms medium-risk
//! PRs until the open issue labelled [`LABEL`] holds [`SIZE`] merge comments, then hands them back
//! to the user until the retro decides. Its record is that issue's comments, read each time: one
//! per medium PR the loop armed that merged (how it was armed, its `hold` labellings, minutes from
//! the clean review to the merge, how late the arming was) and one per break. sbxm keeps no state
//! of its own for it.

use crate::github::{GitHubBackend, LabelEvent};
use crate::task::window::{GO_LABEL, HOLD_LABEL};

/// The label of the issue the trial is recorded on.
pub const LABEL: &str = "medium-trial";

/// How many medium auto-merges the trial runs for.
pub const SIZE: usize = 10;

/// The first line of a merge comment.
const MERGE_MARK: &str = "Medium trial: merge";

/// The first line of a break comment.
const BREAK_MARK: &str = "Medium trial: break";

/// What set a medium PR's arming off (#460 answer 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The user labelled it `go`.
    Go,
    /// Its veto window ended.
    WindowEnd,
}

/// A medium PR the loop armed, as the trial records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Armed {
    pub how: How,
    /// Minutes from the window's end, or from `go` being labelled, to the arming (answer 1).
    pub late_min: u64,
    /// When its clean review was posted (the window's start), seconds since the epoch.
    pub review_at: u64,
}

/// A medium PR the loop armed that merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Merge {
    pub pr: u32,
    pub armed: Armed,
    /// Its `hold` labellings, from its label history.
    pub holds: usize,
    /// When it merged, seconds since the epoch.
    pub merged_at: u64,
}

/// Whole minutes from `from` to `to`; 0 when `to` is earlier.
pub fn minutes(from: u64, to: u64) -> u64 {
    to.saturating_sub(from) / 60
}

/// The comment recording `merge`.
pub fn merge_comment(merge: &Merge) -> String {
    let armed = match merge.armed.how {
        How::Go => format!(
            "`{GO_LABEL}`, {} min after it was labelled",
            merge.armed.late_min
        ),
        How::WindowEnd => format!(
            "the window's end, {} min after it ended",
            merge.armed.late_min
        ),
    };
    format!(
        "{MERGE_MARK} of PR #{}\n\n- armed by: {armed}\n- holds: {}\n- clean review to merge: {} \
         min\n",
        merge.pr,
        merge.holds,
        minutes(merge.armed.review_at, merge.merged_at)
    )
}

/// The comment recording a break, `why` naming it.
pub fn break_comment(why: &str) -> String {
    format!("{BREAK_MARK}\n\n{why}\n")
}

/// How many of `bodies` are merge comments: their first line starts with the merge mark,
/// whoever posted them. Break comments and anything else don't count.
pub fn merges(bodies: &[String]) -> usize {
    bodies
        .iter()
        .filter(|body| {
            body.lines()
                .next()
                .is_some_and(|line| line.trim_start().starts_with(MERGE_MARK))
        })
        .count()
}

/// Whether `merges` merge comments end the trial.
pub fn full(merges: usize) -> bool {
    merges >= SIZE
}

/// How many times `events` add the `hold` label, in any case.
pub fn holds(events: &[LabelEvent]) -> usize {
    events
        .iter()
        .filter(|e| e.added && e.label.eq_ignore_ascii_case(HOLD_LABEL))
        .count()
}

/// When `events` last added the `go` label, in any case.
pub fn go_at(events: &[LabelEvent]) -> Option<u64> {
    events
        .iter()
        .filter(|e| e.added && e.label.eq_ignore_ascii_case(GO_LABEL))
        .map(|e| e.at)
        .max()
}

/// The trial as GitHub has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trial {
    /// No open issue is labelled [`LABEL`].
    NoIssue,
    /// The oldest open issue labelled [`LABEL`], and its merge comments.
    Open { issue: u32, merges: usize },
}

impl Trial {
    /// Whether the trial's merges are all in.
    pub fn full(&self) -> bool {
        matches!(self, Self::Open { merges, .. } if full(*merges))
    }
}

/// Reads the trial: the oldest open issue labelled [`LABEL`] and its merge comments. Why it can't
/// be read otherwise.
pub fn read(github: &dyn GitHubBackend, repo: &str) -> Result<Trial, String> {
    let Some(issue) = issue(github, repo)? else {
        return Ok(Trial::NoIssue);
    };
    let bodies = github.issue_comment_bodies(repo, issue).map_err(|e| {
        format!(
            "the comments of the medium trial's issue #{issue} can't be read: {}",
            first_line(&format!("{e:#}"))
        )
    })?;
    Ok(Trial::Open {
        issue,
        merges: merges(&bodies),
    })
}

/// Posts `body` on the trial's issue; `None` when there is none open.
pub fn post(github: &dyn GitHubBackend, repo: &str, body: &str) -> anyhow::Result<Option<u32>> {
    let Some(issue) = issue(github, repo).map_err(anyhow::Error::msg)? else {
        return Ok(None);
    };
    github.issue_comment(repo, issue, body)?;
    Ok(Some(issue))
}

fn issue(github: &dyn GitHubBackend, repo: &str) -> Result<Option<u32>, String> {
    let issues = github.issues_open_labeled(repo, LABEL).map_err(|e| {
        format!(
            "the open issue labelled {LABEL} can't be listed: {}",
            first_line(&format!("{e:#}"))
        )
    })?;
    Ok(issues.iter().map(|i| i.number).min())
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}
