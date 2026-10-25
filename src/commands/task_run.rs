//! `sbxm task run (--issue N | --spec FILE)` (spec §4, decision 158): `start` then `review` for
//! one task, and then it stops; `finish` publishes, so it stays a deliberate command of its own.
//! Gates that fail after the worker don't stop it: the review reruns them (decision 180).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::task_review;
use super::task_start::{self, Restart};
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::run::config::{check_model_flag, headless_harness, parse_duration};
use crate::task::config::TaskConfig;
use crate::task::finish;
use crate::task::gates::HostRunner;
use crate::task::pipeline;
use crate::task::record::{self, ProcessProbe};
use crate::task::repo::Identity;

/// The flags `start` and `review` share, and each one's own.
pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub issue: u32,
    pub worker_harness: Option<Harness>,
    pub worker_model: Option<String>,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    /// The worker's time limit, also for the fix round.
    pub time_limit: Option<String>,
    pub reviewer_time_limit: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub repo: Option<String>,
    /// For tests: what `repo.git` is cloned from.
    pub clone_source: Option<String>,
    /// For tests: the committer identity.
    pub identity: Option<Identity>,
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
    /// `--cloud` or `--local`; `None` leaves it to `[sandbox] location` (P14).
    pub location: Option<Location>,
}

/// The drive can fill up while the worker runs, so it is checked again before the reviewer's
/// sandbox (decision 194). A refusal names `task review`, which goes on from here: `task run`
/// would not pick the task up again. Returns the guard to hand the review, which has nothing left
/// to check: its own refusal would name no way to continue.
fn review_has_room(config_dir: &Path, guard: &disk::Guard, flag: &str) -> Result<disk::Guard> {
    disk::require(&GlobalConfig::load(config_dir)?, config_dir, guard).with_context(|| {
        format!(
            "the worker is done but its review was not started; continue with `sbxm task review \
             {flag}`"
        )
    })?;
    Ok(disk::Guard {
        ignore: true,
        ..guard.clone()
    })
}

/// Drops the `next:` hint of a step that isn't the last one.
fn without_next(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .lines()
        .filter(|line| !line.trim_start().starts_with("next:"))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Passes each line on once: `start` and `review` both say the config's warnings.
struct Once<'a> {
    inner: &'a mut dyn Write,
    seen: Vec<Vec<u8>>,
    line: Vec<u8>,
}

