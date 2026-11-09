//! `sbxm task loop --check [--issue N]...` (issue 511, decision 307): for each issue, whether the
//! loop would take it or pass it over and why, from the loop's own checks before any write plus
//! the label, the size and a branch of the task's name already on origin; then the exact `task
//! loop` command for the ones it would take. Reads only: no sandbox, no lock, no GitHub write, no
//! log row.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use super::{OPEN_POINTS, Options, refused_reason, unmerged_dependencies, waiting_reason};
use crate::config::GlobalConfig;
use crate::github::{GitHubBackend, Issue};
use crate::status::looping::{self, Lock};
use crate::task::config::TaskConfig;
use crate::task::finish::check_own_folder;
use crate::task::queue::{self, READY_LABEL};
use crate::task::record::{self, ProcessProbe};
use crate::task::{findings, pipeline};

use super::super::task_start::resolve_repo;

/// Prints a verdict per issue (`opts.issues`, or the labeled queue in the loop's order without
/// them) and the loop command for those it would take. `Ok(true)` when every issue would be
/// taken (an empty queue too), `Ok(false)` otherwise. A running loop is named on the first line,
/// and the command then says a second loop waits for it.
pub fn check(
    config_dir: &Path,
    opts: &Options,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<bool> {
    if opts.issues.contains(&0) {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    // What the loop itself would refuse on before its first issue.
    TaskConfig::load(&opts.repo_root)?;
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let tasks = record::tasks_root(&base_dir);
    if fs::symlink_metadata(&tasks).is_ok() {
        check_own_folder(&base_dir, &tasks)?;
    }

    let running = running_loop(&base_dir, probe);
    if let Some((line, _)) = &running {
        writeln!(out, "{line}")?;
    }
    let open = github.issues_open(&repo)?;
    let issues = if opts.issues.is_empty() {
        queue::queue_order(&open)
    } else {
        // The loop tries an issue named twice once, at its first place.
        let mut named = Vec::new();
        for &n in &opts.issues {
            if !named.contains(&n) {
                named.push(n);
            }
        }
        named
    };
    if issues.is_empty() {
        writeln!(
            out,
            "the queue is empty: no open issue is labeled {READY_LABEL}; nothing to run"
        )?;
        return Ok(true);
    }

    let mut taken = Vec::new();
    for &number in &issues {
        let Some(issue) = open.iter().find(|i| i.number == number) else {
            writeln!(out, "#{number}: skip: isn't an open issue")?;
            continue;
        };
        let reasons = reasons(config_dir, &repo, github, &open, issue)?;
        if reasons.is_empty() {
            writeln!(out, "#{number} {}: take", issue.title)?;
            taken.push(number);
        } else {
            writeln!(
                out,
                "#{number} {}: skip: {}",
                issue.title,
                reasons.join("; also: ")
            )?;
        }
    }

    if taken.is_empty() {
        writeln!(out, "command: none, no issue would be taken")?;
    } else {
        let list: Vec<String> = taken.iter().map(|n| format!("--issue {n}")).collect();
        writeln!(out, "command: sbxm task loop {}", list.join(" "))?;
        if let Some((_, refusal)) = &running {
            writeln!(out, "  {refusal}")?;
        }
    }
    Ok(taken.len() == issues.len())
}

/// Every reason the loop would pass `issue` over, or none: the label and the size, the open
/// points, then the loop's own checks in its order (an unmerged dependency, else what `task run`
/// would refuse before any write), then a branch of the task's name already on origin, which
/// `task finish` would refuse to push over.
pub(super) fn reasons(
    config_dir: &Path,
    repo: &str,
    github: &dyn GitHubBackend,
    open: &[Issue],
    issue: &Issue,
) -> Result<Vec<String>> {
    let number = issue.number;
    let mut reasons = Vec::new();
    if !issue.labels.iter().any(|l| l == READY_LABEL) {
        reasons.push(format!("not labeled {READY_LABEL}"));
    }
    if queue::size(&issue.labels).is_none() {
        reasons.push("no size label (size:S, size:M or size:L)".to_owned());
    }
    if queue::has_open_points(&issue.body) {
        reasons.push(OPEN_POINTS.to_owned());
    }
    let unmerged = unmerged_dependencies(github, repo, open, &issue.body);
    if unmerged.is_empty() {
        let problems = pipeline::preflight_issues(config_dir, repo, github, &[number])?;
        if let Some(reason) = problems.get(&number) {
            reasons.push(refused_reason(reason));
        }
    } else {
        reasons.push(waiting_reason(&unmerged));
    }
    // An issue that continues a PR (`PR: #n`) works on that PR's branch, not its own.
    let branch = format!("issue-{number}");
    if findings::pr_of(&issue.body).is_none() && github.branch_exists(repo, &branch)? {
        reasons.push(format!(
            "branch {branch} is already on origin, so `task finish` would refuse to push the \
             task's branch over it; delete or rename it on origin"
        ));
    }
    Ok(reasons)
}

/// A loop holding the lock, as `(first line, what it means for the printed command)`, from the
/// lock alone (never taken here); `None` when no loop runs, or a gone one left its lock, which a
/// new loop takes over.
fn running_loop(base_dir: &Path, probe: &dyn ProcessProbe) -> Option<(String, String)> {
    let (lock, file) = looping::read_lock_file(base_dir, probe);
    match lock {
        Lock::Absent | Lock::Stale(_) => None,
        Lock::Unreadable(why) => Some((
            format!("the loop lock can't be read ({why})"),
            "a loop refuses to start until that lock is deleted; delete it if no `sbxm task \
             loop` is running"
                .to_owned(),
        )),
        Lock::Running(process) => {
            let at = file
                .and_then(|file| looping::work_for(base_dir, &file).ok())
                .map_or_else(String::new, |work| match &work.listed {
                    Some(listed) => match listed.position() {
                        Some(n) => format!(", task {n} of {}", listed.total()),
                        None => format!(", between tasks, {} done", work.done.len()),
                    },
                    // A loop over the labeled queue doesn't know its total.
                    None => match work.current {
                        Some(_) => format!(
                            ", task {} of the labeled queue (its total is not known)",
                            work.done.len() + 1
                        ),
                        None => format!(", between tasks, {} done", work.done.len()),
                    },
                });
            Some((
                format!("a loop is running: process {}{at}", process.pid),
                format!(
                    "a second loop is refused until the running one (process {}) ends; run \
                     this after it stops",
                    process.pid
                ),
            ))
        }
    }
}
