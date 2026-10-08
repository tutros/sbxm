//! `sbxm task start (--issue N... | --workers N)` (spec §4, §5.1): resolves the repo, the base
//! and the config (flags override the file), chooses the issues, and starts a task for each.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::git;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::headless::RunStatus;
use crate::run::config::{check_model_flag, headless_harness, parse_duration};
use crate::task::config::TaskConfig;
use crate::task::finish;
use crate::task::gates::ShellHostRunner;
use crate::task::pipeline::{self, Ctx};
use crate::task::record::{self, ProcessProbe};
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

pub(crate) fn check_repo(repo: &str) -> Result<()> {
    match repo.split('/').collect::<Vec<_>>().as_slice() {
        [owner, name] if name_ok(owner) && name_ok(name) => Ok(()),
        _ => bail!("--repo {repo:?} isn't owner/name; write it like tutros/sbxm"),
    }
}

/// Discards the existing task of each issue (`--restart`). Every one is checked before the first
/// is deleted: the issue must still be open (else the restart would only destroy the work), the
/// PR it names (if any) must still be continuable, and the task must not be running.
fn discard_existing(
    ctx: &Ctx,
    issues: &[u32],
    restart: &Restart,
    out: &mut dyn Write,
) -> Result<()> {
    let (repo, github, backend, probe) = (ctx.repo, ctx.github, ctx.backend, ctx.probe);
    let base_dir = GlobalConfig::load(ctx.config_dir)?.base_dir;
    let mut ids: Vec<(u32, String)> = Vec::new();
    for &number in issues {
        let id = format!("issue-{number}");
        if finish::exists(&base_dir, &id) && !ids.iter().any(|(_, known)| *known == id) {
            ids.push((number, id));
        }
    }
    for (number, id) in &ids {
        let issue = github.issue(repo, *number)?;
        if issue.state != "OPEN" {
            bail!(
                "issue #{number} is {}, not open; --restart would only delete task {id}; remove it with `sbxm task rm --issue {number}` if you want it gone",
                issue.state.to_lowercase()
            );
        }
        finish::plan_removal(&base_dir, id, probe)?;
        // An issue of a PR that is closed, merged or a fork's would fail to start again.
        pipeline::continued_pr(ctx, &issue)?;
    }
    // A requested issue with no task yet is started after the deletions, so its PR is checked
    // now too. One that can't be read or isn't open is left to the selection, which skips it.
    for &number in issues {
        if ids.iter().any(|(known, _)| *known == number) {
            continue;
        }
        if let Ok(issue) = github.issue(repo, number)
            && issue.state == "OPEN"
        {
            pipeline::continued_pr(ctx, &issue)?;
        }
    }
    // Each issue must be able to start again (not a question, not blocked, ...) before the first
    // task is deleted, and the user is asked once for all of them.
    let restarting: Vec<u32> = ids.iter().map(|(number, _)| *number).collect();
    pipeline::check_restartable(ctx, &restarting)?;
    let id_list: Vec<String> = ids.into_iter().map(|(_, id)| id).collect();
    if id_list.is_empty() {
        return Ok(());
    }
    finish::discard_many(
        &base_dir,
        &id_list,
        restart.yes,
        backend,
        probe,
        restart.confirm,
        out,
    )
}

/// The branch the checkout's `origin/HEAD` points at, for a spec task's default base (no GitHub
/// call). `git clone` sets it; a checkout made some other way may not have it.
fn local_default_branch(repo_root: &std::path::Path) -> Result<String> {
    let head = git::user_run(
        repo_root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map_err(|_| {
        anyhow!(
            "cannot tell the default branch from {} without GitHub; pass --base <branch>",
            repo_root.display()
        )
    })?;
    let head = head.trim();
    Ok(head.strip_prefix("origin/").unwrap_or(head).to_owned())
}

/// `--spec <path>` as the retry hints print it: the path is one argument, quoted when it has a
/// space or a character a shell would read (double quotes work in PowerShell and POSIX shells;
/// single quotes, with `'` doubled, when the path itself has a `"`, `$` or backtick).
pub(crate) fn spec_flag(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || "/\\:._-+=@%,~".contains(c));
    let arg = if plain {
        text
    } else if text.contains(['"', '$', '`']) {
        format!("'{}'", text.replace('\'', "''"))
    } else {
        format!("\"{text}\"")
    };
    format!("--spec {arg}")
}