impl Write for Once<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &byte in buf {
            self.line.push(byte);
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                if !self.seen.contains(&line) {
                    self.inner.write_all(&line)?;
                    self.seen.push(line);
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    config_dir: &Path,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if restart.is_none() {
        return run_issue(
            config_dir, opts, None, backend, github, probe, host, out, warn,
        );
    }
    // As in `task_start::run_with`: a restart's screen output can't stop it on the way.
    let (mut out, mut warn) = (finish::Unfailing::new(out), finish::Unfailing::new(warn));
    let result = run_issue(
        config_dir, opts, restart, backend, github, probe, host, &mut out, &mut warn,
    );
    finish::after_output(result, [out.into_error(), warn.into_error()])
}

#[allow(clippy::too_many_arguments)]
fn run_issue(
    config_dir: &Path,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    // The review's flags are checked now, not after the worker has spent its time.
    if let Some(harness) = opts.reviewer_harness {
        headless_harness("--reviewer-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
    }
    if let Some(model) = &opts.reviewer_model {
        check_model_flag("--reviewer-model", model).map_err(|e| anyhow!(e))?;
    }
    if let Some(model) = &opts.worker_model {
        check_model_flag("--worker-model", model).map_err(|e| anyhow!(e))?;
    }
    if let Some(limit) = &opts.reviewer_time_limit {
        parse_duration("--reviewer-time-limit", limit).map_err(|e| anyhow!(e))?;
    }

    // The reviewer's secret and profile are checked now too: a missing one must not be found only
    // after the worker has spent its time (spec §5.2). The effective config is the one both
    // phases will build from their flags.
    {
        let mut config = TaskConfig::load(&opts.repo_root)?;
        pipeline::refuse_cloud(
            pipeline::choose_location(opts.location, &config),
            "`task run`",
            "set [sandbox] location = \"local\" in sbxm-task.toml to run it here",
        )?;
        if let Some(harness) = opts.worker_harness {
            let harness = headless_harness("--worker-harness", harness.as_str(), "tasks")
                .map_err(|e| anyhow!(e))?;
            config.set_worker_harness(harness);
        }
        if let Some(harness) = opts.reviewer_harness {
            let harness = headless_harness("--reviewer-harness", harness.as_str(), "tasks")
                .map_err(|e| anyhow!(e))?;
            config.set_reviewer_harness(harness);
        }
        if let Some(profile) = &opts.profile {
            config.sandbox.profile = profile.clone();
        }
        disk::require(&GlobalConfig::load(config_dir)?, config_dir, &opts.disk)?;
        pipeline::check_reviewer_inputs(config_dir, backend, &config)?;
    }

    let start = task_start::Options {
        repo_root: opts.repo_root.clone(),
        issues: vec![opts.issue],
        workers: None,
        worker_harness: opts.worker_harness,
        worker_model: opts.worker_model.clone(),
        time_limit: opts.time_limit.clone(),
        profile: opts.profile.clone(),
        base: opts.base.clone(),
        repo: opts.repo.clone(),
        clone_source: opts.clone_source.clone(),
        identity: opts.identity.clone(),
        disk: opts.disk.clone(),
        location: None,
    };
    let mut warn = Once {
        inner: warn,
        seen: Vec::new(),
        line: Vec::new(),
    };
    let mut started = Vec::new();
    let result = task_start::run_with(
        config_dir,
        &start,
        restart,
        backend,
        github,
        probe,
        &mut started,
        &mut warn,
    );
    out.write_all(without_next(&started).as_bytes())?;
    // Gates that failed after the worker go on to the review, which reruns them and gives a
    // failure to a fix round (decision 180); every other failure stops here.
    if let Err(e) = result
        && !e.is::<task_start::GatesFailed>()
    {
        return Err(e);
    }

    let disk = review_has_room(config_dir, &opts.disk, &format!("--issue {}", opts.issue))?;
    task_review::run(
        config_dir,
        &task_review::Options {
            repo_root: opts.repo_root.clone(),
            target: task_review::Target::Issue(opts.issue),
            repo: opts.repo.clone(),
            base: opts.base.clone(),
            clone_source: opts.clone_source.clone(),
            reviewer_harness: opts.reviewer_harness,
            reviewer_model: opts.reviewer_model.clone(),
            reviewer_time_limit: opts.reviewer_time_limit.clone(),
            time_limit: opts.time_limit.clone(),
            profile: opts.profile.clone(),
            resolved_spec_id: None,
            force: false,
            disk,
            // A task that exists runs where its record says (decision 253).
            location: None,
            identity: opts.identity.clone(),
        },
        backend,
        github,
        probe,
        host,
        out,
        &mut warn,
    )
}

/// `task run --spec <FILE>` (decision 174(d), issue 194): `start --spec` then `review --spec` for
/// one spec task, stopping at `ready`. No GitHub call is made anywhere in this path.
pub struct SpecOptions {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub spec: PathBuf,
    pub worker_harness: Option<Harness>,
    pub worker_model: Option<String>,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    /// The worker's time limit, also for the fix round.
    pub time_limit: Option<String>,
    pub reviewer_time_limit: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub repo: Option<String>,
    /// For tests: what `repo.git` is cloned from.
    pub clone_source: Option<String>,
    /// For tests: the committer identity.
    pub identity: Option<Identity>,
    /// The free-space check of the sandbox drive before a sandbox is created (decision 194).
    pub disk: disk::Guard,
    /// `--cloud` or `--local`; `None` leaves it to `[sandbox] location` (P14).
    pub location: Option<Location>,
}

/// Sets the spec task of `spec` aside, if one exists, instead of deleting it (decision 185): the
/// caller decides, once it knows whether the new one's preparation completed, whether to put it
/// back or delete it for good. Everything is checked and shown before it is moved: no leftover
/// from an earlier, unsettled `--restart` ([`finish::check_no_leftover`]), and a finished result
/// that lives only in the task's `repo.git` (not yet fetched into `repo_root`, decision 178) is
/// protected the same way `task rm --spec` protects it, since `--restart` has no `--force` of its
/// own; fetch the result first, or run `sbxm task rm --spec FILE --force` before restarting.
fn discard_existing_spec(
    config_dir: &Path,
    repo_root: &Path,
    spec: &Path,
    restart: &Restart,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<Option<finish::SetAside>> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let (id, _) = record::spec_id(spec)?;
    finish::check_no_leftover(&base_dir, &id)?;
    if !finish::exists(&base_dir, &id) {
        return Ok(None);
    }
    finish::check_spec_result_fetched(&base_dir, &id, repo_root)?;
    let aside = finish::set_aside(
        &base_dir,
        &id,
        restart.yes,
        backend,
        probe,
        restart.confirm,
        out,
    )?;
    Ok(Some(aside))
}

/// [`run_spec`], optionally discarding an existing spec task first (`--restart`, as for `--issue`).
#[allow(clippy::too_many_arguments)]
pub fn run_spec(
    config_dir: &Path,
    opts: &SpecOptions,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if restart.is_none() {
        return run_spec_task(
            config_dir, opts, None, backend, github, probe, host, out, warn,
        );
    }
    // As in `task_start::run_with`: a restart's screen output can't stop it on the way.
    let (mut out, mut warn) = (finish::Unfailing::new(out), finish::Unfailing::new(warn));
    let result = run_spec_task(
        config_dir, opts, restart, backend, github, probe, host, &mut out, &mut warn,
    );
    finish::after_output(result, [out.into_error(), warn.into_error()])
}

#[allow(clippy::too_many_arguments)]
fn run_spec_task(
    config_dir: &Path,
    opts: &SpecOptions,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    // The review's flags are checked now, not after the worker has spent its time.
    if let Some(harness) = opts.reviewer_harness {
        headless_harness("--reviewer-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
    }
    if let Some(model) = &opts.reviewer_model {
        check_model_flag("--reviewer-model", model).map_err(|e| anyhow!(e))?;
    }
    if let Some(model) = &opts.worker_model {
        check_model_flag("--worker-model", model).map_err(|e| anyhow!(e))?;
    }
    if let Some(limit) = &opts.reviewer_time_limit {
        parse_duration("--reviewer-time-limit", limit).map_err(|e| anyhow!(e))?;
    }

    // As for `--issue`: the reviewer's secret and profile are checked now too, before the worker
    // has spent its time (spec §5.2).
    {
        let mut config = TaskConfig::load(&opts.repo_root)?;
        pipeline::refuse_cloud(
            pipeline::choose_location(opts.location, &config),
            "`task run`",
            "set [sandbox] location = \"local\" in sbxm-task.toml to run it here",
        )?;
        if let Some(harness) = opts.worker_harness {
            let harness = headless_harness("--worker-harness", harness.as_str(), "tasks")
                .map_err(|e| anyhow!(e))?;
            config.set_worker_harness(harness);
        }
        if let Some(harness) = opts.reviewer_harness {
            let harness = headless_harness("--reviewer-harness", harness.as_str(), "tasks")
                .map_err(|e| anyhow!(e))?;
            config.set_reviewer_harness(harness);
        }
        if let Some(profile) = &opts.profile {
            config.sandbox.profile = profile.clone();
        }
        disk::require(&GlobalConfig::load(config_dir)?, config_dir, &opts.disk)?;
        pipeline::check_reviewer_inputs(config_dir, backend, &config)?;
    }

    let start = task_start::SpecOptions {
        repo_root: opts.repo_root.clone(),
        spec: opts.spec.clone(),
        worker_harness: opts.worker_harness,
        worker_model: opts.worker_model.clone(),
        time_limit: opts.time_limit.clone(),
        profile: opts.profile.clone(),
        base: opts.base.clone(),
        repo: opts.repo.clone(),
        clone_source: opts.clone_source.clone(),
        identity: opts.identity.clone(),
        disk: opts.disk.clone(),
        location: None,
    };
    // The same preflight `task_start::run_spec` itself runs, so a bad --time-limit, repo,
    // identity, base or spec file is caught here too: `--restart`'s discard below must not cost
    // the old task its result over an input that was always going to be rejected (issue #194
    // review finding M-1).
    let mut resolved = task_start::resolve_spec_start(&start)?;
    // The worker's secret, checked the same way the reviewer's already is above, before anything
    // is discarded: a missing one must not be found only after the old task is gone (issue #194
    // review finding M-2).
    pipeline::check_worker_inputs(config_dir, backend, &resolved.config)?;

    let mut aside: Option<finish::SetAside> = None;
    if let Some(restart) = restart {
        aside = discard_existing_spec(
            config_dir,
            &opts.repo_root,
            &opts.spec,
            restart,
            backend,
            probe,
            out,
        )?;
        if let Some(aside) = &aside {
            // The resolution above was the preflight only. The spec file could have changed or
            // vanished while the confirmation prompt was pending, so it is resolved again and the
            // start fails here if it no longer names the same task (decision 184): a task started
            // under the old id would be one `task finish|resume|rm --spec FILE` cannot address.
            // Either way the old task, set aside rather than deleted (decision 185), is put back:
            // its preparation never got far enough to need it gone for good.
            let discarded = resolved.id.clone();
            let refreshed = task_start::resolve_spec_start(&start)
                .context("the old task was discarded, but the new one could not be started")
                .and_then(|fresh| {
                    if fresh.id == discarded {
                        Ok(fresh)
                    } else {
                        Err(anyhow!(
                            "the old task was discarded, but the new one could not be started: \
                             {} now names another task ({}, was {discarded})",
                            opts.spec.display(),
                            fresh.id
                        ))
                    }
                });
            match refreshed {
                Ok(fresh) => resolved = fresh,
                Err(e) => return Err(finish::restore_after_failed_start(aside, e)),
            }
        }
    }
    // The composed review below opens the task this snapshot starts, not one whose id is derived
    // from `opts.spec` again.
    let spec_id = resolved.id.clone();

    let mut warn = Once {
        inner: warn,
        seen: Vec::new(),
        line: Vec::new(),
    };
    // Settled the moment the new task's preparation succeeds (decision 185(b)), before its worker
    // runs: `aside_cell` still holds it only if preparation itself fails, which is the only case
    // `run_spec_from` can return without ever calling `on_prepared`.
    let aside_cell = std::cell::RefCell::new(aside);
    let settle_warning = std::cell::RefCell::new(None);
    let on_prepared = |_id: &str| {
        let Some(aside) = aside_cell.borrow_mut().take() else {
            return;
        };
        if let Err(e) = aside.settle() {
            *settle_warning.borrow_mut() = Some(format!(
                "the old task's set-aside copy at {} could not be deleted: {e:#}",
                aside.folder().display()
            ));
        }
    };
    let mut started = Vec::new();
    let result = task_start::run_spec_from(
        config_dir,
        &start,
        resolved,
        backend,
        github,
        probe,
        &mut started,
        &mut warn,
        &on_prepared,
        &std::thread::sleep,
        // `task run --spec` refuses the cloud before this (decision 261), so no scoped secrets.
        &[],
    );
    out.write_all(without_next(&started).as_bytes())?;

    if let Some(warning) = settle_warning.into_inner() {
        writeln!(warn, "warning: {warning}")?;
    }
    // Still holding its `SetAside` means preparation itself failed (the only way `run_spec_from`
    // can return without `on_prepared` ever running), so the old task goes back; a later worker
    // or gate failure never reaches here, since `on_prepared` already settled it for good.
    if let Some(aside) = aside_cell.into_inner() {
        let start_failed = match &result {
            Err(e) => anyhow!("{e:#}"),
            Ok(()) => unreachable!("preparation failed, so run_spec_from cannot have succeeded"),
        };
        return Err(finish::restore_after_failed_start(&aside, start_failed));
    }

    // Gates that failed after the worker go on to the review, which reruns them and gives a
    // failure to a fix round (decision 180); every other failure stops here.
    if let Err(e) = result
        && !e.is::<task_start::GatesFailed>()
    {
        return Err(e);
    }

    let disk = review_has_room(config_dir, &opts.disk, &task_start::spec_flag(&opts.spec))?;
    task_review::run(
        config_dir,
        &task_review::Options {
            repo_root: opts.repo_root.clone(),
            target: task_review::Target::Spec(opts.spec.clone()),
            repo: opts.repo.clone(),
            base: opts.base.clone(),
            clone_source: opts.clone_source.clone(),
            reviewer_harness: opts.reviewer_harness,
            reviewer_model: opts.reviewer_model.clone(),
            reviewer_time_limit: opts.reviewer_time_limit.clone(),
            time_limit: opts.time_limit.clone(),
            profile: opts.profile.clone(),
            resolved_spec_id: Some(spec_id),
            force: false,
            disk,
            // A task that exists runs where its record says (decision 253).
            location: None,
            identity: opts.identity.clone(),
        },
        backend,
        github,
        probe,
        host,
        out,
        &mut warn,
    )
}

/// One target of a `task run` list (decision 193): an issue or a spec file, each run to its own
/// `ready` or its own stop, in the order they're written.
#[derive(Debug, Clone)]
pub enum Target {
    Issue(u32),
    Spec(PathBuf),
}

impl Target {
    /// `--issue N` or `--spec FILE`, as a retry hint or the summary line names it.
    fn flag(&self) -> String {
        match self {
            Target::Issue(n) => format!("--issue {n}"),
            Target::Spec(path) => task_start::spec_flag(path),
        }
    }

    /// The task id this target names, when it can be told without a backend call: `issue-<n>`
    /// always, a spec's own derived id only once its file resolves to one. Used only to read the
    /// task's record back for the summary line; a spec file that never resolved to an id has no
    /// record to read either way.
    fn id(&self) -> Option<String> {
        match self {
            Target::Issue(n) => Some(format!("issue-{n}")),
            Target::Spec(path) => record::spec_id(path).ok().map(|(id, _)| id),
        }
    }
}

/// The flags every task of a `task run` list shares (decision 193): the same as [`Options`] and
/// [`SpecOptions`] minus the one target each task carries on its own.
pub struct ListOptions {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub worker_harness: Option<Harness>,
    pub worker_model: Option<String>,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    /// The worker's time limit, also for the fix round.
    pub time_limit: Option<String>,
    pub reviewer_time_limit: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub repo: Option<String>,
    /// For tests: what `repo.git` is cloned from.
    pub clone_source: Option<String>,
    /// For tests: the committer identity.
    pub identity: Option<Identity>,
    /// The free-space check of the sandbox drive, read again before every task of the list, not
    /// only the first (decision 193 point 2): a task's own sandbox can fill the drive the next
    /// one needs.
    pub disk: disk::Guard,
    /// `--cloud` or `--local`; `None` leaves it to `[sandbox] location` (P14).
    pub location: Option<Location>,
}

impl ListOptions {
    fn for_issue(&self, issue: u32) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issue,
            worker_harness: self.worker_harness,
            worker_model: self.worker_model.clone(),
            reviewer_harness: self.reviewer_harness,
            reviewer_model: self.reviewer_model.clone(),
            time_limit: self.time_limit.clone(),
            reviewer_time_limit: self.reviewer_time_limit.clone(),
            profile: self.profile.clone(),
            base: self.base.clone(),
            repo: self.repo.clone(),
            clone_source: self.clone_source.clone(),
            identity: self.identity.clone(),
            disk: self.disk.clone(),
            location: self.location,
        }
    }

    fn for_spec(&self, spec: PathBuf) -> SpecOptions {
        SpecOptions {
            repo_root: self.repo_root.clone(),
            spec,
            worker_harness: self.worker_harness,
            worker_model: self.worker_model.clone(),
            reviewer_harness: self.reviewer_harness,
            reviewer_model: self.reviewer_model.clone(),
            time_limit: self.time_limit.clone(),
            reviewer_time_limit: self.reviewer_time_limit.clone(),
            profile: self.profile.clone(),
            base: self.base.clone(),
            repo: self.repo.clone(),
            clone_source: self.clone_source.clone(),
            identity: self.identity.clone(),
            disk: self.disk.clone(),
            location: self.location,
        }
    }
}

/// The command's next step for a task at `record`'s stage, as the summary line names it: the
/// same wording `task review` itself prints when a task reaches `ready` (decision 193).
fn next_command(record: &record::Record, flag: &str) -> String {
    match record.stage {
        record::Stage::Ready => match record.review.as_ref().map_or(0, |r| r.must_fix) {
            0 => format!("sbxm task finish {flag}"),
            _ => format!("fix them by hand, or file them: sbxm task file-findings {flag}"),
        },
        record::Stage::Finished => "already finished".to_owned(),
        _ => "see `sbxm task status` and the task's folder for what it left".to_owned(),
    }
}

/// The target's task record, read fresh; `None` when it names no id yet (a spec file that hasn't
/// resolved to one) or the record can't be read.
fn read_record(config_dir: &Path, target: &Target) -> Option<record::Record> {
    let id = target.id()?;
    let base_dir = GlobalConfig::load(config_dir).ok()?.base_dir;
    record::read(&record::task_dir(&base_dir, &id).join("task.json")).ok()
}

/// The summary line for one target once its run has finished (decision 193): its stage, its
/// must-fix findings left, and the next command, read back from its record so it reflects what
/// actually happened rather than the error text alone; a target whose preparation never reserved
/// a task folder (its own input was refused before anything was created) has no record to read,
/// so the line falls back to that refusal. `pre_record` is that same read taken *before* this
/// target's run, so a refusal that never touched an already-existing task (issue 201 review S-1,
/// e.g. `task issue-N exists`) is told apart from a worker or review failure that did advance the
/// task: the record is identical either way, but only the latter is this run's own doing.
fn summary_line(
    config_dir: &Path,
    target: &Target,
    pre_record: Option<&record::Record>,
    result: &Result<()>,
) -> (String, bool) {
    let flag = target.flag();
    let record = read_record(config_dir, target);
    let unchanged = matches!((&record, pre_record), (Some(r), Some(p)) if r == p);
    let line = match (&record, result) {
        (Some(_), Err(e)) if unchanged => format!("{flag}: not started: {e:#}"),
        (Some(record), _) => {
            let must_fix = record
                .review
                .as_ref()
                .map_or_else(|| "-".to_owned(), |r| r.must_fix.to_string());
            format!(
                "{flag}: stage {}, must-fix {must_fix}; next: {}",
                record.stage.name(),
                next_command(record, &flag)
            )
        }
        (None, Err(e)) => format!("{flag}: not started: {e:#}"),
        // A task that reached `ready` always has a record; this only guards against the record
        // becoming unreadable between the run finishing and this line reading it back (self-
        // review: don't panic on a race this function can't rule out by construction).
        (None, Ok(())) => format!("{flag}: ready (its record could not be read back)"),
    };
    (line, result.is_ok())
}

/// Whether `e` is one of decision 193(a)'s three list-wide stops that aren't the free-disk check
/// (issue 201 review M-2, repeat of M-2): a missing shared secret, `sbx` itself not answering a
/// preflight call (`check_secrets`), or `SandboxBackend::exec` itself failing while the worker,
/// fix round or reviewer ran (`headless::run`'s `ExecUnavailable`, which carries the same "`sbx`
/// couldn't run" meaning `SbxUnavailable` does, just found at a different point). Either stops the
/// whole list instead of being counted as this target's own failure and asked again for every
/// later one; a harness-reported failure or an ordinary failing gate is still `Ok` with a status,
/// not this, so it stays a per-task stop that the list goes on past.
fn queue_wide_stop(e: &anyhow::Error) -> bool {
    e.is::<pipeline::MissingSecret>()
        || e.is::<pipeline::SbxUnavailable>()
        || e.is::<crate::headless::ExecUnavailable>()
}

/// Checks every target's own identity before the first one's sandbox is created (decision 193,
/// issue 201 review M-1): an issue that doesn't exist, isn't open, already has a task, or names a
/// PR that can't be continued; a spec file that can't be read or already has a task. Skipped, not
/// an error, when the list's shared repo can't even be resolved (no `--repo` and no git origin):
/// each target then discovers that the same way a single `task run` always has, one at a time as
/// the loop below reaches it — decision 193 point 1 still applies to that shared failure, and the
/// existing per-target checks already catch it before that target's own sandbox.
fn preflight_targets(
    config_dir: &Path,
    opts: &ListOptions,
    targets: &[Target],
    github: &dyn GitHubBackend,
) -> Result<()> {
    let Ok(repo) = task_start::resolve_repo(opts.repo.as_deref(), &opts.repo_root) else {
        return Ok(());
    };
    // All of this batch's issue numbers are checked together (issue 201 review M-1), not one at a
    // time, so two targets related to each other are caught the same way an explicit `task start
    // --issue A --issue B` already catches it.
    let issue_numbers: Vec<u32> = targets
        .iter()
        .filter_map(|target| match target {
            Target::Issue(n) => Some(*n),
            Target::Spec(_) => None,
        })
        .collect();
    let issue_problems = if issue_numbers.is_empty() {
        HashMap::new()
    } else {
        pipeline::preflight_issues(config_dir, &repo, github, &issue_numbers)?
    };
    let mut problems = Vec::new();
    for target in targets {
        match target {
            Target::Issue(n) => {
                if let Some(reason) = issue_problems.get(n) {
                    problems.push(format!("{}: {reason}", target.flag()));
                }
            }
            Target::Spec(path) => {
                if let Err(e) = pipeline::preflight_spec(config_dir, path) {
                    problems.push(format!("{}: {e:#}", target.flag()));
                }
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "every target is checked before any of them starts:\n{}",
            problems.join("\n")
        );
    }
    Ok(())
}

/// `task run` over a list of `--issue`/`--spec` targets (decision 193): each one runs the same
/// `start` then `review` as a single `task run`, to its own `ready` or its own stop, before the
/// next one starts. The tasks are independent: a later one never gets an earlier one's work, and
/// a task that stops short of `ready` does not stop the others — only a problem that would hit
/// every task does (here, too little free disk, checked again before each one). A single-target
/// list runs through the unchanged single-task path below, so `--restart` still works there;
/// with more than one target it is refused; there is nothing left to show the user when several
/// tasks' old copies would be discarded at once.
#[allow(clippy::too_many_arguments)]
pub fn run_list(
    config_dir: &Path,
    opts: &ListOptions,
    targets: &[Target],
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if targets.is_empty() {
        bail!("say which tasks to run: --issue <n> or --spec <file> (repeatable)");
    }
    if targets.len() > 1 && restart.is_some() {
        bail!(
            "--restart isn't allowed with more than one task; run `sbxm task run --restart` on \
             one issue or spec at a time"
        );
    }
    if let [one] = targets {
        return match one {
            Target::Issue(n) => run(
                config_dir,
                &opts.for_issue(*n),
                restart,
                backend,
                github,
                probe,
                host,
                out,
                warn,
            ),
            Target::Spec(path) => run_spec(
                config_dir,
                &opts.for_spec(path.clone()),
                restart,
                backend,
                github,
                probe,
                host,
                out,
                warn,
            ),
        };
    }

    preflight_targets(config_dir, opts, targets, github)?;

    let mut summaries: Vec<(String, bool)> = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        writeln!(
            out,
            "task {} of {}: {}",
            i + 1,
            targets.len(),
            target.flag()
        )?;
        if let Err(e) = disk::require(&GlobalConfig::load(config_dir)?, config_dir, &opts.disk) {
            let reason = format!("{e:#}");
            writeln!(out, "{}: not started: {reason}", target.flag())?;
            summaries.push((format!("{}: not started: {reason}", target.flag()), false));
            for later in &targets[i + 1..] {
                summaries.push((
                    format!(
                        "{}: not started; the list stopped (see above)",
                        later.flag()
                    ),
                    false,
                ));
            }
            break;
        }

        let pre_record = read_record(config_dir, target);
        let result = match target {
            Target::Issue(n) => run(
                config_dir,
                &opts.for_issue(*n),
                None,
                backend,
                github,
                probe,
                host,
                out,
                warn,
            ),
            Target::Spec(path) => run_spec(
                config_dir,
                &opts.for_spec(path.clone()),
                None,
                backend,
                github,
                probe,
                host,
                out,
                warn,
            ),
        };
        // Decision 193(a) (issue 201 review M-2): a missing shared secret or `sbx` itself not
        // answering stops the whole list, rather than being asked again for every later target.
        let stop = result.as_ref().err().is_some_and(queue_wide_stop);
        summaries.push(summary_line(
            config_dir,
            target,
            pre_record.as_ref(),
            &result,
        ));
        if stop {
            for later in &targets[i + 1..] {
                summaries.push((
                    format!(
                        "{}: not started; the list stopped (see above)",
                        later.flag()
                    ),
                    false,
                ));
            }
            break;
        }
    }

    writeln!(out, "\nsummary:")?;
    for (line, _) in &summaries {
        writeln!(out, "  {line}")?;
    }

    let not_ready = summaries.iter().filter(|(_, ready)| !ready).count();
    if not_ready > 0 {
        bail!(
            "{not_ready} of {} task(s) did not reach ready; see the summary above",
            summaries.len()
        );
    }
    Ok(())
}
