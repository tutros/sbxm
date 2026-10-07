//! `sbxm task review (--issue N | --pr N)` (spec §4, §5.2). An issue's task: gates, an independent
//! reviewer in its own sandbox, at most one fix round, then `ready`; findings left over are
//! reported, not an error. A pull request: gates on a clean checkout of its head, one reviewer,
//! and the review posted as a PR comment.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use super::task_start::resolve_repo;
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::run::config::{check_model_flag, headless_harness, parse_duration};
use crate::task::config::TaskConfig;
use crate::task::finish;
use crate::task::gates::HostRunner;
use crate::task::pipeline::{self, Ctx, Prepared, TaskEnv};
use crate::task::record::{self, GateResult, ProcessProbe};
use crate::task::repo::{self, Identity};

/// What is reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    Pr(u32),
    /// A spec task's file (decision 174(d), issue 142).
    Spec(PathBuf),
}

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub target: Target,
    /// `owner/name`; default: the `origin` of the checkout (pull requests only).
    pub repo: Option<String>,
    /// The base branch a pull request is diffed against; default: the repo's default branch.
    pub base: Option<String>,
    /// For tests: what `repo.git` is cloned from instead of `https://github.com/<repo>.git`.
    pub clone_source: Option<String>,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    pub reviewer_time_limit: Option<String>,
    /// The worker's time limit for the fix round (issue tasks).
    pub time_limit: Option<String>,
    pub profile: Option<String>,
}

