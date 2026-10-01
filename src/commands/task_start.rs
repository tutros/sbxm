//! `sbxm task start (--issue N... | --workers N)` (spec §4, §5.1): resolves the repo, the base
//! and the config (flags override the file), chooses the issues, and starts a task for each.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::backend::SandboxBackend;
use crate::git;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::headless::RunStatus;
use crate::run::config::{headless_harness, parse_duration};
use crate::task::config::TaskConfig;
use crate::task::pipeline::{self, Ctx};
use crate::task::record::ProcessProbe;
use crate::task::repo::{self, Identity};

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    /// `--issue N...`: start exactly these.
    pub issues: Vec<u32>,
    /// `--workers N`: start up to N issues chosen by the selection rules.
    pub workers: Option<usize>,
    pub worker_harness: Option<Harness>,
    pub worker_model: Option<String>,
    pub time_limit: Option<String>,
    pub profile: Option<String>,
    /// Default: the repo's default branch on GitHub.
    pub base: Option<String>,
    /// `owner/name`. Default: the `origin` of the checkout at `repo_root`.
    pub repo: Option<String>,
    /// For tests: what `repo.git` is cloned from instead of `https://github.com/<repo>.git`.
    pub clone_source: Option<String>,
    /// For tests: the committer identity instead of the user's global git config.
    pub identity: Option<Identity>,
}

fn name_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `owner/name` from a GitHub URL (https, `git@github.com:`, or ssh).
pub fn github_repo(url: &str) -> Result<String> {
    let fail = || anyhow!("origin {url:?} isn't a GitHub URL; pass --repo owner/name");
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let after = ["https://", "http://", "ssh://"]
            .iter()
            .find_map(|scheme| url.strip_prefix(scheme))
            .ok_or_else(fail)?;
        let (host, path) = after.split_once('/').ok_or_else(fail)?;
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        if host != "github.com" {
            return Err(fail());
        }
        path
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [owner, name] if name_ok(owner) && name_ok(name) => Ok(format!("{owner}/{name}")),
        _ => Err(fail()),
    }
}

fn check_repo(repo: &str) -> Result<()> {
    match repo.split('/').collect::<Vec<_>>().as_slice() {
        [owner, name] if name_ok(owner) && name_ok(name) => Ok(()),
        _ => bail!("--repo {repo:?} isn't owner/name; write it like tutros/sbxm"),
    }
}

fn plural(n: u32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Starts the tasks and prints one line per task to `out` and warnings to `warn`. Fails if any
/// task failed (after printing everything).
pub fn run(
    config_dir: &std::path::Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if opts.issues.is_empty() && opts.workers.is_none() {
        bail!("say which issues to start: --issue <n> (repeatable) or --workers <n>");
    }
    if !opts.issues.is_empty() && opts.workers.is_some() {
        bail!("use either --issue <n> or --workers <n>, not both");
    }

    // Everything that can be wrong with the inputs, before anything is created.
    let mut config = TaskConfig::load(&opts.repo_root)?;
    if let Some(harness) = opts.worker_harness {
        config.worker.harness = headless_harness("--worker-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
    }
    if let Some(model) = &opts.worker_model {
        config.worker.model = Some(model.clone());
    }
    if let Some(limit) = &opts.time_limit {
        config.worker.time_limit = parse_duration("--time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(profile) = &opts.profile {
        config.sandbox.profile = profile.clone();
    }
    let repo_name = match &opts.repo {
        Some(repo) => {
            check_repo(repo)?;
            repo.clone()
        }
        None => {
            let url =
                git::user_run(&opts.repo_root, &["remote", "get-url", "origin"]).map_err(|e| {
                    anyhow!(
                        "cannot read the origin of {}: {e:#}; pass --repo owner/name",
                        opts.repo_root.display()
                    )
                })?;
            github_repo(url.trim())?
        }
    };
    let identity = match &opts.identity {
        Some(identity) => identity.clone(),
        None => Identity::read_from(None)?,
    };
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
    for warning in &config.warnings {
        writeln!(warn, "warning: {warning}")?;
    }

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
    };
    let selection = if opts.issues.is_empty() {
        pipeline::select_issues(&ctx, None, opts.workers.unwrap_or(1))?
    } else {
        pipeline::select_issues(&ctx, Some(&opts.issues), opts.issues.len())?
    };
    for number in &selection.not_open {
        writeln!(warn, "warning: #{number} isn't an open issue; skipped")?;
    }
    for (number, why) in &selection.skips {
        writeln!(out, "#{number}: skipped, {why}")?;
    }

    let names: Vec<String> = selection
        .picks
        .iter()
        .map(|n| format!("issue-{n}"))
        .collect();
    writeln!(out, "Starting {} ...", names.join(", "))?;
    let reports = pipeline::start(&ctx, &selection.picks);

    let mut failed = 0;
    for report in &reports {
        let id = format!("issue-{}", report.number);
        for warning in &report.warnings {
            writeln!(warn, "warning: {warning}")?;
        }
        match &report.result {
            Ok(worked) => {
                let (label, bad) = match &worked.status {
                    RunStatus::Completed => ("completed".to_owned(), false),
                    RunStatus::TimedOut => ("timed out".to_owned(), false),
                    RunStatus::Failed(why) => (format!("failed: {why}"), true),
                };
                writeln!(
                    out,
                    "{id}: worker {label}, {}",
                    plural(worked.commits, "commit")
                )?;
                for note in &worked.notes {
                    writeln!(out, "  note: {note}")?;
                }
                if bad {
                    failed += 1;
                    writeln!(out, "  see: sbxm task status --issue {}", report.number)?;
                } else {
                    writeln!(out, "  next: sbxm task review --issue {}", report.number)?;
                }
            }
            Err(e) => {
                failed += 1;
                writeln!(out, "{id}: failed: {e:#}")?;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} task(s) failed; see above", reports.len());
    }
    Ok(())
}
