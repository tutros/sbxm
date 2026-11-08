//! `sbxm task rm (--issue N | --pr N | --spec FILE | --merged) [--yes] [--force] [--dry-run]`
//! (spec §4, decision 153): removes a task's sandboxes, clones and folder after showing exactly
//! what, and asking. `--merged` (decision 195, issue 202) removes every task whose work is merged
//! instead of naming one. `--pr N` removes PR N's review task and every fix task of it (issue
//! 485).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::disk;
use crate::github::GitHubBackend;
use crate::task::finish;
use crate::task::merged;
use crate::task::record::{self, ProcessProbe};

/// Which task to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    Pr(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
    /// Every task whose work is merged (decision 195, issue 202).
    Merged,
}

pub struct Options {
    pub target: Target,
    /// Skip the question (needed without a terminal).
    pub yes: bool,
    /// Delete a spec task's result even though it exists only in its `repo.git` (issue 143), or
    /// a fix task's commits that were never pushed to its PR's branch (issue 485).
    /// Refused together with `Target::Merged` (decision 195 point 3).
    pub force: bool,
    /// The checkout a spec task's result is fetched into.
    pub repo_root: PathBuf,
    /// With `Target::Merged`: list what would be removed and why, and remove nothing.
    pub dry_run: bool,
    /// `--forget-ambiguous` (decision 268): drop a cloud create that was started and never
    /// recorded as returned from the record, touching nothing in the cloud for it, then go on.
    pub forget_ambiguous: bool,
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<()> {
    run_with_sleep(
        config_dir,
        opts,
        backend,
        github,
        probe,
        confirm,
        out,
        &std::thread::sleep,
    )
}

/// [`run`], with `sleep` called for each wait between a cloud sandbox's removal retries (P6 (a))
/// instead of really sleeping: a seam for testing them, as `task_start::run_spec_with_sleep` is.
/// A task's sandboxes are removed where its `task.json` says they are (decision 253).
#[allow(clippy::too_many_arguments)]
pub fn run_with_sleep(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(std::time::Duration),
) -> Result<()> {
    if opts.dry_run && opts.target != Target::Merged {
        bail!("--dry-run only makes sense with --merged");
    }
    let config = GlobalConfig::load(config_dir)?;
    let base_dir = config.base_dir.clone();
    let id = match &opts.target {
        Target::Issue(n) => format!("issue-{n}"),
        Target::Pr(n) => {
            let review = format!("pr-{n}");
            let fixes = finish::fix_tasks_of(&base_dir, *n)?;
            if fixes.is_empty() && !finish::exists(&base_dir, &review) {
                bail!(
                    "no task {review} or fix-{n}-<head> (a review or a fix of PR #{n}); run `sbxm \
                     task status` to list the tasks"
                );
            }
            if !fixes.is_empty() {
                return remove_pr_tasks(
                    &base_dir, &review, fixes, opts, backend, probe, confirm, out, sleep,
                );
            }
            review
        }
        Target::Spec(path) => {
            let id = record::spec_id(path)?.0;
            if !finish::exists(&base_dir, &id) {
                bail!("no task {id}; run `sbxm task status` to list the tasks");
            }
            id
        }
        Target::Merged => {
            let found = merged::plan_forgetting(&base_dir, github, probe, opts.forget_ambiguous)?;
            if opts.dry_run {
                if found.is_empty() && found.left_alone.is_empty() {
                    writeln!(out, "no merged tasks to remove")?;
                } else {
                    writeln!(out, "{}", found.listing())?;
                }
                return Ok(());
            }
            let state_dir = disk::state_dir(&config, disk::platform_state_dir().as_deref());
            return merged::remove_with_sleep(
                &base_dir,
                &found,
                opts.yes,
                backend,
                confirm,
                |p| fs4::available_space(p),
                state_dir.as_deref(),
                out,
                sleep,
            );
        }
    };
    finish::discard(
        &base_dir,
        &id,
        opts.yes,
        opts.forget_ambiguous,
        backend,
        probe,
        confirm,
        out,
        sleep,
        // Under the task's lock (issue 58): its record and `repo.git` can't change after this.
        &|| match (&opts.target, opts.force) {
            (Target::Spec(_), false) => {
                finish::check_spec_result_fetched(&base_dir, &id, &opts.repo_root)
            }
            _ => Ok(()),
        },
    )
}

/// `task rm --pr N` when PR N has fix tasks (issue 485): its review task, when there is one, and
/// every fix task, whatever head each was made for, listed with one question. A fix task whose
/// commits were never pushed is kept and named, unless `--force`; the others still go.
#[allow(clippy::too_many_arguments)]
fn remove_pr_tasks(
    base_dir: &Path,
    review: &str,
    fixes: Vec<String>,
    opts: &Options,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
    sleep: &dyn Fn(std::time::Duration),
) -> Result<()> {
    let ids: Vec<String> = finish::exists(base_dir, review)
        .then(|| review.to_owned())
        .into_iter()
        .chain(fixes)
        .collect();
    finish::discard_each(
        base_dir,
        &ids,
        opts.yes,
        opts.forget_ambiguous,
        backend,
        probe,
        confirm,
        out,
        sleep,
        // Under each task's lock (issue 58): its record and `repo.git` can't change after this.
        &|id| {
            if id == review || opts.force {
                Ok(())
            } else {
                finish::check_fix_result_pushed(base_dir, id)
            }
        },
    )
}
