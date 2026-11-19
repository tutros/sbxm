//! The hold before review (decision 331(3), issue 727): a loop task whose changed files, once its
//! gates pass, share a path with other open work waits at stage `held`, without a sandbox slot,
//! instead of going to its review. What it waits on is read here: an open PR a loop opened
//! (its files on GitHub), a running fix or resolve of a PR (that PR's files), or another task
//! already past its gates and not finished (its branch's changed files). Once nothing overlaps,
//! the loop releases it: it merges the default branch in, runs its gates again and gets its one
//! review ([`crate::task::pipeline::release_hold`]).
//!
//! Every hold is kept in the task's record ([`Hold`]), with when it started, what it waited on,
//! when it was released and whether the merge of the base had a conflict: `sbxm status`, the
//! loop's log row and `task facts` read them, so a restarted loop needs no memory of its own.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::finish::check_own_folder;
use super::queue;
use super::record::{self, Kind, ProcessProbe, Record, Stage, Status};
use crate::github::{GitHubBackend, PrState};
use crate::run::results::{format_timestamp, parse_timestamp};

/// What a held task waits on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Wait {
    /// An open PR a loop opened; `armed` when GitHub's auto-merge is armed on it, else it
    /// awaits the user's merge.
    Pr { number: u32, armed: bool },
    /// A running fix or resolve of PR `pr` (`task fix --pr`), the task `task`.
    Fix { task: String, pr: u32 },
    /// Another task past its gates and not finished, at `stage`.
    Task { task: String, stage: Stage },
}

impl Wait {
    /// How `sbxm status` and the loop name it: `#12 (PR, awaits your merge)`, `issue-9 (in
    /// review)`.
    pub fn describe(&self) -> String {
        match self {
            Self::Pr {
                number,
                armed: false,
            } => format!("#{number} (PR, awaits your merge)"),
            Self::Pr {
                number,
                armed: true,
            } => format!("#{number} (PR, auto-merge armed)"),
            Self::Fix { task, pr } => format!("{task} (fix of PR #{pr})"),
            Self::Task { task, stage } => {
                let what = match stage {
                    Stage::Fixing | Stage::Gating => "in a fix round",
                    Stage::Ready => "ready, not finished",
                    Stage::Held => "released, merging the base",
                    _ => "in review",
                };
                format!("{task} ({what})")
            }
        }
    }

    /// Whether it waits on the user (a PR awaiting their merge) rather than on a task.
    pub fn on_user(&self) -> bool {
        matches!(self, Self::Pr { armed: false, .. })
    }
}

/// `waits on A, B`: what a held task waits on, in one line.
pub fn waits_line(on: &[Wait]) -> String {
    let named: Vec<String> = on.iter().map(Wait::describe).collect();
    format!("waits on {}", named.join(", "))
}

/// One hold of a task, kept in its record (`holds`), the last one first in effect while the task
/// is `held`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// When the task was held.
    pub since: String,
    /// What it waited on, as the last check found it.
    pub on: Vec<Wait>,
    /// When the loop released it (nothing overlapped any more); `None` while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    /// Whether the merge of the default branch after the release had a conflict; `None` until
    /// it was looked at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<bool>,
}

impl Hold {
    /// The seconds it waited: to its release, or to `now` while it waits.
    pub fn seconds(&self, now: u64) -> u64 {
        let since = parse_timestamp(&self.since).unwrap_or(now);
        let until = self
            .released
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or(now);
        until.saturating_sub(since)
    }
}

/// The hold of a `held` task still waiting (not released); `None` for any other task.
pub fn waiting(task: &Record) -> Option<&Hold> {
    (task.stage == Stage::Held)
        .then(|| task.holds.last())
        .flatten()
        .filter(|h| h.released.is_none())
}

/// The minutes `task` was held, every hold of it, rounded up; `now` ends one still waiting.
pub fn minutes(task: &Record, now: u64) -> u64 {
    task.holds
        .iter()
        .map(|h| h.seconds(now))
        .sum::<u64>()
        .div_ceil(60)
}

/// Whether the merge of the base after any of `task`'s holds had a conflict: `None` when none
/// was looked at.
pub fn conflict(task: &Record) -> Option<bool> {
    task.holds
        .iter()
        .filter_map(|h| h.conflict)
        .reduce(|a, b| a || b)
}

/// Records that `task` is held now, waiting on `on`: a new hold.
pub fn start(task: &mut Record, now: u64, on: Vec<Wait>) {
    task.holds.push(Hold {
        since: format_timestamp(now),
        on,
        released: None,
        conflict: None,
    });
}

/// Records that the loop released `task`'s hold now; `false` when it has none waiting.
pub fn release(task: &mut Record, now: u64) -> bool {
    if waiting(task).is_none() {
        return false;
    }
    if let Some(hold) = task.holds.last_mut() {
        hold.released = Some(format_timestamp(now));
    }
    true
}

/// What a task's changed paths may overlap: one piece of other open work and its paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Other {
    pub wait: Wait,
    pub paths: Vec<String>,
}

