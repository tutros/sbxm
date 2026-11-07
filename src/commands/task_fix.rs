//! `sbxm task fix --pr N` (issue 313): a task on an open PR's own branch whose worker fixes the
//! must-fix findings of the PR's latest sbxm review (or, with `--check-run`, a failed run of its head
//! from the run's log; decision 300), followed by the gates and the in-task review
//! of any task, and whose result is pushed onto the PR's branch as a fast-forward, with that
//! review posted as the new head's. Every refusal but the push's comes before any write.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};

use super::task_review::print_review;
use super::task_start::resolve_repo;
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::GitHubBackend;
use crate::headless::RunStatus;
use crate::task::config::TaskConfig;
use crate::task::finish;
use crate::task::gates::HostRunner;
use crate::task::lifetime;
use crate::task::pipeline::{self, Ctx};
use crate::task::pr_fix;
use crate::task::record::{self, ProcessProbe};
use crate::task::repo::{self, Identity};

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub pr: u32,
    /// `owner/name`; default: the `origin` of the checkout.
    pub repo: Option<String>,
    /// The task's base branch; default: the repo's default branch.
    pub base: Option<String>,
    /// For tests: what `repo.git` is cloned from instead of `https://github.com/<repo>.git`.
    pub clone_source: Option<String>,
    /// For tests: the committer identity of the worker's clone instead of the user's git config.
    pub identity: Option<Identity>,
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
    /// `--check-run <id>` (decision 300): the worker fixes this failed run of the PR's head from
    /// its log instead of the review's must-fix findings.
    pub check_run: Option<u64>,
}

