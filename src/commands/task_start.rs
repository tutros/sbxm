//! `sbxm task start (--issue N... | --workers N)` (spec §4, §5.1): resolves the repo, the base
//! and the config (flags override the file), chooses the issues, and starts a task for each.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::disk;
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
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
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

/// Discards the existing task of each issue (`--restart`), moving each aside instead of deleting
/// it (decision 185): the caller decides, once it knows which of them started again, whether to
/// settle or restore each one. Every one is checked before the first is set aside: no leftover
/// from an earlier, unsettled `--restart` ([`finish::check_no_leftover`]), the issue must still be
/// open (else the restart would only destroy the work), the PR it names (if any) must still be
/// continuable, and the task must not be running. `base_dir` is resolved by the caller, once,
/// before anything is set aside, and reused after: reloading it again afterward could fail
/// (config.toml changing or vanishing while confirmation was pending) outside the guarded restore
/// path that only covers what happens from here on (issue #207 review M-1).
fn discard_existing(
    ctx: &Ctx,
    base_dir: &std::path::Path,
    issues: &[u32],
    restart: &Restart,
    out: &mut dyn Write,
) -> Result<Vec<finish::SetAside>> {
    let (repo, github, backend, probe) = (ctx.repo, ctx.github, ctx.backend, ctx.probe);
    // Checked for every requested issue, not only those with a task right now: an earlier
    // `--restart` that was interrupted after setting the old task aside but before settling it
    // may have left no live task folder at all, which must not look like a clean slate to start
    // a brand new task over (decision 185 point 4).
    for &number in issues {
        finish::check_no_leftover(base_dir, &format!("issue-{number}"))?;
    }
    let mut ids: Vec<(u32, String)> = Vec::new();
    for &number in issues {
        let id = format!("issue-{number}");
        if finish::exists(base_dir, &id) && !ids.iter().any(|(_, known)| *known == id) {
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
        finish::plan_removal(base_dir, id, probe)?;
        // An issue of a PR that is closed, merged or a fork's would fail to start again.
        pipeline::continued_pr(ctx.repo, ctx.github, &issue)?;
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
            pipeline::continued_pr(ctx.repo, ctx.github, &issue)?;
        }
    }
    // Each issue must be able to start again (not a question, not blocked, ...) before the first
    // task is deleted, and the user is asked once for all of them.
    let restarting: Vec<u32> = ids.iter().map(|(number, _)| *number).collect();
    pipeline::check_restartable(ctx, &restarting)?;
    let id_list: Vec<String> = ids.into_iter().map(|(_, id)| id).collect();
    if id_list.is_empty() {
        return Ok(Vec::new());
    }
    finish::set_aside_many(
        base_dir,
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

/// `--spec <path>` as the retry hints print it (see [`path_arg`]).
pub(crate) fn spec_flag(path: &std::path::Path) -> String {
    format!("--spec {}", path_arg(path))
}

/// A path as one argument of a printed command, quoted when it has a space or a character a shell
/// would read (double quotes work in PowerShell and POSIX shells; single quotes when the path
/// itself has a `"`, `$` or backtick, with a `'` inside doubled for PowerShell on Windows and
/// written `'\''` for a POSIX shell elsewhere, which can't double it).
pub(crate) fn path_arg(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || "/\\:._-+=@%,~".contains(c));
    if plain {
        text
    } else if text.contains(['"', '$', '`']) {
        let quote = if cfg!(windows) { "''" } else { r"'\''" };
        format!("'{}'", text.replace('\'', quote))
    } else {
        format!("\"{text}\"")
    }
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

/// The error [`run_with`] returns when every failed task got through its worker and only its gates
/// failed: `task run` goes on to the review then, which reruns them and gives a failure to a fix
/// round (decision 180).
#[derive(Debug)]
pub struct GatesFailed(String);

impl std::fmt::Display for GatesFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GatesFailed {}

/// Whether a task failed only because its gates ran and failed, the one failure [`GatesFailed`]
/// stands for. `gates` is the gate run after the worker, if there was one: `Some(Ok(gated))` when
/// the run itself completed (whether or not every gate passed), `Some(Err(_))` when the gate run
/// itself returned an error, which is not a gate failure and stops `task run`, or `None` when
/// there was no gate run (the worker failed first). A gate that could not run at all (issue 186)
/// does not count either: the review would have nothing to react to, so the plain error is
/// returned instead and `task run` stops after the start phase.
fn only_gates_failed(worker_failed: bool, gates: Option<&Result<pipeline::Gated>>) -> bool {
    !worker_failed
        && matches!(
            gates,
            Some(Ok(gated)) if !gated.passed
                && !gated.failed_outcome().is_some_and(|o| o.could_not_run)
        )
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
    if restart.is_none() {
        return start(config_dir, opts, None, backend, github, probe, out, warn);
    }
    // A restart sets old tasks aside and must get to settling or restoring each one: its screen
    // output can't stop it on the way (decision 185).
    let (mut out, mut warn) = (finish::Unfailing::new(out), finish::Unfailing::new(warn));
    let result = start(
        config_dir, opts, restart, backend, github, probe, &mut out, &mut warn,
    );
    finish::after_output(result, [out.into_error(), warn.into_error()])
}

#[allow(clippy::too_many_arguments)]
fn start(
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
    disk::require(&GlobalConfig::load(config_dir)?, config_dir, &opts.disk)?;
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
    // Resolved once, before any task is set aside, and reused for the rest of this function: a
    // second load after the discard could itself fail (issue #207 review M-1) outside the path
    // below that restores a set-aside task on any later failure.
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let mut aside_by_id: HashMap<String, finish::SetAside> = HashMap::new();
    if let Some(restart) = restart {
        for aside in discard_existing(&ctx, &base_dir, &opts.issues, restart, out)? {
            aside_by_id.insert(aside.id().to_owned(), aside);
        }
    }

    // Settled the moment each issue's preparation succeeds (decision 185(b)), before its worker
    // runs: called from that issue's own thread inside `pipeline::start_with`, so a later failure
    // of the worker or its gates never finds anything left of the old task to put back. Shared
    // across threads behind a `Mutex`, since only the thread whose issue just finished preparing
    // removes that one entry; `settle_warnings` carries what a failed deletion says, printed once
    // every thread has finished.
    let asides = std::sync::Mutex::new(aside_by_id);
    let settle_warnings: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let on_prepared = |number: u32| {
        let id = format!("issue-{number}");
        let Some(aside) = asides.lock().unwrap().remove(&id) else {
            return;
        };
        if let Err(e) = aside.settle() {
            settle_warnings.lock().unwrap().push(format!(
                "{id}'s old copy at {} could not be deleted: {e:#}",
                aside.folder().display()
            ));
        }
    };

    let run_outcome: Result<Vec<pipeline::TaskReport>> = (|| {
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
        Ok(pipeline::start_with(&ctx, &selection.picks, &on_prepared))
    })();

    for warning in settle_warnings.into_inner().unwrap() {
        writeln!(warn, "warning: {warning}")?;
    }
    let mut aside_by_id = asides.into_inner().unwrap();

    // Every restarted task that never got settled above (its preparation failed, or the run never
    // reached it) is put back now, regardless of where the run stopped (decision 185 point 3): a
    // selection failure never even reaches `pipeline::start_with`, so none of them got a report at
    // all; here that is treated the same as one that did and failed.
    let reports = match run_outcome {
        Ok(reports) => reports,
        Err(e) => {
            for (id, aside) in aside_by_id {
                let msg = finish::restore_after_failed_start(&aside, anyhow!("{e:#}"));
                writeln!(out, "{id}: {msg:#}")?;
            }
            return Err(e);
        }
    };
    let total = reports.len();

    let (mut failed, mut gates_failed, mut exec_unavailable, mut restored) = (0, 0, 0, 0);
    for report in reports {
        let pipeline::TaskReport {
            number,
            warnings,
            result,
            gates,
        } = report;
        let id = format!("issue-{number}");
        for warning in &warnings {
            writeln!(warn, "warning: {warning}")?;
        }
        // A task whose preparation completed was already settled by `on_prepared`, whether or
        // not its worker then failed, so this only ever finds something for a task whose
        // preparation itself failed.
        let aside = aside_by_id.remove(&id);
        match result {
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
                // Set when the failed gate could not run at all (issue 186): the review would have
                // nothing to react to, so `task run` must not go on to it.
                let mut gate_could_not_run = false;
                if only_gates_failed(bad, gates.as_ref()) {
                    gates_failed += 1;
                }
                match &gates {
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
                            let log = record::task_dir(&base_dir, &id).join("gates.log");
                            match gated.failed_outcome().filter(|o| o.could_not_run) {
                                Some(outcome) => {
                                    gate_could_not_run = true;
                                    writeln!(
                                        out,
                                        "  gates: could not run: `{}` ({}); {}; see {}",
                                        first.command,
                                        first.tier,
                                        outcome.output_tail,
                                        log.display()
                                    )?;
                                }
                                None => {
                                    let exit = first.exit.map_or_else(
                                        || "no exit code".to_owned(),
                                        |c| format!("exit {c}"),
                                    );
                                    writeln!(
                                        out,
                                        "  gates: failed: `{}` ({}, {exit}); output in {}",
                                        first.command,
                                        first.tier,
                                        log.display()
                                    )?;
                                }
                            }
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
                    writeln!(out, "  see: sbxm task status --issue {number}")?;
                } else if !gates_ok {
                    failed += 1;
                    // The review reruns the gates and gives a failure to a fix round (decision
                    // 180) — but only for a gate that ran and failed; one that could not run at
                    // all would give the review nothing to react to (issue 186).
                    if !gate_could_not_run {
                        writeln!(out, "  next: sbxm task review --issue {number}")?;
                    }
                } else {
                    writeln!(out, "  next: sbxm task review --issue {number}")?;
                }
            }
            Err(e) => {
                failed += 1;
                if e.is::<crate::headless::ExecUnavailable>() {
                    exec_unavailable += 1;
                }
                match aside {
                    // The task's preparation never completed, so its old copy goes back.
                    Some(aside) => {
                        restored += 1;
                        let msg = finish::restore_after_failed_start(&aside, e);
                        writeln!(out, "{id}: {msg:#}")?;
                    }
                    None => writeln!(out, "{id}: failed: {e:#}")?,
                }
            }
        }
    }
    // A restarted issue the selection didn't even pick this time around never got a report, so
    // the loop above never saw it; it is exactly as unprepared as one that did and failed.
    for (id, aside) in aside_by_id {
        restored += 1;
        let msg = finish::restore_after_failed_start(
            &aside,
            anyhow!("issue was not picked to start again"),
        );
        writeln!(out, "{id}: {msg:#}")?;
    }
    if failed > 0 {
        let message = format!("{failed} of {total} task(s) failed; see above");
        if failed == gates_failed && restored == 0 {
            return Err(GatesFailed(message).into());
        }
        // Decision 193(a), issue 201 review M-2: `SandboxBackend::exec` itself failing (not a
        // harness-reported failure, which is `Ok` with a status) means `sbx` couldn't run, the
        // same meaning `check_secrets`'s `SbxUnavailable` already carries; a `task run` list stops
        // for it instead of counting it as this task's own failure. `restored == 0`, as above: a
        // task whose preparation never completed (restarted and put back) never got this far.
        if failed == exec_unavailable && restored == 0 {
            return Err(crate::headless::ExecUnavailable(message).into());
        }
        bail!(message);
    }
    if restored > 0 {
        bail!(
            "{restored} restarted task(s) could not be started again and were put back; see above"
        );
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
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
}

/// Every input [`run_spec`] resolves before it does anything with the backend or writes
/// anything: the worker config, the repo, the committer identity, the base branch and the spec
/// file itself. Shared with `task_run::run_spec` (issue #194 review finding M-1) so a `--restart`
/// discard there happens only after all of this has been checked, the same as `run_with`
/// validates every start input before its own `discard_existing` call. `source_text`, `id` and
/// `title` are this snapshot's read of the spec file: a caller that discards an existing task
/// between resolving and starting resolves again after the discard and hands that snapshot to
/// [`run_spec_from`], since the file may have changed or vanished while a confirmation prompt
/// was pending (decision 184).
pub(crate) struct SpecStart {
    pub config: TaskConfig,
    pub repo_name: String,
    pub identity: Identity,
    pub base_branch: String,
    pub clone_source: String,
    pub source_text: String,
    pub id: String,
    pub title: String,
}

pub(crate) fn resolve_spec_start(opts: &SpecOptions) -> Result<SpecStart> {
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
    let source_text = pipeline::validate_spec_file(&opts.spec)?;
    let (id, title) = record::spec_id(&opts.spec)?;
    // Rendered here only as a check; the start renders it again from the same inputs.
    pipeline::spec_worker_prompt(&config, &id, &base_branch, &repo_name)?;
    let clone_source = opts
        .clone_source
        .clone()
        .unwrap_or_else(|| format!("https://github.com/{repo_name}.git"));
    Ok(SpecStart {
        config,
        repo_name,
        identity,
        base_branch,
        clone_source,
        source_text,
        id,
        title,
    })
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
    let start = resolve_spec_start(opts)?;
    disk::require(&GlobalConfig::load(config_dir)?, config_dir, &opts.disk)?;
    run_spec_from(
        config_dir,
        opts,
        start,
        backend,
        github,
        probe,
        out,
        warn,
        &|_| {},
    )
}

/// [`run_spec`], taking an already-resolved [`SpecStart`] instead of resolving one itself: a
/// caller that discards an existing task (`--restart`) first passes the snapshot it resolved
/// after the discard, so the task it starts and the id it reviews are the same one. `on_prepared`
/// is called with the task's id the moment its preparation succeeds, before its worker runs, so a
/// caller settling that task's set-aside old copy (decision 185(b)) does it right then, not after
/// the worker or its gates, which a later failure of either must not undo.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_spec_from(
    config_dir: &std::path::Path,
    opts: &SpecOptions,
    start: SpecStart,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    on_prepared: &dyn Fn(&str),
) -> Result<()> {
    let SpecStart {
        config,
        repo_name,
        identity,
        base_branch,
        clone_source,
        source_text,
        id: spec_id,
        title,
    } = start;
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
    let mut prepared =
        pipeline::prepare_spec_resolved(&ctx, &opts.spec, &source_text, &spec_id, &title)?;
    let id = prepared.record.id.clone();
    on_prepared(&id);
    for warning in &prepared.warnings {
        writeln!(warn, "warning: {warning}")?;
    }

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
        let first = gated
            .failed
            .clone()
            .expect("failed gates name the first failure");
        // A gate that could not run at all gives the review nothing to react to, so it is not
        // named as the next step (issue 186).
        match gated.failed_outcome().filter(|o| o.could_not_run) {
            Some(outcome) => {
                writeln!(
                    out,
                    "  gates: could not run: `{}` ({}); {}",
                    first.command, first.tier, outcome.output_tail
                )?;
                bail!(
                    "{id}: gates could not run: `{}` ({}); {}",
                    first.command,
                    first.tier,
                    outcome.output_tail
                )
            }
            None => {
                let exit = first
                    .exit
                    .map_or_else(|| "no exit code".to_owned(), |c| format!("exit {c}"));
                writeln!(
                    out,
                    "  gates: failed: `{}` ({}, {exit})",
                    first.command, first.tier
                )?;
                writeln!(
                    out,
                    "  next: sbxm task review {} (it reruns the gates and gives a failure to a \
                     fix round)",
                    spec_flag(&opts.spec)
                )?;
                // A gate that ran and failed goes on to the review, which reruns it and gives a
                // failure to a fix round (decision 180): `task run --spec` catches this typed
                // error to tell it apart from any other failure, the same way it does for
                // `--issue` (`GatesFailed`, defined above for the issue path).
                Err(GatesFailed(format!(
                    "{id}: gates failed: `{}` ({}, {exit}); next: sbxm task review {} (it reruns \
                     the gates and gives a failure to a fix round)",
                    first.command,
                    first.tier,
                    spec_flag(&opts.spec)
                ))
                .into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{only_gates_failed, path_arg};
    use crate::task::gates::GateOutcome;
    use crate::task::pipeline::Gated;
    use crate::task::record::GateResult;
    use std::path::Path;

    fn gate_result(passed: bool) -> GateResult {
        GateResult {
            phase: "after-worker".into(),
            tier: "sandbox".into(),
            command: "cargo test".into(),
            exit: if passed { Some(0) } else { Some(1) },
            passed,
        }
    }

    /// A one-gate [`Gated`]: `passed`, and (when it failed) whether it could not run at all.
    fn fake_gated(passed: bool, could_not_run: bool) -> Gated {
        let result = gate_result(passed);
        Gated {
            passed,
            failed: (!passed).then(|| result.clone()),
            outcomes: vec![GateOutcome {
                result,
                output_tail: "could not run the command: no such file\n".into(),
                timed_out: false,
                could_not_run,
            }],
        }
    }

    /// Review of PR 184, M-1 (decision 180): `task run` goes on to the review only after gates
    /// that ran and failed, never after gates that could not run or a failed worker. Issue 186:
    /// a gate command that could not be executed at all does not count either, even though
    /// `Gated::passed` is `false` for it the same as a gate that ran and failed.
    #[test]
    fn only_gates_that_ran_and_failed_after_a_working_worker_count_as_a_gate_failure() {
        let ran_and_failed = Ok(fake_gated(false, false));
        let could_not_run = Ok(fake_gated(false, true));
        let passed = Ok(fake_gated(true, false));
        let run_errored = Err(anyhow::anyhow!("no worker"));

        assert!(only_gates_failed(false, Some(&ran_and_failed)));
        assert!(
            !only_gates_failed(false, Some(&could_not_run)),
            "could not run"
        );
        assert!(!only_gates_failed(false, Some(&passed)), "passed");
        assert!(
            !only_gates_failed(false, Some(&run_errored)),
            "the gate run itself errored"
        );
        assert!(!only_gates_failed(false, None), "no gate run");
        assert!(
            !only_gates_failed(true, Some(&ran_and_failed)),
            "worker failed"
        );
    }

    /// What the platform's shell makes of `arg` as one argument: PowerShell on Windows (the
    /// primary platform), `sh` elsewhere.
    fn through_shell(arg: &str) -> String {
        let dir = tempfile::TempDir::new().unwrap();
        let out = if cfg!(windows) {
            let script = dir.path().join("echo.ps1");
            // A command argument, as `git fetch <arg>` gets it.
            let show = "function Show { [Console]::Out.Write($args[0]) }";
            std::fs::write(&script, format!("{show}\nShow {arg}\n")).unwrap();
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(&script)
                .output()
                .unwrap()
        } else {
            std::process::Command::new("sh")
                .args(["-c", &format!("printf %s {arg}")])
                .output()
                .unwrap()
        };
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// Review of PR 157, M-1: a printed command must give the shell back the exact path.
    #[test]
    fn a_printed_path_reaches_the_shell_unchanged() {
        for path in [
            "/tmp/plain/repo.git",
            "/tmp/a b/repo.git",
            "/tmp/it's/repo.git",
            "/tmp/a'$b/repo.git",
            "/tmp/a\"b'c/repo.git",
            "/tmp/a`b'c/repo.git",
        ] {
            let arg = path_arg(Path::new(path));
            assert_eq!(through_shell(&arg), path, "{arg}");
        }
    }
}