/// What of `others` shares a path with `paths`, each once, in their order.
pub fn overlaps(paths: &[String], others: &[Other]) -> Vec<Wait> {
    let mut found: Vec<Wait> = Vec::new();
    for other in others {
        if other.paths.iter().any(|p| paths.contains(p)) && !found.contains(&other.wait) {
            found.push(other.wait.clone());
        }
    }
    found
}

/// What a task's overlap is checked against, asked once its gates pass (before its review) and
/// at each loop pass while it is held.
pub trait Check {
    /// What `task`, whose branch changes `paths`, waits on now; empty when nothing overlaps.
    fn waits(&self, task: &Record, paths: &[String]) -> Result<Vec<Wait>>;
}

/// [`Check`] against the task records under `base_dir` and the PRs of `repo` on GitHub.
pub struct Checker<'a> {
    pub base_dir: &'a Path,
    pub repo: &'a str,
    pub github: &'a dyn GitHubBackend,
    pub probe: &'a dyn ProcessProbe,
}

impl Check for Checker<'_> {
    fn waits(&self, task: &Record, paths: &[String]) -> Result<Vec<Wait>> {
        Ok(overlaps(paths, &self.others(task)?))
    }
}

impl Checker<'_> {
    /// Every piece of open work `task` may wait on, with its paths: the open PRs a loop opened,
    /// the running fixes of PRs, and the tasks past their gates ([`past_gates`]).
    pub fn others(&self, task: &Record) -> Result<Vec<Other>> {
        let own_pr = task.continues.as_ref().map(|c| c.pr);
        let mut others = Vec::new();
        let mut prs: Vec<u32> = Vec::new();
        for dir in record::task_folders(self.base_dir)? {
            if check_own_folder(self.base_dir, &dir).is_err() {
                continue;
            }
            let Ok(rec) = record::read(&dir.join("task.json")) else {
                continue;
            };
            if rec.id == task.id || rec.repo != self.repo {
                continue;
            }
            // A running fix or resolve of a PR: that PR's files.
            if rec.kind == Kind::Fix
                && rec.stage != Stage::Finished
                && rec.process_alive(self.probe)
            {
                if Some(rec.number) == own_pr || prs.contains(&rec.number) {
                    continue;
                }
                prs.push(rec.number);
                others.push(Other {
                    wait: Wait::Fix {
                        task: rec.id.clone(),
                        pr: rec.number,
                    },
                    paths: self.pr_paths(rec.number)?,
                });
                continue;
            }
            if past_gates(&rec, task, self.probe) {
                let Ok(paths) = rec.changed_paths(&dir.join("repo.git")) else {
                    continue;
                };
                others.push(Other {
                    wait: Wait::Task {
                        task: rec.id.clone(),
                        stage: rec.stage,
                    },
                    paths,
                });
            }
        }
        // The open PRs a loop opened, other than those a running fix already stands for.
        for pr in queue::loop_prs(self.base_dir, self.repo)? {
            let Some(number) = pr.number else {
                continue;
            };
            if Some(number) == own_pr || prs.contains(&number) || pr.task == task.id {
                continue;
            }
            let state = self.github.pr_merge_state(self.repo, number)?;
            if state.state != PrState::Open {
                continue;
            }
            prs.push(number);
            others.push(Other {
                wait: Wait::Pr {
                    number,
                    armed: state.auto_merge,
                },
                paths: self.pr_paths(number)?,
            });
        }
        Ok(others)
    }

    /// PR `number`'s files, each path once (a renamed file's old path too).
    fn pr_paths(&self, number: u32) -> Result<Vec<String>> {
        let files = self.github.pr_files(self.repo, number)?;
        let mut paths = Vec::new();
        for file in files.files {
            paths.push(file.filename);
            paths.extend(file.previous_filename);
        }
        Ok(paths)
    }
}

/// Whether `other` is a task past its gates and not finished, which `task` waits on when they
/// share a path: an issue or spec task with no PR yet that is in review, a fix round (or its
/// gates) or `ready`, or one a loop released from its hold (it merges the base, then goes to its
/// review). A task whose gates passed before its first review and whose process still runs is
/// one only when its gates started before `task`'s (then by id): of two tasks whose gates pass
/// at once, the first holds the second, and never both each other. A task still held waits
/// itself, and holds nothing.
pub fn past_gates(other: &Record, task: &Record, probe: &dyn ProcessProbe) -> bool {
    if !matches!(other.kind, Kind::Issue | Kind::Spec) || other.pr.is_some() || other.id == task.id
    {
        return false;
    }
    match other.stage {
        Stage::Reviewing | Stage::Fixing | Stage::Ready => true,
        Stage::Held => other.holds.last().is_some_and(|h| h.released.is_some()),
        Stage::Gating if other.review.is_some() || other.round > 0 => true,
        Stage::Gating => {
            other.status == Status::Passed
                && other.process_alive(probe)
                && (gates_started(other), &other.id) < (gates_started(task), &task.id)
        }
        Stage::Prepared | Stage::Working | Stage::Finished => false,
    }
}

/// When `task`'s latest gates started, in seconds; the latest time when it has none recorded.
fn gates_started(task: &Record) -> u64 {
    task.stages
        .iter()
        .rev()
        .find(|s| s.stage == Stage::Gating)
        .and_then(|s| parse_timestamp(&s.at))
        .unwrap_or(u64::MAX)
}
