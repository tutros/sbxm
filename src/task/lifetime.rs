//! How long a cloud sandbox of a task lives (milestone 4 slice 3; P5 (d), P6, P7; decisions 259,
//! 262, 263, 265): the TTL each role's sandbox is created with, and its removal, retried, when the
//! task stops.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use super::config::TaskConfig;
use super::record::{self, Record, SandboxRole};
use super::secrets;
use crate::backend::{Location, SandboxBackend, ScopedSecret, ttl_arg, whole_minutes};
use crate::run::results::now;

/// What every TTL adds to the time its sandbox is expected to need (decision 259).
const MARGIN: Duration = Duration::from_secs(30 * 60);

/// The worker sandbox lives through the task's automatic run (P5 (d), decision 263): the worker,
/// its sandbox gates, and `fix_rounds` times a review, a fix round and the gates again, plus the
/// margin, rounded up to whole minutes as `sbx` takes it. `fix_rounds` is the task's budget: the
/// config's at its start, then the one its record holds (with any `--rounds N` added), never the
/// config's again (PR 322 review M-2). Refused when the limits are too large to add up.
pub fn worker_ttl(config: &TaskConfig, fix_rounds: u32) -> Result<Duration> {
    let worker = config.worker.time_limit;
    let sum = || {
        let gates = config
            .gates
            .timeout
            .checked_mul(u32::try_from(config.gates.sandbox.len()).ok()?)?;
        let round = config
            .reviewer
            .time_limit
            .checked_add(worker)?
            .checked_add(gates)?;
        worker
            .checked_add(gates)?
            .checked_add(round.checked_mul(fix_rounds)?)?
            .checked_add(MARGIN)
    };
    sum().and_then(rounded).with_context(|| {
        format!(
            "the worker sandbox's TTL ([worker] time_limit, plus every [gates] sandbox command's \
             timeout, plus {fix_rounds} fix rounds times the reviewer's, the worker's and the \
             gates' time, plus 30 minutes) is too large to add up"
        )
    })
}

/// [`worker_ttl`], its refusal ending with `fix`, the way out for the caller's case: only a new
/// task reads `[worker] fix_rounds` (PR 322 review S-1).
pub fn worker_ttl_or(
    config: &TaskConfig,
    fix_rounds: u32,
    fix: impl FnOnce() -> String,
) -> Result<Duration> {
    worker_ttl(config, fix_rounds).map_err(|e| anyhow!("{e:#}; {}", fix()))
}

/// The way out of a new task's [`worker_ttl`] refusal: its budget is still the config's.
pub fn new_task_fix() -> String {
    "lower those limits or [worker] fix_rounds in sbxm-task.toml".to_owned()
}

/// A reviewer sandbox lives for one review round (P5 (d)): the reviewer's time limit plus the
/// margin, rounded up to whole minutes.
pub fn reviewer_ttl(config: &TaskConfig) -> Result<Duration> {
    config
        .reviewer
        .time_limit
        .checked_add(MARGIN)
        .and_then(rounded)
        .with_context(|| {
            "the reviewer sandbox's TTL ([reviewer] time_limit plus 30 minutes) is too large; \
             lower [reviewer] time_limit in sbxm-task.toml"
        })
}

/// `ttl` rounded up to whole minutes; `None` past what a `Duration` holds.
fn rounded(ttl: Duration) -> Option<Duration> {
    whole_minutes(ttl).checked_mul(60).map(Duration::from_secs)
}

/// The waits before each retry of a removal that `sbx` says to retry (P6 (a), decision 262): up
/// to 3 retries after the first attempt.
const REMOVAL_WAITS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];

/// The one removal error worth retrying (S14 Q5's wording, decision 267).
const RETRYABLE: &str = "temporarily unavailable";

/// How a cloud sandbox's removal ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// `sbx` didn't know the sandbox (`sbx-cloud-rm-not-found.txt`): it counts as removed.
    AlreadyGone,
    /// The last error, after any retries: the sandbox may still be there.
    Failed(String),
}

/// Removes the sandbox `name` (`sbx rm <name> -f` on the backend's instance), retrying only an
/// error that says it is temporarily unavailable, after 5, 10 and 20 seconds through `sleep`;
/// any other error ends it at once. Never fails: the caller reports what it returns.
pub fn remove_sandbox(
    backend: &dyn SandboxBackend,
    name: &str,
    sleep: &dyn Fn(Duration),
) -> Removal {
    let mut waits = REMOVAL_WAITS.iter();
    loop {
        let error = match backend.remove(name) {
            Ok(()) => return Removal::Removed,
            Err(e) => format!("{e:#}"),
        };
        if error.contains(&format!("sandbox '{name}' not found")) {
            return Removal::AlreadyGone;
        }
        match waits.next() {
            Some(wait) if error.contains(RETRYABLE) => sleep(*wait),
            _ => return Removal::Failed(error),
        }
    }
}

