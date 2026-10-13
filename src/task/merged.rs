//! `task rm --merged` (issue 202, decision 195): finds every task whose work is merged, shows what
//! it would remove and why, and removes it after one confirmation. Never touches a task whose PR
//! is open or closed without merging, nor one whose result isn't safely elsewhere yet (decision
//! 178's protection).

use std::path::Path;

use anyhow::{Context, Result};

use super::finish::{self, RemovalPlan};
use super::record::{self, Kind, ProcessProbe, Record};
use crate::github::{GitHubBackend, PrState};

/// A task `task rm --merged` would remove, and why.
#[derive(Debug)]
pub struct Candidate {
    pub id: String,
    pub reason: String,
    pub plan: RemovalPlan,
}

/// A task considered but left alone, and why (decision 195 point 3: listed, not silently dropped,
/// when it is merged but protected, or when a spec task's merge can't be told from "not merged
/// yet").
#[derive(Debug)]
pub struct LeftAlone {
    pub id: String,
    pub reason: String,
}

/// What a `--merged` scan found. A task whose PR is open, or closed without merging, or that has
/// no PR at all yet, is excluded entirely: neither list names it.
#[derive(Debug, Default)]
pub struct MergedPlan {
    pub candidates: Vec<Candidate>,
    pub left_alone: Vec<LeftAlone>,
}

impl MergedPlan {
    /// Every task it would remove and why, then every task left alone and why: for the
    /// confirmation prompt and the dry run.
    pub fn listing(&self) -> String {
        let mut lines = Vec::new();
        for c in &self.candidates {
            lines.push(format!("task {}: {}", c.id, c.reason));
            for line in c.plan.listing().lines() {
                lines.push(format!("  {line}"));
            }
        }
        for l in &self.left_alone {
            lines.push(format!("left alone, task {}: {}", l.id, l.reason));
        }
        lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// [`finish::plan_removal`] for a task already known to be merged: a task that can't be planned
/// (most likely because it is running) is left alone instead of stopping the whole scan, with the
/// reason [`finish::plan_removal`] gave.
fn add_merged(
    base_dir: &Path,
    id: &str,
    reason: String,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) {
    match finish::plan_removal(base_dir, id, probe) {
        Ok(plan) => out.candidates.push(Candidate {
            id: id.to_owned(),
            reason,
            plan,
        }),
        Err(e) => out.left_alone.push(LeftAlone {
            id: id.to_owned(),
            reason: format!("{e:#}"),
        }),
    }
}

/// An issue task (decision 195 point 1): no PR yet isn't a candidate at all; a merged PR is;
/// anything else (open, or closed without merging) is left alone silently, like `task rm` leaves
/// it.
fn check_issue(
    base_dir: &Path,
    task: &Record,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) -> Result<()> {
    let Some(url) = &task.pr else {
        return Ok(());
    };
    let Some(number) = finish::pr_number(url) else {
        out.left_alone.push(LeftAlone {
            id: task.id.clone(),
            reason: format!("its recorded PR {url:?} doesn't end in a PR number"),
        });
        return Ok(());
    };
    let info = github.pr(&task.repo, number).with_context(|| {
        format!(
            "task {}: cannot read PR #{number} of {}",
            task.id, task.repo
        )
    })?;
    if info.state != PrState::Merged {
        return Ok(());
    }
    add_merged(
        base_dir,
        &task.id,
        format!("PR #{number} is merged"),
        probe,
        out,
    );
    Ok(())
}

/// A PR-review task (decision 195 point 1): its own number is the PR's.
fn check_pr_review(
    _base_dir: &Path,
    _task: &Record,
    _github: &dyn GitHubBackend,
    _probe: &dyn ProcessProbe,
    _out: &mut MergedPlan,
) -> Result<()> {
    Ok(())
}

/// A spec task (decision 195 point 1).
fn check_spec(
    _base_dir: &Path,
    _task: &Record,
    _repo_root: &Path,
    _probe: &dyn ProcessProbe,
    _out: &mut MergedPlan,
) -> Result<()> {
    Ok(())
}

/// Scans every task under `base_dir` and works out which are merged (decision 195). Nothing is
/// removed; the caller shows [`MergedPlan::listing`], confirms, then calls [`remove`].
pub fn plan(
    base_dir: &Path,
    repo_root: &Path,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<MergedPlan> {
    let mut out = MergedPlan::default();
    for task in record::load_all(base_dir)? {
        match task.kind {
            Kind::Issue => check_issue(base_dir, &task, github, probe, &mut out)?,
            Kind::Pr => check_pr_review(base_dir, &task, github, probe, &mut out)?,
            Kind::Spec => check_spec(base_dir, &task, repo_root, probe, &mut out)?,
        }
    }
    Ok(out)
}