fn gates_failed_line(id: &str, failed: &GateResult, log: &std::path::Path) -> String {
    let exit = failed
        .exit
        .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
    format!(
        "{id}: gates failed: `{}` ({}, {exit}, {}); output in {}",
        failed.command,
        failed.tier,
        failed.phase,
        log.display()
    )
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
    if let Target::Issue(0) | Target::Pr(0) = opts.target {
        bail!("issue and PR numbers start at 1");
    }
    // Every input is checked before anything is created.
    let mut config = TaskConfig::load(&opts.repo_root)?;
    if let Some(harness) = opts.reviewer_harness {
        let harness = headless_harness("--reviewer-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
        config.set_reviewer_harness(harness);
    }
    if let Some(model) = &opts.reviewer_model {
        check_model_flag("--reviewer-model", model).map_err(|e| anyhow!(e))?;
        config.reviewer.model = Some(model.clone());
    }
    if let Some(limit) = &opts.reviewer_time_limit {
        config.reviewer.time_limit =
            parse_duration("--reviewer-time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(limit) = &opts.time_limit {
        config.worker.time_limit = parse_duration("--time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(profile) = &opts.profile {
        config.sandbox.profile = profile.clone();
    }
    // The same-harness warning is wanted even if the review then fails, so say it up front.
    for warning in &config.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    match &opts.target {
        Target::Issue(n) => run_issue(
            config_dir, opts, &config, *n, backend, probe, host, out, warn,
        ),
        Target::Pr(n) => run_pr(
            config_dir, opts, &config, *n, backend, github, probe, host, out, warn,
        ),
        Target::Spec(path) => run_spec(config_dir, &config, path, backend, probe, host, out, warn),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_issue(
    config_dir: &std::path::Path,
    _opts: &Options,
    config: &TaskConfig,
    number: u32,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = format!("issue-{number}");
    let mut prepared = Prepared::open(&base_dir, &id)?;
    // The worker is whoever the task recorded: a flag of `task start`/`run` may have chosen it, and
    // the file read now can say something else. The fix round runs that harness in that sandbox,
    // and the default reviewer must differ from it (decision 148).
    let mut config = config.clone();
    if let Some(worker) = &prepared.record.worker {
        if let Ok(harness) = <Harness as clap::ValueEnum>::from_str(&worker.harness, true) {
            config.set_worker_harness(harness);
        }
        config.worker.model.clone_from(&worker.model);
    }
    let config = &config;
    let env = TaskEnv {
        config_dir,
        config,
        backend,
        probe,
        host,
    };
    let report = pipeline::review_issue(&env, &mut prepared)?;
    for warning in report
        .warnings
        .iter()
        .filter(|w| !config.warnings.contains(w))
    {
        writeln!(warn, "warning: {warning}")?;
    }

    let meta = record::task_dir(&base_dir, &id);
    if let Some(failed) = &report.gates_failed {
        writeln!(
            out,
            "{}",
            gates_failed_line(&id, failed, &meta.join("gates.log"))
        )?;
        for round in &report.rounds {
            writeln!(
                out,
                "{id}: review round {}: {} must-fix finding(s)",
                round.round, round.must_fix
            )?;
        }
        bail!(
            "gates failed for {id}: `{}`; fix it, then run `sbxm task gates --issue {number}` and review again",
            failed.command
        );
    }

    for round in &report.rounds {
        writeln!(
            out,
            "{id}: review round {}: {} must-fix finding(s){}",
            round.round,
            round.must_fix,
            if round.full { "" } else { " (narrow)" }
        )?;
    }
    if report.fix_ran {
        writeln!(
            out,
            "{id}: the fix round ran {} time(s); the gates after each passed",
            prepared.record.round
        )?;
    }
    let review = meta.join("review.md");
    match (report.must_fix_left, prepared.record.stopped) {
        (0, _) => {
            writeln!(out, "{id}: ready; the review is {}", review.display())?;
            writeln!(out, "  next: sbxm task finish --issue {number}")?;
        }
        (n, Some(stopped)) => {
            writeln!(
                out,
                "{id}: ready, with {n} must-fix finding(s) left (stopped: {}); read {}",
                stopped.name(),
                review.display()
            )?;
            writeln!(
                out,
                "  next: fix them by hand, or file them: sbxm task file-findings --issue {number}"
            )?;
        }
        (n, None) => {
            writeln!(
                out,
                "{id}: ready, with {n} must-fix finding(s) left; read {}",
                review.display()
            )?;
            writeln!(
                out,
                "  next: fix them by hand, or file them: sbxm task file-findings --issue {number}"
            )?;
        }
    }
    Ok(())
}

/// `task review --spec <FILE>` (decision 174(d), issue 142): the same gates/review/fix-round loop
/// as `run_issue`'s (`pipeline::review_issue`, which makes no GitHub call), but `finish` is
/// refused for the result, so the "ready" hint says where the review is instead of naming it.
#[allow(clippy::too_many_arguments)]
fn run_spec(
    config_dir: &std::path::Path,
    config: &TaskConfig,
    path: &std::path::Path,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let (id, _) = record::spec_id(path)?;
    let mut prepared = Prepared::open(&base_dir, &id)?;
    // As `run_issue`: the worker is whoever the task recorded.
    let mut config = config.clone();
    if let Some(worker) = &prepared.record.worker {
        if let Ok(harness) = <Harness as clap::ValueEnum>::from_str(&worker.harness, true) {
            config.set_worker_harness(harness);
        }
        config.worker.model.clone_from(&worker.model);
    }
    let config = &config;
    let env = TaskEnv {
        config_dir,
        config,
        backend,
        probe,
        host,
    };
    let report = pipeline::review_issue(&env, &mut prepared)?;
    for warning in report
        .warnings
        .iter()
        .filter(|w| !config.warnings.contains(w))
    {
        writeln!(warn, "warning: {warning}")?;
    }

    let meta = record::task_dir(&base_dir, &id);
    if let Some(failed) = &report.gates_failed {
        writeln!(
            out,
            "{}",
            gates_failed_line(&id, failed, &meta.join("gates.log"))
        )?;
        for round in &report.rounds {
            writeln!(
                out,
                "{id}: review round {}: {} must-fix finding(s)",
                round.round, round.must_fix
            )?;
        }
        bail!(
            "gates failed for {id}: `{}`; fix it in the worker's clone and commit, then run \
             `sbxm task review --spec {}` again",
            failed.command,
            path.display()
        );
    }

    for round in &report.rounds {
        writeln!(
            out,
            "{id}: review round {}: {} must-fix finding(s){}",
            round.round,
            round.must_fix,
            if round.full { "" } else { " (narrow)" }
        )?;
    }
    if report.fix_ran {
        writeln!(
            out,
            "{id}: the fix round ran {} time(s); the gates after each passed",
            prepared.record.round
        )?;
    }
    let review = meta.join("review.md");
    match (report.must_fix_left, prepared.record.stopped) {
        (0, _) => {
            writeln!(out, "{id}: ready; the review is {}", review.display())?;
        }
        (n, Some(stopped)) => {
            writeln!(
                out,
                "{id}: ready, with {n} must-fix finding(s) left (stopped: {}); read {}",
                stopped.name(),
                review.display()
            )?;
        }
        (n, None) => {
            writeln!(
                out,
                "{id}: ready, with {n} must-fix finding(s) left; read {}",
                review.display()
            )?;
        }
    }
    writeln!(
        out,
        "  note: `sbxm task finish` is refused for a spec task until its sink is built; fix \
         findings by hand in the task's clone"
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_pr(
    config_dir: &std::path::Path,
    opts: &Options,
    config: &TaskConfig,
    number: u32,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = format!("pr-{number}");
    let repo_name = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let base_branch = match &opts.base {
        Some(base) => base.clone(),
        None => github.default_branch(&repo_name)?,
    };
    if !repo::valid_ref_name(&base_branch) {
        bail!("base {base_branch:?} isn't a usable branch name; pass --base <branch>");
    }
    let clone_source = opts
        .clone_source
        .clone()
        .unwrap_or_else(|| format!("https://github.com/{repo_name}.git"));
    // A PR review makes no worker clone, so no committer identity is needed.
    let identity = Identity {
        name: "sbxm".into(),
        email: "sbxm@localhost".into(),
    };
    let ctx = Ctx {
        config_dir,
        repo: &repo_name,
        clone_source: &clone_source,
        base_branch: &base_branch,
        config,
        backend,
        github,
        identity: &identity,
        probe,
        host,
    };

    // A task that already exists is resumed when its review can be tried again (a failed
    // reviewer, failed gates); otherwise the refusal says what to do.
    let mut prepared = if finish::exists(&base_dir, &id) {
        let prepared = Prepared::open(&base_dir, &id)?;
        pipeline::check_can_review_pr(&prepared.record)?;
        prepared
    } else {
        let pr = github.pr(&repo_name, number)?;
        let prepared = pipeline::prepare_pr(&ctx, &pr)?;
        for warning in prepared
            .warnings
            .iter()
            .filter(|w| !config.warnings.contains(w))
        {
            writeln!(warn, "warning: {warning}")?;
        }
        prepared
    };

    let meta = record::task_dir(&base_dir, &id);
    let report = pipeline::review_pr(&ctx.env(), github, &mut prepared)?;
    if let Some(failed) = &report.gates_failed {
        writeln!(
            out,
            "{}",
            gates_failed_line(&id, failed, &meta.join("gates.log"))
        )?;
        bail!(
            "gates failed for {id}: `{}`; fix the PR, then run `sbxm task rm --pr {number}` and review it again",
            failed.command
        );
    }
    if let Some(reviewed) = &report.reviewed {
        writeln!(
            out,
            "{id}: review: {} must-fix finding(s)",
            reviewed.must_fix
        )?;
    }
    if report.posted {
        writeln!(
            out,
            "{id}: the review was posted on PR #{number}; a copy is {}",
            meta.join("review.md").display()
        )?;
    }
    Ok(())
}
