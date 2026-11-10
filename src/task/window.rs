//! The veto window before `task loop` arms auto-merge on a medium-risk PR (issue 548, decision
//! 316(3), #460 guardrail 5): two hours from the PR head's latest `sbxm/review` status, when that
//! status is a success. A `hold` label keeps the PR unarmed, a `go` label arms it at once, neither
//! arms it when the window ends; labels match case-insensitively, as GitHub's do. `go` only skips
//! the wait: the other arming rules ([`crate::task::medium`], the not-run list...) come first.
//! The window's start is read from GitHub each time, so no state of sbxm's holds it. Nothing
//! calls this before #550, which arms by it; low risk never comes here (decisions 266, 280).

use crate::github::{GitHubBackend, PostedStatus};
use crate::task::review::STATUS_CONTEXT;

/// How long the window lasts.
pub const WINDOW_SECS: u64 = 2 * 3_600;

/// The label that keeps a PR unarmed.
pub const HOLD_LABEL: &str = "hold";

/// The label that arms a PR at once.
pub const GO_LABEL: &str = "go";

/// Where a medium-risk PR stands in its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    /// Labelled `hold`: not armed, and disarmed if it is (#550).
    Held,
    /// Another arming rule refuses it, with that rule's reasons; `go` doesn't change that.
    Refused(Vec<String>),
    /// No clean review of its head starts a window, with the reason.
    NotEligible(String),
    /// Labelled `go`: arm now.
    Go,
    /// In the window: armed at `until` (seconds since the epoch) unless labelled meanwhile.
    Waiting { until: u64 },
    /// The window ended: arm now.
    Over,
}

impl Window {
    /// Whether the PR is to be armed now.
    pub fn arm_now(&self) -> bool {
        matches!(self, Self::Go | Self::Over)
    }

    /// Which of the trial's outcomes applied (#460 answer 5), for #550's record: `go`, `window
    /// end` or `held`; `None` while waiting or refused.
    pub fn how(&self) -> Option<&'static str> {
        match self {
            Self::Go => Some("go"),
            Self::Over => Some("window end"),
            Self::Held => Some("held"),
            _ => None,
        }
    }
}

/// When the window starts: the time of the latest `sbxm/review` status of `statuses` (the PR
/// head's), if that one is a success; the reason it doesn't start otherwise. Of two at the same
/// time, the one listed first (GitHub lists the newest first).
pub fn start(statuses: &[PostedStatus]) -> Result<u64, String> {
    let mut latest: Option<&PostedStatus> = None;
    for status in statuses.iter().filter(|s| s.context == STATUS_CONTEXT) {
        if latest.is_none_or(|l| status.at > l.at) {
            latest = Some(status);
        }
    }
    match latest {
        None => Err(format!("its head has no `{STATUS_CONTEXT}` status")),
        Some(status) if status.state == "success" => Ok(status.at),
        Some(status) => Err(format!(
            "its head's latest `{STATUS_CONTEXT}` status is {}",
            status.state
        )),
    }
}

/// The window of a medium-risk PR with `labels`, whose window starts at `start`, at `now`.
/// `refusals` are the other arming rules' reasons; `go_ignored` is set when the loop couldn't
/// remove its `go` label before a push, so that `go` counts as absent. In order: `hold`, then
/// the other rules, then the review, then `go`, then the time.
pub fn decide(
    labels: &[String],
    start: Result<u64, String>,
    refusals: &[String],
    go_ignored: bool,
    now: u64,
) -> Window {
    if has_label(labels, HOLD_LABEL) {
        return Window::Held;
    }
    if !refusals.is_empty() {
        return Window::Refused(refusals.to_vec());
    }
    let start = match start {
        Ok(start) => start,
        Err(why) => return Window::NotEligible(why),
    };
    if !go_ignored && has_label(labels, GO_LABEL) {
        return Window::Go;
    }
    let until = start.saturating_add(WINDOW_SECS);
    if now < until {
        Window::Waiting { until }
    } else {
        Window::Over
    }
}

/// [`decide`] for PR `number` of `repo` at `head`, its labels and the head's statuses read from
/// GitHub. Labels that can't be read make it not eligible (a `hold` may be among them); statuses
/// that can't be read do too, unless it is held.
pub fn read(
    github: &dyn GitHubBackend,
    repo: &str,
    number: u32,
    head: &str,
    refusals: &[String],
    go_ignored: bool,
    now: u64,
) -> Window {
    let labels = match github.pr_labels(repo, number) {
        Ok(labels) => labels,
        Err(e) => {
            return Window::NotEligible(format!(
                "its labels could not be read: {}",
                first_line(&format!("{e:#}"))
            ));
        }
    };
    let start = match github.commit_statuses(repo, head) {
        Ok(statuses) => start(&statuses),
        Err(e) => Err(format!(
            "its head's statuses could not be read: {}",
            first_line(&format!("{e:#}"))
        )),
    };
    decide(&labels, start, refusals, go_ignored, now)
}

/// The label of `labels` named `name`, in any case, as GitHub names it.
pub fn find_label<'a>(labels: &'a [String], name: &str) -> Option<&'a str> {
    labels
        .iter()
        .find(|label| label.eq_ignore_ascii_case(name))
        .map(String::as_str)
}

fn has_label(labels: &[String], name: &str) -> bool {
    find_label(labels, name).is_some()
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}