/// The line the screen and `run.log` show at a cloud create (decision 265).
pub fn created_line(name: &str, role: SandboxRole, ttl: Duration) -> String {
    format!(
        "cloud sandbox {name} created ({}, TTL {})",
        role.name(),
        ttl_arg(ttl)
    )
}

/// The warning for a cloud sandbox whose removal still failed: never dropped silently (P6 (a)),
/// and naming the command that removes it by hand.
pub fn not_removed(name: &str, error: &str) -> String {
    format!(
        "cloud sandbox {name} could not be removed: {error}; remove it with `sbx --cloud rm \
         {name} -f`"
    )
}

/// What [`release_worker`] has to say, for the screen and so `run.log` (decision 265).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Released {
    /// The sandbox is gone (removed, or already gone).
    Line(String),
    /// It may still be there, or its removal couldn't be recorded.
    Warning(String),
}

/// Everything [`release_worker`] has to say, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stopped {
    /// The sandbox's removal: `None` for a local task, or when the sandbox has no entry still
    /// open (it was never created, or is already removed).
    pub sandbox: Option<Released>,
    /// The task's scoped secrets, removed after the sandbox (slice 4, decision 256): one entry
    /// per secret the cloud listed, or one warning for a list it wouldn't give; empty for a task
    /// without scoped secrets, whose list is never read.
    pub secrets: Vec<Released>,
}

impl Stopped {
    /// The sandbox's outcome, then each secret's.
    pub fn all(&self) -> impl Iterator<Item = &Released> {
        self.sandbox.iter().chain(self.secrets.iter())
    }
}

/// The task stops, ready or for any reason: its cloud worker sandbox goes now (P5 (d), decision
/// 263), with the removal retry, and its lifetime entry is stamped and saved; then every secret
/// scoped to it, when the task has `scoped_secrets` (slice 4), whatever the sandbox's removal
/// said: the cloud's `rm` leaves them behind, and a sandbox that couldn't be removed shouldn't
/// keep them either. The one function every "the task stops" path calls.
pub fn release_worker(
    backend: &dyn SandboxBackend,
    meta: &Path,
    task: &mut Record,
    sleep: &dyn Fn(Duration),
    scoped_secrets: &[ScopedSecret],
) -> Stopped {
    match task.worker.as_ref().map(|w| w.sandbox.clone()) {
        Some(name) => release(backend, meta, task, &name, sleep, scoped_secrets),
        None => Stopped::default(),
    }
}

/// [`release_worker`] for any of a cloud task's sandboxes, by name (a reviewer's after its round,
/// milestone 4 slice 5): removed, retried, when it has an entry still open, then every secret
/// scoped to it when the task has `scoped_secrets`. Nothing for a local task.
pub fn release(
    backend: &dyn SandboxBackend,
    meta: &Path,
    task: &mut Record,
    name: &str,
    sleep: &dyn Fn(Duration),
    scoped_secrets: &[ScopedSecret],
) -> Stopped {
    let mut stopped = Stopped::default();
    if task.location != Location::Cloud {
        return stopped;
    }
    if task.has_open_sandbox(name) {
        stopped.sandbox = Some(remove_entry(backend, meta, task, name, sleep));
    }
    if !scoped_secrets.is_empty() {
        stopped.secrets = secrets::remove_all(backend, name);
    }
    stopped
}

/// A command that acts on a cloud task after its run (`task finish`, milestone 4 slice 6) removes
/// each of `names`, its sandboxes still open in the record: retried, its entry stamped and saved,
/// then every secret the cloud lists as scoped to it, by scope (slice 4: the command has no list
/// of its own). `backend` is the cloud instance. Never fails: the caller shows what it says.
pub fn release_open(
    backend: &dyn SandboxBackend,
    meta: &Path,
    task: &mut Record,
    names: &[String],
    sleep: &dyn Fn(Duration),
) -> Vec<Released> {
    let mut said = Vec::new();
    for name in names {
        if task.has_open_sandbox(name) {
            said.push(remove_entry(backend, meta, task, name, sleep));
        }
        said.extend(secrets::remove_all(backend, name));
    }
    said
}

/// A sandbox's removal, retried, and its lifetime entry stamped and saved.
fn remove_entry(
    backend: &dyn SandboxBackend,
    meta: &Path,
    task: &mut Record,
    name: &str,
    sleep: &dyn Fn(Duration),
) -> Released {
    let how = match remove_sandbox(backend, name, sleep) {
        Removal::Removed => "removed",
        Removal::AlreadyGone => "already gone",
        Removal::Failed(error) => return Released::Warning(not_removed(name, &error)),
    };
    task.sandbox_removed(name, now());
    match record::write(meta, task) {
        Ok(()) => Released::Line(format!("cloud sandbox {name} {how}")),
        Err(e) => Released::Warning(format!(
            "cloud sandbox {name} {how}, but task.json couldn't record it: {e:#}"
        )),
    }
}
