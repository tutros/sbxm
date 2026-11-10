//! The veto window before `task loop` arms auto-merge on a medium-risk PR (issue 548, decision
//! 316(3), #460 guardrail 5): two hours from the PR head's latest `sbxm/review` status, when that
//! status is a success. A `hold` label keeps the PR unarmed, a `go` label arms it at once, neither
//! arms it when the window ends; labels match case-insensitively, as GitHub's do. `go` only skips
//! the wait: the other arming rules ([`crate::task::medium`], the not-run list...) come first.
//! The window's start is read from GitHub each time, so no state of sbxm's holds it. Nothing
//! calls this before #550, which arms by it; low risk never comes here (decisions 266, 280).

use std::io::Write;

use anyhow::Result;

use crate::github::{GitHubBackend, PostedStatus};
use crate::notify::{Notification, Notifier, Shown};
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

/// A time zone to show times in: its offset from UTC and its abbreviation (`PST`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    /// Seconds east of UTC: `-28800` for PST.
    pub offset_secs: i64,
    pub abbreviation: String,
}

/// `at` in `zone` on a 12-hour clock with the zone's abbreviation, then in UTC on a 24-hour one:
/// `11:40 am PST (19:40 UTC)`. Seconds are left out.
pub fn time_text(at: u64, zone: &Zone) -> String {
    let minute_of_day = |secs: i64| secs.rem_euclid(86_400) / 60;
    let local = minute_of_day(at as i64 + zone.offset_secs);
    let (hour, minute) = (local / 60, local % 60);
    let half = if hour < 12 { "am" } else { "pm" };
    let hour12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    let utc = minute_of_day(at as i64);
    format!(
        "{hour12}:{minute:02} {half} {} ({:02}:{:02} UTC)",
        zone.abbreviation,
        utc / 60,
        utc % 60
    )
}

/// The key a window's notice is shown once under, in `notified.json`: one per PR head.
pub fn notice_key(number: u32, head: &str) -> String {
    format!("window:{number}:{head}")
}

/// The PR a notice is about.
pub struct Pr<'a> {
    pub number: u32,
    /// What the toast's click opens.
    pub url: &'a str,
    /// The PR's head commit, whose review started the window.
    pub head: &'a str,
}

/// What a window's notice goes through (answer 4): a PR comment on `github` and a toast on
/// `notifier`, with times in `zone`.
pub struct Notice<'a> {
    pub github: &'a dyn GitHubBackend,
    pub repo: &'a str,
    pub notifier: &'a dyn Notifier,
    pub zone: &'a Zone,
}

impl Notice<'_> {
    /// Announces `pr`'s window once per head, when `window` has it waiting: a PR comment saying
    /// when it will be armed, then a toast whose click opens the PR, then the key recorded in
    /// `shown`. A PR held, refused, not eligible or to be armed now gets none. A comment that
    /// fails to post, a toast that fails to show or a key that can't be written is a warning on
    /// `warn`: the window runs all the same. No comment is edited later.
    pub fn announce(
        &self,
        shown: &mut Shown,
        pr: &Pr,
        window: &Window,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let Window::Waiting { until } = *window else {
            return Ok(());
        };
        let key = notice_key(pr.number, pr.head);
        if shown.contains(&key) {
            return Ok(());
        }
        let at = time_text(until, self.zone);
        let comment = format!(
            "Medium risk: auto-merge will be armed at {at} unless labelled `{HOLD_LABEL}`. \
             Label `{GO_LABEL}` to arm now."
        );
        if let Err(e) = self.github.pr_comment(self.repo, pr.number, &comment) {
            writeln!(
                warn,
                "warning: the window's comment on PR #{} could not be posted: {}; it is armed at \
                 {at} all the same",
                pr.number,
                first_line(&format!("{e:#}"))
            )?;
        }
        let local = at.split(" (").next().unwrap_or(&at);
        let note = Notification {
            key: key.clone(),
            title: format!("PR #{} arms at {local} (medium)", pr.number),
            body: format!(
                "Medium risk, reviewed clean: auto-merge is armed then; label it {HOLD_LABEL} to \
                 keep it unarmed, or {GO_LABEL} to arm it now"
            ),
            click: pr.url.to_owned(),
        };
        if let Err(e) = self.notifier.show(&note, out) {
            writeln!(
                warn,
                "warning: the notification \"{}\" could not be shown: {e:#}; the loop goes on",
                note.title
            )?;
        }
        if let Err(e) = shown.record(&key) {
            writeln!(
                warn,
                "warning: {e:#}; the window's notice of PR #{} may be posted again after a restart",
                pr.number
            )?;
        }
        Ok(())
    }
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
