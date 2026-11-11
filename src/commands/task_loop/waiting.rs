//! The loop's step that re-checks the PRs waiting to be armed (issue 550, decisions 316(3) and
//! 316(4)), between tasks only, after the checks of the runs (answer 1): a medium-risk PR is
//! armed once its veto window ends or it is labelled `go`, unless labelled `hold` (which also
//! disarms one already armed), the loop is paused (on a red `main`, or medium only by
//! `--pause-medium`), or the `medium-trial` issue holds its 10 merges; a PR of any risk held only
//! for its real check is armed once `sbxm/real-check` = `success` is on its head (#547 answer 3),
//! under every other rule of [`Loop::arm`]. A medium PR this run armed that merges gets a merge
//! comment on the trial's issue; a red `main` after one gets a break comment.

use std::io::Write;

use anyhow::Result;

use super::{Loop, medium_source, not_armed, not_run_reason, real_check_of};
use crate::github::PrState;
use crate::task::queue::{self, Row};
use crate::task::record::{Record, Stage};
use crate::task::risk::Level;
use crate::task::trial::{self, Armed, How, Merge, Trial};
use crate::task::window::{self, Window, Zone};
use crate::task::{loop_pause, medium, pr_fix};

/// Where a medium-risk PR stands, besides its own task's checks.
pub(super) enum Medium {
    /// `--pause-medium` set, or its file can't be read: why.
    Paused(String),
    /// The trial's issue holds its 10 merges.
    TrialFull,
    /// The trial's count can't be read: why.
    TrialUnreadable(String),
    /// Its window, and when it started.
    Window(Window, Option<u64>),
}

impl Medium {
    /// What keeps the PR unarmed, said after `risk medium; `; `None` when it is to be armed now.
    pub(super) fn reason(&self, zone: &Zone) -> Option<String> {
        Some(match self {
            Self::Paused(why) => {
                format!("medium auto-merge is paused ({why}) until `sbxm task loop --clear-pause`")
            }
            Self::TrialFull => format!(
                "the medium trial has its {} auto-merges, so medium PRs are yours to merge until \
                 the retro decides",
                trial::SIZE
            ),
            Self::TrialUnreadable(why) => why.clone(),
            Self::Window(Window::Held, _) => format!("labelled `{}`", window::HOLD_LABEL),
            Self::Window(Window::Refused(reasons), _) => reasons.join("; "),
            Self::Window(Window::NotEligible(why), _) => format!("no veto window: {why}"),
            Self::Window(Window::Waiting { until }, _) => format!(
                "its veto window ends at {}; auto-merge is armed then unless labelled `{}`",
                window::time_text(*until, zone),
                window::HOLD_LABEL
            ),
            Self::Window(Window::Go | Window::Over, _) => return None,
        })
    }
}

/// The zone the window's times are shown in.
// sbxm has no time-zone source, so times are UTC (declared in issue 550's result).
pub(super) fn zone() -> Zone {
    Zone {
        offset_secs: 0,
        abbreviation: "UTC".to_owned(),
    }
}

/// A task record that passes [`Loop::arm`]'s checks besides its risk: no work stop, a review
/// with 0 must-fix and no check left not run.
fn clean(rec: &Record) -> bool {
    rec.stopped.is_none()
        && rec
            .review
            .as_ref()
            .is_some_and(|r| r.must_fix == 0 && not_run_reason(r).is_none())
}