/// `--repo owner/name`, or the GitHub repo of the checkout's `origin`.
pub(crate) fn resolve_repo(repo: Option<&str>, repo_root: &std::path::Path) -> Result<String> {
    match repo {
        Some(repo) => {
            check_repo(repo)?;
            Ok(repo.to_owned())
        }
        None => {
            let url = git::user_run(repo_root, &["remote", "get-url", "origin"]).map_err(|e| {
                anyhow!(
                    "cannot read the origin of {}: {e:#}; pass --repo owner/name",
                    repo_root.display()
                )
            })?;
            github_repo(url.trim())
        }
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
    run_with(config_dir, opts, None, backend, github, probe, out, warn)
}

/// `--restart`: discard the task of each `--issue` that has one before starting it.
pub struct Restart<'a> {
    pub confirm: &'a dyn Confirm,
    /// `--yes`: don't ask.
    pub yes: bool,
}

/// [`run`], optionally discarding existing tasks first (spec §4, decision 153): all of them are
/// checked before any is deleted, and nothing is deleted if an input is wrong.
#[allow(clippy::too_many_arguments)]
pub fn run_with(
    config_dir: &std::path::Path,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if restart.is_some() && opts.issues.is_empty() {
        bail!("--restart needs the issues to restart: --issue <n> (repeatable)");
    }
    if opts.issues.is_empty() && opts.workers.is_none() {
        bail!("say which issues to start: --issue <n> (repeatable) or --workers <n>");
    }
    if !opts.issues.is_empty() && opts.workers.is_some() {
        bail!("use either --issue <n> or --workers <n>, not both");
    }

    // Everything that can be wrong with the inputs, before anything is created.
    let mut config = TaskConfig::load(&opts.repo_root)?;
    if let Some(harness) = opts.worker_harness {
        let harness = headless_harness("--worker-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
        config.set_worker_harness(harness);
    }
    if let Some(model) = &opts.worker_model {
        check_model_flag("--worker-model", model).map_err(|e| anyhow!(e))?;
        config.worker.model = Some(model.clone());
    }
    if let Some(limit) = &opts.time_limit {
        config.worker.time_limit = parse_duration("--time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(profile) = &opts.profile {
        config.sandbox.profile = profile.clone();
    }
    let repo_name = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
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
        host: &ShellHostRunner,
    };
    if let Some(restart) = restart {
        discard_existing(&ctx, &opts.issues, restart, out)?;
    }
    let selection = if opts.issues.is_empty() {
        pipeline::choose_issues(&ctx, None, opts.workers.unwrap_or(1))?
    } else {
        pipeline::choose_issues(&ctx, Some(&opts.issues), opts.issues.len())?
    };
    for warning in &selection.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    for number in &selection.not_open {
        writeln!(warn, "warning: #{number} isn't an open issue; skipped")?;
    }
    for (number, why) in &selection.skips {
        writeln!(out, "#{number}: skipped, {why}")?;
    }
    let selection = pipeline::require_picks(selection)?;

    let names: Vec<String> = selection
        .picks
        .iter()
        .map(|n| format!("issue-{n}"))
        .collect();
    writeln!(out, "Starting {} ...", names.join(", "))?;
    let reports = pipeline::start(&ctx, &selection.picks);
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;

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
                // `gates_ok`: the gates passed (or none were configured); a failed worker has none.
                let mut gates_ok = !bad;
                match &report.gates {
                    Some(Ok(gated)) if gated.passed => {
                        let count = |tier: &str| {
                            gated
                                .outcomes
                                .iter()
                                .filter(|o| o.result.tier == tier)
                                .count()
                        };
                        if gated.outcomes.is_empty() {
                            writeln!(out, "  gates: none configured")?;
                        } else {
                            writeln!(
                                out,
                                "  gates: passed ({} sandbox, {} host)",
                                count("sandbox"),
                                count("host")
                            )?;
                        }
                    }
                    Some(Ok(gated)) => {
                        gates_ok = false;
                        if let Some(first) = &gated.failed {
                            let exit = first
                                .exit
                                .map_or_else(|| "no exit code".to_owned(), |c| format!("exit {c}"));
                            writeln!(
                                out,
                                "  gates: failed: `{}` ({}, {exit}); output in {}",
                                first.command,
                                first.tier,
                                record::task_dir(&base_dir, &id).join("gates.log").display()
                            )?;
                        }
                    }
                    Some(Err(e)) => {
                        gates_ok = false;
                        writeln!(out, "  gates: could not run: {e:#}")?;
                    }
                    None => {}
                }
                if bad {
                    failed += 1;
                    writeln!(out, "  see: sbxm task status --issue {}", report.number)?;
                } else if !gates_ok {
                    failed += 1;
                    writeln!(
                        out,
                        "  next: fix it in the worker's clone and commit, then run: sbxm task gates --issue {}",
                        report.number
                    )?;
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

/// `task start --spec <FILE>` (decision 174(d), issue 142): one task, not a selection of issues.
pub struct SpecOptions {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub spec: PathBuf,
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

/// Starts a spec task: prepares it, runs the worker, then the gates (as `run` does for an issue
/// task), and prints what happened. No GitHub call is made anywhere in this path.
#[allow(clippy::too_many_arguments)]
pub fn run_spec(
    config_dir: &std::path::Path,
    opts: &SpecOptions,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let mut config = TaskConfig::load(&opts.repo_root)?;
    if let Some(harness) = opts.worker_harness {
        let harness = headless_harness("--worker-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
        config.set_worker_harness(harness);
    }
    if let Some(model) = &opts.worker_model {
        check_model_flag("--worker-model", model).map_err(|e| anyhow!(e))?;
        config.worker.model = Some(model.clone());
    }
    if let Some(limit) = &opts.time_limit {
        config.worker.time_limit = parse_duration("--time-limit", limit).map_err(|e| anyhow!(e))?;
    }
    if let Some(profile) = &opts.profile {
        config.sandbox.profile = profile.clone();
    }
    let repo_name = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let identity = match &opts.identity {
        Some(identity) => identity.clone(),
        None => Identity::read_from(None)?,
    };
    // A spec task makes no GitHub call, so an omitted base is read from the checkout (`origin/HEAD`,
    // which `git clone` sets), not asked of GitHub.
    let base_branch = match &opts.base {
        Some(base) => base.clone(),
        None => {
            // The checkout's `origin/HEAD` is the default of the checkout's own origin only.
            let origin = resolve_repo(None, &opts.repo_root);
            if opts.repo.is_some() && !origin.is_ok_and(|o| o.eq_ignore_ascii_case(&repo_name)) {
                bail!(
                    "cannot tell the default branch of {repo_name} without GitHub (it isn't this \
                     checkout's origin); pass --base <branch>"
                );
            }
            local_default_branch(&opts.repo_root)?
        }
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
        host: &ShellHostRunner,
    };
    writeln!(out, "Starting a task from {} ...", opts.spec.display())?;
    let mut prepared = pipeline::prepare_spec(&ctx, &opts.spec)?;
    for warning in &prepared.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let id = prepared.record.id.clone();

    let worked = pipeline::run_worker(&ctx, &mut prepared)?;
    let (label, bad) = match &worked.status {
        RunStatus::Completed => ("completed".to_owned(), false),
        RunStatus::TimedOut => ("timed out".to_owned(), false),
        RunStatus::Failed(why) => (format!("failed: {why}"), true),
    };
    writeln!(out, "{id}: worker {label}, {} commit(s)", worked.commits)?;
    for note in &worked.notes {
        writeln!(out, "  note: {note}")?;
    }
    if bad {
        bail!(
            "{id}: the worker failed; see `sbxm task status` and the task's folder for what it left"
        );
    }

    let gated = pipeline::run_gates(
        &ctx.env(),
        &mut prepared,
        "after-worker",
        pipeline::Tiers::ALL,
    )?;
    if gated.passed {
        if gated.outcomes.is_empty() {
            writeln!(out, "  gates: none configured")?;
        } else {
            writeln!(out, "  gates: passed")?;
        }
        writeln!(out, "  next: sbxm task review {}", spec_flag(&opts.spec))?;
        Ok(())
    } else {
        let first = gated.failed.expect("failed gates name the first failure");
        let exit = first
            .exit
            .map_or_else(|| "no exit code".to_owned(), |c| format!("exit {c}"));
        bail!(
            "{id}: gates failed: `{}` ({}, {exit}); fix it in the worker's clone and commit, \
             then run: sbxm task review {}",
            first.command,
            first.tier,
            spec_flag(&opts.spec)
        )
    }
}