fn print_released(out: &mut dyn Write, warn: &mut dyn Write, said: &[lifetime::Released]) {
    for said in said {
        let _ = match said {
            lifetime::Released::Line(line) => writeln!(out, "{line}"),
            lifetime::Released::Warning(warning) => writeln!(warn, "warning: {warning}"),
        };
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    config_dir: &std::path::Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let number = opts.pr;
    if number == 0 {
        bail!("PR numbers start at 1");
    }
    // Every input is checked before anything is created.
    let config = TaskConfig::load(&opts.repo_root)?;
    let repo_name = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    if let Some(base) = &opts.base
        && !repo::valid_ref_name(base)
    {
        bail!("base {base:?} isn't a usable branch name; pass --base <branch>");
    }
    for warning in &config.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let location = pipeline::choose_location(None, &config);
    let global = GlobalConfig::load(config_dir)?;
    if location == Location::Local {
        disk::require(&global, config_dir, &opts.disk)?;
    }
    let backend = backend.located(location);

    let pr = github.pr(&repo_name, number)?;
    pr_fix::check_fixable(&pr, &repo_name, opts.base.as_deref())?;
    let review = match opts.check_run {
        Some(id) => {
            let runs = github.commit_runs(&repo_name, &pr.head_oid)?;
            let run = pr_fix::failed_run(&pr, &repo_name, id, &runs)?;
            let jobs = github.run_failed_log(&repo_name, id)?;
            pr_fix::log_to_fix(&pr, &run, &jobs)
        }
        None => {
            let user = github.whoami()?;
            let comments = github.pr_comments(&repo_name, number)?;
            pr_fix::review_to_fix(&pr, &repo_name, opts.base.as_deref(), &user, &comments)?
        }
    };
    let base_branch = match &opts.base {
        Some(base) => base.clone(),
        None => github.default_branch(&repo_name)?,
    };
    if !repo::valid_ref_name(&base_branch) {
        bail!("base {base_branch:?} isn't a usable branch name; pass --base <branch>");
    }
    // The reviewer's secret and profile are checked now too: a missing one must not be found only
    // after the worker has spent its time (spec section 5.2).
    pipeline::check_reviewer_inputs(config_dir, backend, &config, location)?;
    let identity = match &opts.identity {
        Some(identity) => identity.clone(),
        None => Identity::read_from(None)?,
    };
    let clone_source = opts
        .clone_source
        .clone()
        .unwrap_or_else(|| format!("https://github.com/{repo_name}.git"));
    let ctx = Ctx {
        config_dir,
        repo: &repo_name,
        clone_source: &clone_source,
        base_branch: &base_branch,
        config: &config,
        backend,
        github,
        identity: &identity,
        probe,
        host,
        sleep: &std::thread::sleep,
        // Empty in production while P8 (a) holds (slice 4), as for any task.
        scoped_secrets: &[],
        disk: &opts.disk,
    };

    match opts.check_run {
        Some(id) => writeln!(
            out,
            "PR #{number}: fixing the failed run {id} of {} from its log",
            review.commit
        )?,
        None => writeln!(
            out,
            "PR #{number}: fixing {} must-fix finding(s) of the sbxm review of {}",
            review.must_fix, review.commit
        )?,
    }
    let prepared = pipeline::prepare_fix(&ctx, &pr, &review, location);
    let mut prepared = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            if let Some(after) = e.downcast_ref::<pipeline::FailedAfterCreate>() {
                for event in &after.events {
                    writeln!(out, "{event}")?;
                }
                if let Some(why) = &after.kept {
                    writeln!(warn, "warning: {why}")?;
                }
                for warning in &after.warnings {
                    writeln!(warn, "warning: {warning}")?;
                }
            }
            return Err(e);
        }
    };
    let id = prepared.record.id.clone();
    for warning in prepared
        .warnings
        .iter()
        .filter(|w| !config.warnings.contains(w))
    {
        writeln!(warn, "warning: {warning}")?;
    }
    for event in prepared.events.drain(..) {
        writeln!(out, "{event}")?;
    }

    let worked = pipeline::run_worker(&ctx, &mut prepared);
    let stops = match &worked {
        Ok(worked) => match &worked.status {
            RunStatus::Completed => false,
            // A timed-out worker's partial commits are reviewed, except in the cloud, where it
            // stops the task (milestone 4 slice 3, behavior 2).
            RunStatus::TimedOut => location == Location::Cloud,
            RunStatus::Failed(_) => true,
        },
        Err(_) => true,
    };
    if let Ok(worked) = &worked {
        let label = match &worked.status {
            RunStatus::Completed => "completed".to_owned(),
            RunStatus::TimedOut => "timed out".to_owned(),
            RunStatus::Failed(why) => format!("failed: {why}"),
        };
        writeln!(
            out,
            "{id}: worker {} {label}, {} commit(s)",
            worked.agent, worked.commits
        )?;
        for note in &worked.notes {
            writeln!(out, "  note: {note}")?;
        }
    }
    if stops {
        let said = lifetime::release_worker(
            backend,
            &prepared.meta,
            &mut prepared.record,
            &std::thread::sleep,
            &[],
        );
        let said: Vec<_> = said.all().cloned().collect();
        print_released(out, warn, &said);
    }
    // The worker's phase is over, whatever its outcome: a local worker sandbox goes now (issue
    // 341).
    let released = pipeline::release_local_worker(backend, &mut prepared);
    print_released(out, warn, &released);
    let worked = worked?;
    if stops {
        bail!(
            "the worker of {id} didn't finish, so nothing was pushed to PR #{number}; see `sbxm \
             task status` and {}",
            prepared.meta.join("result.md").display()
        );
    }
    if worked.commits == 0 {
        bail!(
            "the worker of {id} made no commits, so there is nothing to push to PR #{number}; \
             see {}",
            prepared.meta.join("result.md").display()
        );
    }
    // Decision 292: the worker merged the base first.
    pipeline::check_base_merged(&prepared)?;

    let env = ctx.env();
    let reviewed = pipeline::review_issue(&env, &mut prepared);
    for line in prepared.events.drain(..) {
        writeln!(out, "{line}")?;
    }
    for warning in prepared.warnings.drain(..) {
        writeln!(warn, "warning: {warning}")?;
    }
    let report = reviewed?;
    let meta = record::task_dir(&global.base_dir, &id);
    if let Some(failed) = print_review(out, warn, &config, &id, &meta, &report, &prepared.record)? {
        bail!(
            "gates failed for {id}: `{}`, so nothing was pushed to PR #{number}; the result is \
             kept in {} as {}",
            failed.command,
            meta.join("repo.git").display(),
            prepared.record.branch
        );
    }

    let pushed = finish::finish_fix(&global.base_dir, &id, github, probe)?;
    writeln!(
        out,
        "{id}: pushed {} commit(s) onto PR #{number}'s branch {}, now at {}: {}",
        pushed.commits.len(),
        pushed.branch,
        pushed.commit,
        pushed.url
    )?;
    if pushed.review_posted {
        writeln!(
            out,
            "{id}: the in-task review was posted on PR #{number} as the review of {}",
            pushed.commit
        )?;
    } else {
        writeln!(
            out,
            "{id}: no review covered {}; review it with `{}`",
            pushed.commit, pushed.review_again
        )?;
    }
    for warning in &pushed.warnings {
        writeln!(out, "warning: {warning}")?;
    }
    if !pushed.unresolved.is_empty() {
        writeln!(
            out,
            "  left: {}; once that review is posted, `sbxm task fix --pr {number} --repo {}` \
             fixes it",
            pushed.unresolved.join("; "),
            repo_name
        )?;
    }
    Ok(())
}