impl Loop<'_> {
    /// Where medium-risk PR `number` at `head`, from `source`, stands: the medium pause, then the
    /// trial's count, then its window ([`window::decide`]: `hold`, the other rules of
    /// [`medium::refusals`], the review of its head, `go`, the time). Reads only.
    pub(super) fn medium_verdict(
        &mut self,
        number: u32,
        head: &str,
        source: medium::Source,
        warn: &mut dyn Write,
    ) -> Result<Medium> {
        match loop_pause::read_medium(&self.base_dir) {
            loop_pause::MediumState::None => {}
            loop_pause::MediumState::Set(pause) => return Ok(Medium::Paused(pause.why)),
            loop_pause::MediumState::Unreadable(why) => {
                self.warn_once(
                    warn,
                    format!(
                        "{why}; counted as a medium pause: no medium PR is armed until you run \
                         `sbxm task loop --clear-pause`"
                    ),
                )?;
                return Ok(Medium::Paused(why));
            }
        }
        match trial::read(self.github, &self.repo) {
            Err(why) => return Ok(Medium::TrialUnreadable(why)),
            Ok(Trial::NoIssue) => self.warn_once(
                warn,
                format!(
                    "no open issue is labelled {}, so the medium auto-merges aren't recorded or \
                     counted; they are armed all the same",
                    trial::LABEL
                ),
            )?,
            Ok(found) if found.full() => return Ok(Medium::TrialFull),
            Ok(_) => {}
        }
        let refusals = medium::refusals(self.github, &self.repo, source, number);
        let labels = match self.github.pr_labels(&self.repo, number) {
            Ok(labels) => labels,
            Err(e) => {
                let why = format!(
                    "its labels could not be read: {}",
                    super::first_line(&format!("{e:#}"))
                );
                return Ok(Medium::Window(Window::NotEligible(why), None));
            }
        };
        let start = match self.github.commit_statuses(&self.repo, head) {
            Ok(statuses) => window::start(&statuses),
            Err(e) => Err(format!(
                "its head's statuses could not be read: {}",
                super::first_line(&format!("{e:#}"))
            )),
        };
        let go_ignored = self.go_unremoved.contains(&number);
        let decided = window::decide(&labels, start.clone(), &refusals, go_ignored, self.now());
        Ok(Medium::Window(decided, start.ok()))
    }

    /// Acts on `verdict` for medium-risk PR `number` at `head` (the commit its clean review
    /// covered), whose URL is `url`: armed on `head` when its window says so, its window
    /// announced while it waits, said once otherwise. Returns the log row's note.
    pub(super) fn act_on_medium(
        &mut self,
        number: u32,
        head: &str,
        url: &str,
        verdict: Medium,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        let zone = zone();
        let Some(reason) = verdict.reason(&zone) else {
            let Medium::Window(window, start) = verdict else {
                unreachable!("only a window arms");
            };
            return self.arm_medium(number, head, &window, start, out, warn);
        };
        if let Medium::Window(window @ Window::Waiting { .. }, _) = &verdict {
            let github = self.github;
            let repo = self.repo.clone();
            if let Some((notifier, shown)) = self.notify.as_mut() {
                window::Notice {
                    github,
                    repo: &repo,
                    notifier: *notifier,
                    zone: &zone,
                }
                .announce(
                    shown,
                    &window::Pr { number, url, head },
                    window,
                    out,
                    warn,
                )?;
            }
        }
        self.say_once(
            out,
            format!(
                "{pr_named} awaits you: risk medium; {reason}",
                pr_named = self.pr_named(number)
            ),
        )?;
        Ok(Some(not_armed("risk medium", Some(&reason))))
    }

    /// Arms medium-risk PR `number` on `head`, as `window` says (`go` or its end), and keeps
    /// what the trial records of it until it merges.
    fn arm_medium(
        &mut self,
        number: u32,
        head: &str,
        window: &Window,
        start: Option<u64>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        let now = self.now();
        let (how, since) = match window {
            Window::Go => {
                let labelled = match self.github.label_events(&self.repo, number) {
                    Ok(events) => trial::go_at(&events),
                    Err(e) => {
                        self.warn_once(
                            warn,
                            format!(
                                "{pr_named}'s label history can't be read ({e:#}); its arming is \
                                 recorded as 0 min after `go`",
                                pr_named = self.pr_named(number)
                            ),
                        )?;
                        None
                    }
                };
                (How::Go, labelled.unwrap_or(now))
            }
            _ => (
                How::WindowEnd,
                start.map_or(now, |s| s.saturating_add(window::WINDOW_SECS)),
            ),
        };
        let armed = Armed {
            how,
            late_min: trial::minutes(since, now),
            review_at: start.unwrap_or(now),
        };
        let by = match how {
            How::Go => format!("labelled `{}`", window::GO_LABEL),
            How::WindowEnd => "its veto window ended".to_owned(),
        };
        let note = self.arm_head(number, head, &format!("medium, {by}"), out, warn)?;
        if self.armed.contains(&number) {
            self.medium_armed.insert(number, armed);
        }
        Ok(note)
    }

    /// Arms PR `number` on `head` for the reason `why` (said in brackets): the line and a log
    /// row. Returns the note.
    fn arm_head(
        &mut self,
        number: u32,
        head: &str,
        why: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        if let Err(e) = self.github.pr_arm_auto_merge(&self.repo, number, head) {
            return self.arming_failed(number, &e, out, warn);
        }
        self.armed.insert(number);
        self.armed_heads.insert((number, head.to_owned()));
        writeln!(
            out,
            "{pr_named}: auto-merge armed on {head} ({why})",
            pr_named = self.pr_named(number)
        )?;
        Ok(Some(format!("auto-merge armed ({why})")))
    }

    /// The step itself: every open PR a loop opened, in number order. A PR that can't be read is
    /// left for the next step.
    pub(super) fn recheck_waiting(
        &mut self,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let mut prs: Vec<(u32, String, String)> = queue::loop_prs(&self.base_dir, &self.repo)?
            .into_iter()
            .filter_map(|pr| Some((pr.number?, pr.task, pr.url)))
            .collect();
        prs.sort();
        prs.dedup_by_key(|(number, ..)| *number);
        for (number, task, url) in prs {
            if self.unresolved.contains(&number) {
                continue;
            }
            let Ok(state) = self.github.pr_merge_state(&self.repo, number) else {
                continue;
            };
            if state.state != PrState::Open || state.draft {
                continue;
            }
            let Some(own) = self.record_of(&task) else {
                continue;
            };
            let history = self.history_risk(number, Some(&own));
            if state.auto_merge {
                if history == Some(Level::Medium) {
                    self.disarm_held(number, out, warn)?;
                }
                continue;
            }
            let Ok(info) = self.github.pr(&self.repo, number) else {
                continue;
            };
            let head = info.head_oid;
            let key = (number, head.clone());
            if self.armed_heads.contains(&key) || self.ci_left.contains(&key) {
                continue;
            }
            self.recheck(number, &own, history, &head, &url, out, warn)?;
        }
        Ok(())
    }

    /// One unarmed PR of the step: its own task `own`, the highest risk of its review history
    /// `history`, its head `head`.
    #[allow(clippy::too_many_arguments)]
    fn recheck(
        &mut self,
        number: u32,
        own: &Record,
        history: Option<Level>,
        head: &str,
        url: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let fixes = self.fixes_of(number);
        // The record whose review covered the head: a fix pushed onto it, else its own task.
        let covering = fixes
            .iter()
            .filter(|fix| fix.stage == Stage::Finished)
            .chain(std::iter::once(own))
            .find(|rec| {
                rec.review
                    .as_ref()
                    .is_some_and(|r| r.commit.as_deref() == Some(head))
            });
        let Some(rec) = covering else {
            if history == Some(Level::Medium) {
                self.say_once(
                    out,
                    format!(
                        "{pr_named} awaits you: risk medium; its head {head} moved after its \
                         clean review, so it has no veto window",
                        pr_named = self.pr_named(number)
                    ),
                )?;
            }
            return Ok(());
        };
        let Some(review) = rec.review.as_ref() else {
            return Ok(());
        };
        let risk = history
            .into_iter()
            .chain([review.risk])
            .max_by_key(|&risk| super::awaiting_rank(risk))
            .unwrap_or(review.risk);
        if !clean(rec) || !clean(own) || !self.fixes_clean(&fixes) {
            return Ok(());
        }
        match risk {
            // Arming later (#547 answer 3): only a PR its real check held.
            Level::Low if matches!(real_check_of(&self.base_dir, Some(own)), Ok(Some(_))) => {}
            Level::Medium => {}
            _ => return Ok(()),
        }
        if self
            .real_check_refusal(Some(own), number, head, warn)?
            .is_some()
        {
            return Ok(());
        }
        // The pass over the runs says the pause once.
        if self.paused(warn)? {
            return Ok(());
        }
        let note = if risk == Level::Low {
            self.arm_head(number, head, "its real check passed", out, warn)?
        } else {
            let verdict = self.medium_verdict(number, head, medium_source(own), warn)?;
            let arms = matches!(verdict, Medium::Window(Window::Go | Window::Over, _));
            let note = self.act_on_medium(number, head, url, verdict, out, warn)?;
            if !arms {
                return Ok(());
            }
            note
        };
        if let Some(note) = note {
            self.arm_row(number, note, warn)?;
        }
        Ok(())
    }

    /// Whether every finished fix of the PR in `fixes` passed the checks its own task must, a
    /// log-fix also ending clean (review M-2 of decision 300's issue).
    fn fixes_clean(&self, fixes: &[Record]) -> bool {
        fixes.iter().all(|fix| {
            if fix.stage != Stage::Finished {
                return true;
            }
            let dir = crate::task::record::task_dir(&self.base_dir, &fix.id);
            let log_fix_ok = !dir.join(pr_fix::CHECK_LOG).is_file()
                || super::log_fix_left(&dir, Some(fix)).is_none();
            log_fix_ok && clean(fix)
        })
    }

    /// A log row for an arming of the step.
    fn arm_row(&mut self, number: u32, note: String, warn: &mut dyn Write) -> Result<()> {
        let row = Row {
            issue: number,
            title: self.pr_row(number, "waiting PR checked again".to_owned()),
            size: None,
            started: self.now(),
            ready: None,
            worker_s: None,
            reviewer_s: None,
            rounds: None,
            must_fix: None,
            outcome: note,
            stops: Vec::new(),
            idle_before: None,
        };
        self.log_row(&row, false, warn)
    }

    /// Disarms armed medium-risk PR `number` when it is labelled `hold` (decision 316(3)).
    fn disarm_held(
        &mut self,
        number: u32,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let Ok(labels) = self.github.pr_labels(&self.repo, number) else {
            return Ok(());
        };
        if window::find_label(&labels, window::HOLD_LABEL).is_none() {
            return Ok(());
        }
        match self.github.pr_disarm_auto_merge(&self.repo, number) {
            Ok(()) => {
                self.armed.remove(&number);
                self.medium_armed.remove(&number);
                writeln!(
                    out,
                    "{pr_named}: auto-merge disarmed: labelled `{}`",
                    window::HOLD_LABEL,
                    pr_named = self.pr_named(number)
                )?;
                self.arm_row(
                    number,
                    format!("auto-merge disarmed: labelled `{}`", window::HOLD_LABEL),
                    warn,
                )
            }
            Err(e) => self.warn_once(
                warn,
                format!(
                    "auto-merge could not be disarmed on {pr_named}, labelled `{}` ({e:#}); \
                     tried again at the next step",
                    window::HOLD_LABEL,
                    pr_named = self.pr_named(number)
                ),
            ),
        }
    }

    /// Posts the trial's merge comment for medium-risk PR `number`, which this run armed as
    /// `armed` and GitHub merged (answer 2). Anything that can't be read or posted is a warning.
    pub(super) fn record_trial_merge(
        &mut self,
        number: u32,
        armed: Armed,
        warn: &mut dyn Write,
    ) -> Result<()> {
        self.medium_merged.insert(number);
        let holds = match self.github.label_events(&self.repo, number) {
            Ok(events) => trial::holds(&events),
            Err(e) => {
                self.warn_once(
                    warn,
                    format!(
                        "{pr_named}'s label history can't be read ({e:#}); its trial comment \
                         counts 0 holds",
                        pr_named = self.pr_named(number)
                    ),
                )?;
                0
            }
        };
        let merged_at = match self.github.pr_merged_at(&self.repo, number) {
            Ok(Some(at)) => at,
            _ => self.now(),
        };
        let body = trial::merge_comment(&Merge {
            pr: number,
            armed,
            holds,
            merged_at,
        });
        self.post_trial(&body, &format!("the merge of PR #{number}"), warn)
    }

    /// Posts the trial's break comment for a red `main` after the medium auto-merge of PR
    /// `number` (answer 3), `what` naming the red run.
    pub(super) fn record_trial_break(
        &mut self,
        number: u32,
        what: &str,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let body = trial::break_comment(&format!(
            "main went red after the medium auto-merge of PR #{number}: {what}"
        ));
        self.post_trial(&body, &format!("the break after PR #{number}"), warn)
    }

    fn post_trial(&mut self, body: &str, what: &str, warn: &mut dyn Write) -> Result<()> {
        match trial::post(self.github, &self.repo, body) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => self.warn_once(
                warn,
                format!(
                    "no open issue is labelled {}, so {what} isn't recorded there",
                    trial::LABEL
                ),
            ),
            Err(e) => self.warn_once(
                warn,
                format!("{what} could not be recorded on the medium trial's issue: {e:#}"),
            ),
        }
    }
}
