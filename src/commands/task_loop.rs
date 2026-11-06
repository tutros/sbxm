//! `sbxm task loop [--cap N] [--issue N ...]` (issue 320, decision 245 (b) to (h)): works through
//! the open issues labeled `ready-for-worker` one task at a time, end to end: `task run`, then
//! `task finish` (a draft PR for a work stop), then `task file-findings --create` for a clean
//! task. It pauses while 5 PRs await the user or the disk is low, resumes a task once after an
//! environment stop and parks it after a second, waits out a usage limit, and stops on an empty
//! queue (with a read-only report), two parked tasks in a row, `--cap`, the usage waits' total,
//! or a stop request between phases. One log row per task goes to
//! `sdlc/batches/<date>-loop.md`. Before each new issue it resolves the open PRs it opened that
//! GitHub reports conflicting with their base, with `task fix --pr N` (decision 298), then reads
//! the workflow runs of its PRs and of `main` (decision 299): a failed run is re-run once; a
//! second failure disarms the PR, or on `main` sets the pause that stops all arming until the
//! user runs `task loop --clear-pause`.
//!
//! The phases go through [`Steps`] so the loop's rules are tested without sandboxes;
//! [`TaskSteps`] runs the real commands.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::task_start::resolve_repo;
use super::{task_file_findings, task_finish, task_fix, task_log, task_resume, task_rm, task_run};
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::{GitHubBackend, Issue, Mergeable, PrState, RunStatus, WorkflowRun};
use crate::run::results::format_timestamp;
use crate::task::config::TaskConfig;
use crate::task::finish::check_own_folder;
use crate::task::gates::HostRunner;
use crate::task::loop_pause::{self, Pause};
use crate::task::queue::{self, READY_LABEL, Report, Row, Skipped};
use crate::task::record::{self, Kind, Process, ProcessProbe, Record, Stage};
use crate::task::risk::Level;
use crate::task::runlog::{RunLog, Tee};
use crate::task::{findings, lock, pipeline};

/// How many open PRs the loop opened may await the user before it pauses (decision 245(e)).
pub const PR_CAP: usize = 5;
pub use queue::LOOP_HOOK;
/// How often a pause looks again.
pub const PAUSE_POLL: Duration = Duration::from_secs(5 * 60);
/// How often a task's first phase is looked at for its record, to take its label off.
const LABEL_POLL: Duration = Duration::from_secs(2);

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is and the log is written.
    pub repo_root: PathBuf,
    /// `--issue N`, in the user's order; empty means the `ready-for-worker` queue.
    pub issues: Vec<u32>,
    /// `--cap N`: at most this many tasks.
    pub cap: Option<u32>,
    /// For tests: the GitHub repo, `owner/name`, instead of the checkout's origin.
    pub repo: Option<String>,
}

/// One task's phases, as the loop runs them. Each writes its own screen output.
pub trait Steps {
    /// `task run --issue N`: start, gates, review and fix rounds, to `ready` or a stop.
    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task resume --issue N`: the task again from its recorded stage, to `ready`.
    fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task finish --issue N`: its PR, a draft when something is left unresolved.
    fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task file-findings --issue N --create`: the review's should-fix findings as sized
    /// issues, without the loop's label; nothing when the review has none.
    fn file_findings(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task fix --pr N`: the base merged into the PR's branch in a sandbox, the gates and a
    /// narrow review, then the push (decisions 292, 298).
    fn fix_pr(&self, pr: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task rm --merged --yes`: removes the tasks whose PRs GitHub merged, archiving their text
    /// records first (decisions 195, 293).
    fn rm_merged(&self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// The free-disk check of `sbx`'s sandbox drive: an error while it is under `min_free_disk`.
    fn room(&self) -> Result<()>;
}

/// The clock, the waiting and the user's stop request, injected so tests don't wait.
pub struct Env<'a> {
    /// Seconds since the epoch.
    pub now: &'a dyn Fn() -> u64,
    pub sleep: &'a dyn Fn(Duration),
    /// Asked between phases: `true` ends the loop there.
    pub stop_requested: &'a dyn Fn() -> bool,
}

/// What a Ctrl-C press means (the user's answer, 2026-11-02).
#[derive(Debug, PartialEq, Eq)]
pub enum Press {
    /// Stop after the current task.
    First,
    /// Stop now.
    Again,
}

/// The loop's stop request, set from the Ctrl-C handler `main` installs for `task loop`.
#[derive(Default)]
pub struct StopFlag(std::sync::atomic::AtomicU32);

impl StopFlag {
    /// Counts one press.
    pub fn press(&self) -> Press {
        use std::sync::atomic::Ordering;
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => Press::First,
            _ => Press::Again,
        }
    }

    /// Whether a stop was asked: what `Env::stop_requested` reads.
    pub fn requested(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst) > 0
    }
}

/// `<base>/.sbxm/task-loop.lock`: held while a loop runs; dropping it removes the file while it
/// still names this process.
struct LoopLock {
    path: PathBuf,
    me: String,
}

impl Drop for LoopLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|text| text == self.me) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Takes the loop's lock for this process, as `template build` takes its own (decision 212): a
/// lock whose process is alive refuses the second loop; one whose process is gone is taken over.
fn take_lock(base_dir: &Path, probe: &dyn ProcessProbe, warn: &mut dyn Write) -> Result<LoopLock> {
    let path = queue::lock_path(base_dir);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let me = serde_json::to_string(&Process::current(probe))?;
    for _ in 0..3 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let lock = LoopLock {
                    path,
                    me: me.clone(),
                };
                file.write_all(me.as_bytes())
                    .with_context(|| format!("cannot write {}", lock.path.display()))?;
                return Ok(lock);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("cannot create {}", path.display())),
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let held: Process = serde_json::from_str(&text).with_context(|| {
            format!(
                "cannot read the loop lock {}; if no `sbxm task loop` is running, delete it",
                path.display()
            )
        })?;
        if held.alive(probe) {
            bail!(
                "a `sbxm task loop` is already running (process {}, started {}); wait for it to \
                 stop, or stop it",
                held.pid,
                held.started_at
            );
        }
        // Removed only if it is still the dead loop's: a loop that took over meanwhile is live.
        if fs::read_to_string(&path).is_ok_and(|again| again == text) {
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            writeln!(
                warn,
                "took over the lock of a loop that is no longer running (process {}, started {})",
                held.pid, held.started_at
            )?;
        }
    }
    bail!(
        "cannot take the loop lock {}; another `sbxm task loop` may have started, try again",
        path.display()
    )
}

/// Passes output on and keeps a copy, so a usage limit the harness printed is seen too.
struct Capture<'a> {
    inner: &'a mut dyn Write,
    text: Vec<u8>,
}

impl Write for Capture<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.text.extend_from_slice(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// How one task ended, for its log row and the loop's stop rules.
enum End {
    /// Finished into a PR, a draft for a work stop.
    Done { ready: u64, outcome: String },
    /// Stopped short of `ready` twice, or its finish failed: left as it is for the user. `ready`:
    /// when it reached ready, for one whose finish failed.
    Parked { ready: Option<u64>, outcome: String },
    /// Refused before anything was written: not a task.
    Refused(String),
    /// Its lock is held by a live command run by hand (issue 409): left as it is, nothing
    /// written, and tried again after the loop's next task.
    Held(Retry),
    /// Finished or removed by a command run by hand while its lock was held (review M-1 of issue
    /// 409): that command's, so no row, not counted and not parked. The line saying why.
    Taken(String),
}

/// What a claim found under the task's lock.
enum Claimed {
    /// Marked as the loop's.
    Marked,
    /// Already finished, by a command run by hand: not marked. Its PR, when one is recorded.
    Finished(Option<String>),
}

/// The line for a task a command run by hand finished, into `pr` when one is recorded.
fn finished_by_hand(issue: u32, pr: Option<&str>) -> String {
    let into = pr.map_or_else(String::new, |url| format!(" into {url}"));
    format!("#{issue}: finished by a command run by hand{into}; the loop leaves it to that command")
}

/// Where a task whose lock was held goes on from when it is tried again.
#[derive(Clone, Copy)]
enum Retry {
    /// Its claim and `task finish`; `ready`: when it reached ready.
    Finish { ready: u64 },
    /// Its `task resume`, after this many failures in a row.
    Resume { failures: u32 },
}

/// A task left for a lock held by hand, with what its log row needs once it ends.
struct Held {
    issue: u32,
    title: String,
    size: Option<String>,
    started: u64,
    stops: Vec<(String, u64)>,
    retry: Retry,
}

impl Held {
    /// The line naming what is left when the loop stops before trying it again.
    fn left(&self) -> String {
        let issue = self.issue;
        let command = match self.retry {
            Retry::Finish { .. } => "finish",
            Retry::Resume { .. } => "resume",
        };
        format!(
            "#{issue} was not tried again after its lock was held; continue it with `sbxm task \
             {command} --issue {issue}`"
        )
    }
}

/// The line for a task whose `phase` was skipped because a live command holds its lock.
fn held_line(issue: u32, phase: &str, e: &anyhow::Error) -> String {
    let why = e
        .downcast_ref::<lock::InUse>()
        .map_or_else(|| format!("{e:#}"), lock::InUse::describe);
    format!(
        "#{issue}: its `task {phase}` is skipped for now: {why}; the loop tries it again after \
         its next task"
    )
}

/// Why the whole loop stops early.
enum Halt {
    /// A stop was asked before the task started: nothing to log.
    Stopped,
    UsageLimit(Duration),
    Error(anyhow::Error),
}

struct Loop<'a> {
    config_dir: &'a Path,
    base_dir: PathBuf,
    repo: String,
    settings: crate::task::config::Loop,
    steps: &'a dyn Steps,
    github: &'a dyn GitHubBackend,
    probe: &'a dyn ProcessProbe,
    env: &'a Env<'a>,
    /// The waits in a row for a usage limit (decision 6).
    usage_waited: Duration,
    /// Warnings already given, so a pause's polls don't repeat them.
    warned: HashSet<String>,
    /// Lines already said, so a skip seen again on the next pick isn't repeated.
    said: HashSet<String>,
    /// The (PR, base tip) pairs a resolve was tried for (decision 298): at most one each.
    resolves: HashSet<(u32, String)>,
    /// The PRs whose resolve failed: left to the user for the rest of the run.
    unresolved: HashSet<u32>,
    /// The PRs this run armed, watched for their merge commit (decision 299, answer 4).
    armed: HashSet<u32>,
    /// Merge commits of PRs this run armed whose runs are still to be read to their end.
    merges: Vec<(u32, String)>,
    /// The runs whose failed jobs this run re-ran, so a lagging answer isn't re-run twice.
    reruns: HashSet<u64>,
    /// The (PR, run) second failures already acted on: said once.
    failed_twice: HashSet<(u32, u64)>,
    /// The (commit, run) second failures on `main` already paused for (or found paused): a pause
    /// the user cleared isn't set again for the same run.
    paused_for: HashSet<(String, u64)>,
    log: PathBuf,
}

impl Loop<'_> {
    fn now(&self) -> u64 {
        (self.env.now)()
    }

    fn say_once(&mut self, out: &mut dyn Write, text: String) -> Result<()> {
        if self.said.insert(text.clone()) {
            writeln!(out, "{text}")?;
        }
        Ok(())
    }

    /// The first phase of a task whose issue still has the loop's label, with the label taken off
    /// as soon as the task's record exists (decision 4, review M-2): the record is written before
    /// the worker's sandbox is made, so the label is gone while the worker runs. A run refused
    /// before any write leaves the issue in the queue. Returns the phase's result and, if the
    /// label was taken off meanwhile, how that went.
    fn run_unlabeling(
        &self,
        issue: u32,
        step: impl FnOnce() -> Result<()>,
    ) -> (Result<()>, Option<Result<()>>) {
        let task_json = self.task_dir(issue).join("task.json");
        let (github, repo) = (self.github, self.repo.as_str());
        let (done, ended) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            let watcher = scope.spawn(move || {
                loop {
                    if task_json.is_file() {
                        return Some(github.issue_labels(
                            repo,
                            issue,
                            &[],
                            &[READY_LABEL.to_owned()],
                        ));
                    }
                    if !matches!(
                        ended.recv_timeout(LABEL_POLL),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        return None;
                    }
                }
            });
            let result = step();
            drop(done);
            (result, watcher.join().unwrap_or(None))
        })
    }

    /// After a phase: the label's removal reported, or done now if the watcher didn't see the
    /// record in time. `true` once the label is no longer the loop's to take off.
    fn unlabeled(
        &mut self,
        issue: u32,
        removed: Option<Result<()>>,
        warn: &mut dyn Write,
    ) -> Result<bool> {
        let removed = match removed {
            Some(removed) => removed,
            None if self.record(issue).is_some() => {
                self.github
                    .issue_labels(&self.repo, issue, &[], &[READY_LABEL.to_owned()])
            }
            None => return Ok(false),
        };
        if let Err(e) = removed {
            self.warn_once(
                warn,
                format!("#{issue}'s {READY_LABEL} label could not be removed: {e:#}"),
            )?;
        }
        Ok(true)
    }

    fn warn_once(&mut self, warn: &mut dyn Write, text: String) -> Result<()> {
        if self.warned.insert(text.clone()) {
            writeln!(warn, "warning: {text}")?;
        }
        Ok(())
    }

    /// The open PRs a loop opened that await the user (decision 3): every issue task of this repo
    /// a loop finished ([`LOOP_HOOK`]) whose record names a PR that is open and has no auto-merge
    /// armed, each PR once. A PR that can't be read (gone, or `gh` failing) is left out with a
    /// warning.
    fn awaiting_prs(&mut self, warn: &mut dyn Write) -> Result<Vec<u32>> {
        let mut awaiting = Vec::new();
        for pr in queue::loop_prs(&self.base_dir, &self.repo)? {
            let Some(number) = pr.number else {
                self.warn_once(
                    warn,
                    format!(
                        "task {} names a PR this loop can't read the number of ({}); not \
                         counted among the PRs awaiting you",
                        pr.task, pr.url
                    ),
                )?;
                continue;
            };
            match self.github.pr_merge_state(&self.repo, number) {
                Ok(state) if queue::awaits_user(state) => awaiting.push(number),
                Ok(_) => {}
                Err(e) => self.warn_once(
                    warn,
                    format!(
                        "task {} names PR #{number}, which can't be read ({e:#}); not counted \
                         among the PRs awaiting you",
                        pr.task
                    ),
                )?,
            }
        }
        awaiting.sort_unstable();
        awaiting.dedup();
        Ok(awaiting)
    }

    /// Waits while [`PR_CAP`] PRs await the user. `false`: the user asked to stop meanwhile.
    fn wait_for_prs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<bool> {
        let mut since = None;
        loop {
            let awaiting = self.awaiting_prs(warn)?;
            if awaiting.len() < PR_CAP {
                if let Some(since) = since {
                    let waited = self.now().saturating_sub(since);
                    writeln!(out, "resumed after {}", queue::span(waited))?;
                }
                return Ok(true);
            }
            if since.is_none() {
                since = Some(self.now());
                let list: Vec<String> = awaiting.iter().map(|n| format!("#{n}")).collect();
                writeln!(
                    out,
                    "paused: {} PRs await you ({}); merge or close one, or arm its auto-merge; \
                     checking again every {}",
                    awaiting.len(),
                    list.join(", "),
                    queue::span(PAUSE_POLL.as_secs())
                )?;
            }
            (self.env.sleep)(PAUSE_POLL);
            if (self.env.stop_requested)() {
                return Ok(false);
            }
        }
    }

    /// Waits while the disk is low (decision 245(g)); returns how long it waited, or `None` when
    /// a stop was asked meanwhile.
    fn wait_for_room(&mut self, out: &mut dyn Write) -> Result<Option<u64>> {
        let start = self.now();
        let mut said = false;
        while let Err(e) = self.steps.room() {
            if !said {
                writeln!(
                    out,
                    "paused: {e:#}; checking again every {}",
                    queue::span(PAUSE_POLL.as_secs())
                )?;
                said = true;
            }
            (self.env.sleep)(PAUSE_POLL);
            if (self.env.stop_requested)() {
                return Ok(None);
            }
        }
        Ok(Some(self.now().saturating_sub(start)))
    }

    /// The usage-limit wait, slept in `PAUSE_POLL` steps so a stop request is seen within one
    /// (the user's Ctrl-C answer, 2026-11-02). Returns how long it slept and whether a stop was
    /// asked.
    fn usage_sleep(&self, wait: Duration) -> (Duration, bool) {
        let mut slept = Duration::ZERO;
        while slept < wait {
            let step = (wait - slept).min(PAUSE_POLL);
            (self.env.sleep)(step);
            slept += step;
            if (self.env.stop_requested)() {
                return (slept, true);
            }
        }
        (slept, false)
    }

    /// A task cut short by a stop request: says how to continue it.
    fn stopped(issue: u32, out: &mut dyn Write) -> std::io::Result<End> {
        writeln!(
            out,
            "#{issue}: stopped on request; continue it with `sbxm task resume --issue {issue}`"
        )?;
        Ok(End::Parked {
            ready: None,
            outcome: "stopped on request before its resume".to_owned(),
        })
    }

    fn task_dir(&self, issue: u32) -> PathBuf {
        record::task_dir(&self.base_dir, &format!("issue-{issue}"))
    }

    fn record(&self, issue: u32) -> Option<Record> {
        record::read(&self.task_dir(issue).join("task.json")).ok()
    }

    /// The next issue to start (decision 2, 245(e)), and the labeled ones waiting on an unmerged
    /// `Depends on:` issue or PR, looked at again on every call: a dependency merged meanwhile
    /// frees its issue. One that `task run` would refuse before any write is reported into
    /// `refused` and not tried again. `None` when nothing is left.
    fn next(
        &mut self,
        open: &[Issue],
        named: &[u32],
        tried: &mut HashSet<u32>,
        refused: &mut Vec<Skipped>,
        out: &mut dyn Write,
    ) -> Result<(Option<u32>, Vec<Skipped>)> {
        let candidates = if named.is_empty() {
            queue::queue_order(open)
        } else {
            named.to_vec()
        };
        let mut waiting = Vec::new();
        for number in candidates {
            if tried.contains(&number) {
                continue;
            }
            let found = open.iter().find(|i| i.number == number);
            let title = found.map_or_else(String::new, |i| format!(" {}", i.title));
            let body = found.map_or("", |i| i.body.as_str());
            let unmerged = self.unmerged_dependencies(open, body);
            if !unmerged.is_empty() {
                let reason = format!("depends on {}, not merged", unmerged.join(", "));
                self.say_once(out, format!("skipped #{number}{title}: {reason}"))?;
                waiting.push(Skipped {
                    issue: number,
                    reason,
                });
                continue;
            }
            tried.insert(number);
            let problems =
                pipeline::preflight_issues(self.config_dir, &self.repo, self.github, &[number])?;
            if let Some(reason) = problems.get(&number) {
                let reason = format!("refused before any write: {reason}");
                writeln!(out, "skipped #{number}{title}: {reason}")?;
                refused.push(Skipped {
                    issue: number,
                    reason,
                });
                continue;
            }
            return Ok((Some(number), waiting));
        }
        Ok((None, waiting))
    }

    /// The `Depends on:` numbers that are an open issue or a PR not merged. A number that is
    /// neither an open issue nor a readable PR is taken for a closed issue.
    fn unmerged_dependencies(&self, open: &[Issue], body: &str) -> Vec<String> {
        queue::depends_on(body)
            .into_iter()
            .filter_map(|n| {
                if open.iter().any(|i| i.number == n) {
                    return Some(format!("#{n} (an open issue)"));
                }
                match self.github.pr(&self.repo, n) {
                    Ok(pr) if pr.state == PrState::Merged => None,
                    Ok(pr) => Some(format!(
                        "#{n} (a {} PR)",
                        if pr.state == PrState::Open {
                            "open"
                        } else {
                            "closed"
                        }
                    )),
                    Err(_) => None,
                }
            })
            .collect()
    }

    /// One task, to its PR or its parking (decision 245(d)). `label`: its issue has the loop's
    /// label, taken off once the task exists.
    /// `failures`: the failures in a row it already had, for one tried again after its lock was
    /// held.
    fn task(
        &mut self,
        issue: u32,
        mut label: bool,
        mut failures: u32,
        stops: &mut Vec<(String, u64)>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> std::result::Result<End, Halt> {
        let io = |e: std::io::Error| Halt::Error(e.into());
        let mut pending: Option<(String, u64)> = None;
        loop {
            let Some(paused) = self.wait_for_room(out).map_err(Halt::Error)? else {
                if self.record(issue).is_none() {
                    return Err(Halt::Stopped);
                }
                return Self::stopped(issue, out).map_err(io);
            };
            if paused > 0 {
                stops.push(("disk".to_owned(), paused));
            }
            if let Some((cause, since)) = pending.take() {
                stops.push((cause, self.now().saturating_sub(since)));
            }
            let resuming = self.record(issue).is_some();
            let mut captured = Vec::new();
            let result = {
                let mut out = Capture {
                    inner: out,
                    text: Vec::new(),
                };
                let mut warn = Capture {
                    inner: warn,
                    text: Vec::new(),
                };
                let (result, removed) = if resuming {
                    (self.steps.resume(issue, &mut out, &mut warn), None)
                } else if label {
                    self.run_unlabeling(issue, || self.steps.run(issue, &mut out, &mut warn))
                } else {
                    (self.steps.run(issue, &mut out, &mut warn), None)
                };
                captured.extend(out.text);
                captured.extend(warn.text);
                (result, removed)
            };
            let (result, removed) = result;
            if label && self.unlabeled(issue, removed, warn).map_err(Halt::Error)? {
                label = false;
            }
            let ready = self.record(issue).filter(|r| r.stage == Stage::Ready);
            let e = match (result, ready) {
                (Ok(()), Some(_)) => {
                    self.usage_waited = Duration::ZERO;
                    return self.finish(issue, None, out, warn).map_err(Halt::Error);
                }
                (Ok(()), None) => anyhow!("the task did not reach ready"),
                (Err(e), _) => e,
            };
            // A live command run by hand holds the task (issue 409): its resume is skipped, not
            // failed, and the loop moves on.
            if resuming && e.is::<lock::InUse>() {
                writeln!(out, "{}", held_line(issue, "resume", &e)).map_err(io)?;
                return Ok(End::Held(Retry::Resume { failures }));
            }
            if e.is::<pipeline::MissingSecret>() {
                return Err(Halt::Error(e));
            }
            if queue::failure_says_usage_limit(
                &format!("{e:#}"),
                &String::from_utf8_lossy(&captured),
            ) {
                let total = self.settings.usage_wait_total;
                if self.usage_waited >= total {
                    return Err(Halt::UsageLimit(total));
                }
                let wait = self.settings.usage_wait;
                writeln!(
                    out,
                    "#{issue}: usage limit; waiting {} before `task resume` ({} of {} waited in a \
                     row)",
                    queue::span(wait.as_secs()),
                    queue::span(self.usage_waited.as_secs()),
                    queue::span(total.as_secs())
                )
                .map_err(io)?;
                let (slept, asked) = self.usage_sleep(wait);
                self.usage_waited += slept;
                stops.push(("usage limit".to_owned(), slept.as_secs()));
                if asked {
                    return Self::stopped(issue, out).map_err(io);
                }
                continue;
            }
            self.usage_waited = Duration::ZERO;
            let environment =
                e.is::<pipeline::SbxUnavailable>() || e.is::<crate::headless::ExecUnavailable>();
            if self.record(issue).is_none() && !environment {
                return Ok(End::Refused(format!("{e:#}")));
            }
            failures += 1;
            let cause = first_line(&format!("{e:#}"));
            if failures >= 2 {
                stops.push((cause, 0));
                let stage = self
                    .record(issue)
                    .map_or("no record".to_owned(), |r| r.stage.name().to_owned());
                writeln!(
                    out,
                    "#{issue}: parked at {stage} after a second failure: {e:#}; see `sbxm task \
                     status --issue {issue}`"
                )
                .map_err(io)?;
                return Ok(End::Parked {
                    ready: None,
                    outcome: format!("parked at {stage}"),
                });
            }
            writeln!(out, "#{issue}: stopped ({e:#}); resuming it once").map_err(io)?;
            pending = Some((cause, self.now()));
            if (self.env.stop_requested)() {
                return Self::stopped(issue, out).map_err(io);
            }
        }
    }

    /// Marks a ready task as the loop's before its `task finish`, so a PR that finish opens counts
    /// toward [`PR_CAP`], after a restart too ([`opened_by_loop`]).
    /// Holds the task's lock (issue 58) around the read and the write, and gives it up before
    /// `task finish` takes its own (issue 409); a live holder refuses with [`lock::InUse`].
    /// A task already finished, by a command run by hand that held the lock, is left unmarked
    /// (review M-1 of issue 409).
    fn claim(&self, issue: u32) -> Result<Claimed> {
        let _lock = pipeline::lock_task(&self.base_dir, &format!("issue-{issue}"), self.probe)?;
        let dir = self.task_dir(issue);
        let mut rec = record::read(&dir.join("task.json"))?;
        if rec.pr.is_some() || rec.stage == Stage::Finished {
            return Ok(Claimed::Finished(rec.pr));
        }
        record::write_change(&dir, &mut rec, |r| {
            if !r.hooks.is_object() {
                r.hooks = serde_json::Value::Object(serde_json::Map::new());
            }
            r.hooks[LOOP_HOOK] = serde_json::Value::Bool(true);
        })?;
        Ok(Claimed::Marked)
    }

    /// A task whose lock was held, tried again (issue 409): it goes on from what its record says
    /// now, not from where the loop left it, since the command that held it may have moved it on
    /// (review M-1). Removed or finished: left to that command. Ready: its finish. Otherwise its
    /// resume.
    fn again(
        &mut self,
        issue: u32,
        retry: Retry,
        stops: &mut Vec<(String, u64)>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> std::result::Result<End, Halt> {
        let Some(rec) = self.record(issue) else {
            return Ok(End::Taken(format!(
                "#{issue}: its task was removed by a command run by hand; the loop leaves it"
            )));
        };
        if rec.pr.is_some() || rec.stage == Stage::Finished {
            return Ok(End::Taken(finished_by_hand(issue, rec.pr.as_deref())));
        }
        match (rec.stage, retry) {
            (Stage::Ready, Retry::Finish { ready }) => self
                .finish(issue, Some(ready), out, warn)
                .map_err(Halt::Error),
            (Stage::Ready, Retry::Resume { .. }) => {
                self.finish(issue, None, out, warn).map_err(Halt::Error)
            }
            (_, Retry::Finish { .. }) => self.task(issue, false, 0, stops, out, warn),
            (_, Retry::Resume { failures }) => self.task(issue, false, failures, stops, out, warn),
        }
    }

    /// A ready task: `task finish`, then for a clean one its findings filed. `ready`: when it
    /// reached ready, for one tried again after its lock was held; now otherwise.
    fn finish(
        &mut self,
        issue: u32,
        ready: Option<u64>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<End> {
        let ready = ready.unwrap_or_else(|| self.now());
        let rec = self.record(issue);
        let must_fix = rec
            .as_ref()
            .and_then(|r| r.review.as_ref())
            .map_or(0, |r| r.must_fix);
        let stopped = rec.as_ref().and_then(|r| r.stopped);
        let clean = must_fix == 0 && stopped.is_none();
        match self.claim(issue) {
            Ok(Claimed::Marked) => {}
            Ok(Claimed::Finished(pr)) => {
                return Ok(End::Taken(finished_by_hand(issue, pr.as_deref())));
            }
            Err(e) if e.is::<lock::InUse>() => {
                writeln!(out, "{}", held_line(issue, "finish", &e))?;
                return Ok(End::Held(Retry::Finish { ready }));
            }
            Err(e) => self.warn_once(
                warn,
                format!(
                    "#{issue} could not be marked as the loop's ({e:#}); its PR won't count \
                     among the PRs awaiting you"
                ),
            )?,
        }
        if let Err(e) = self.steps.finish(issue, out, warn) {
            writeln!(
                out,
                "#{issue}: parked at ready, its finish failed: {e:#}; run `sbxm task finish \
                 --issue {issue}`"
            )?;
            return Ok(End::Parked {
                ready: Some(ready),
                outcome: format!(
                    "parked at ready: finish failed ({})",
                    first_line(&format!("{e:#}"))
                ),
            });
        }
        let pr = self
            .record(issue)
            .and_then(|r| r.pr)
            .and_then(|url| queue::pr_number(&url))
            .map_or_else(|| "PR".to_owned(), |n| format!("PR #{n}"));
        let armed = self.arm(issue, out, warn)?;
        let outcome = if clean {
            if let Err(e) = self.steps.file_findings(issue, out, warn) {
                writeln!(
                    warn,
                    "warning: #{issue}'s findings were not filed: {e:#}; run `sbxm task \
                     file-findings --issue {issue} --create`"
                )?;
            }
            match armed {
                Some(note) => format!("{pr}, clean, {note}"),
                None => format!("{pr}, clean"),
            }
        } else {
            let why = match stopped {
                Some(stopped) => format!("work stop: {}", stopped.name()),
                None => "work stop".to_owned(),
            };
            format!("draft {pr}, {why}, must-fix {must_fix}")
        };
        Ok(End::Done { ready, outcome })
    }

    /// Removes the tasks whose PRs GitHub merged (decision 293), before the loop's next step. A
    /// failure is a warning: the tasks stay, and the next step tries again.
    fn clean_merged(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        if let Err(e) = self.steps.rm_merged(out, warn) {
            self.warn_once(
                warn,
                format!("merged tasks could not be removed: {e:#}; the loop goes on"),
            )?;
        }
        Ok(())
    }

    /// Arms GitHub's auto-merge on the PR a loop task just opened, under decisions 266, 280 and
    /// 293: the review's risk (after the path-rule floor) is low, it found 0 must-fix, the task
    /// ended without a work stop (so its PR isn't a draft), and the PR's head is still the commit
    /// the review covered. Nothing is checked on CI: GitHub's `--auto` waits for the required
    /// checks. A PR not armed stays among those awaiting the user, so every refusal is only a
    /// note. Returns the note for the loop's log row, when there is one.
    fn arm(
        &mut self,
        issue: u32,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        let Some(rec) = self.record(issue) else {
            return Ok(None);
        };
        let Some(number) = rec.pr.as_deref().and_then(queue::pr_number) else {
            return Ok(None);
        };
        // A task that continued an open PR only pushed to it; the PR isn't the loop's own.
        if !queue::opened_by_loop(&rec) {
            return Ok(None);
        }
        let Some(review) = rec.review.as_ref() else {
            return Ok(None);
        };
        let waits = |why: String, out: &mut dyn Write| -> Result<Option<String>> {
            writeln!(out, "PR #{number} awaits you: {why}")?;
            Ok(None)
        };
        if rec.stopped.is_some() || review.must_fix > 0 {
            return waits("a draft with must-fix left".to_owned(), out);
        }
        if review.risk != Level::Low {
            return waits(format!("risk {}", review.risk.word()), out);
        }
        let Some(reviewed) = review.commit.as_deref() else {
            return waits("the commit the review covered is unknown".to_owned(), out);
        };
        if self.paused(warn)? {
            return Ok(Some(Self::not_armed_paused(number, out)?));
        }
        let state = match self.github.pr_merge_state(&self.repo, number) {
            Ok(state) => state,
            Err(e) => return self.arming_failed(number, &e, out, warn),
        };
        if !queue::awaits_user(state) {
            // Merged, closed, or armed already (by the user, say): left alone.
            return Ok(None);
        }
        let head = match self.github.pr(&self.repo, number) {
            Ok(pr) => pr.head_oid,
            Err(e) => return self.arming_failed(number, &e, out, warn),
        };
        if head != reviewed {
            let note = "auto-merge not armed: the PR's head moved after the review";
            writeln!(
                out,
                "PR #{number}: {note} (reviewed {reviewed}, head {head}); it awaits you"
            )?;
            return Ok(Some(note.to_owned()));
        }
        match self.github.pr_arm_auto_merge(&self.repo, number, reviewed) {
            Ok(()) => {
                self.armed.insert(number);
                writeln!(out, "PR #{number}: auto-merge armed on {reviewed}")?;
                Ok(Some("auto-merge armed".to_owned()))
            }
            Err(e) => self.arming_failed(number, &e, out, warn),
        }
    }

    fn arming_failed(
        &mut self,
        number: u32,
        e: &anyhow::Error,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        self.warn_once(
            warn,
            format!("auto-merge could not be armed on PR #{number}: {e:#}; it awaits you"),
        )?;
        writeln!(out, "PR #{number}: auto-merge not armed: arming failed")?;
        Ok(Some("auto-merge not armed: arming failed".to_owned()))
    }

    /// Resolves the open PRs a loop opened that GitHub reports conflicting with their base
    /// (decision 298), one at a time in PR-number order, before the next issue: at most once per
    /// (PR, base tip), never a draft, and never again in this run after a failed one. A PR only
    /// behind is left (it still merges), and one GitHub is still computing is read again at the
    /// next step. `false`: the user asked to stop meanwhile.
    fn resolve_conflicts(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<bool> {
        let mut prs: Vec<(u32, String)> = queue::loop_prs(&self.base_dir, &self.repo)?
            .into_iter()
            .filter_map(|pr| Some((pr.number?, pr.task)))
            .collect();
        prs.sort();
        prs.dedup_by_key(|(number, _)| *number);
        let mut base: Option<(String, String)> = None;
        for (number, task) in prs {
            if self.unresolved.contains(&number) {
                continue;
            }
            // A PR that can't be read is warned about among the PRs awaiting the user.
            let Ok(state) = self.github.pr_merge_state(&self.repo, number) else {
                continue;
            };
            if state.state != PrState::Open || state.mergeable != Mergeable::Conflicting {
                continue;
            }
            if state.draft {
                self.say_once(
                    out,
                    format!(
                        "PR #{number} conflicts with its base but is a draft; it awaits you, and \
                         the loop leaves it"
                    ),
                )?;
                continue;
            }
            let (branch, tip) = match &base {
                Some(base) => base.clone(),
                None => match self.base_tip() {
                    Ok(found) => base.insert(found).clone(),
                    Err(e) => {
                        self.warn_once(
                            warn,
                            format!(
                                "the base's head could not be read ({e:#}); conflicting PRs are \
                                 not resolved at this step"
                            ),
                        )?;
                        return Ok(true);
                    }
                },
            };
            if self.resolves.contains(&(number, tip.clone())) {
                continue;
            }
            // `task fix` would refuse on a low disk: the PR waits for a later step, not failed.
            if self.steps.room().is_err() {
                return Ok(true);
            }
            self.resolves.insert((number, tip));
            if (self.env.stop_requested)() {
                return Ok(false);
            }
            self.resolve(number, &task, state.auto_merge, &branch, out, warn)?;
        }
        Ok(true)
    }

    /// The repo's default branch, which `task fix` merges in, and the commit it is at.
    fn base_tip(&self) -> Result<(String, String)> {
        let branch = self.github.default_branch(&self.repo)?;
        let tip = self.github.branch_head(&self.repo, &branch)?;
        Ok((branch, tip))
    }

    /// One resolve (decision 298): auto-merge disarmed if `armed`, `task fix --pr N`, then armed
    /// again on the same checks as [`Self::arm`], read from the fix task's record, for a PR that
    /// was armed or whose own task and earlier resolves passed them. A failed one is said once and left to the user,
    /// its fix task too. One log row either way.
    fn resolve(
        &mut self,
        number: u32,
        task: &str,
        armed: bool,
        branch: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let started = self.now();
        let head = match self.github.pr(&self.repo, number) {
            Ok(pr) => pr.head_oid,
            Err(e) => {
                return self.warn_once(
                    warn,
                    format!(
                        "PR #{number} can't be read ({e:#}); it is not resolved until {branch} \
                         moves"
                    ),
                );
            }
        };
        writeln!(
            out,
            "PR #{number} conflicts with {branch}; resolving it with `sbxm task fix --pr {number}`"
        )?;
        if armed {
            if let Err(e) = self.github.pr_disarm_auto_merge(&self.repo, number) {
                return self.warn_once(
                    warn,
                    format!(
                        "auto-merge could not be disarmed on PR #{number} ({e:#}); it is not \
                         resolved until {branch} moves"
                    ),
                );
            }
            writeln!(out, "PR #{number}: auto-merge disarmed for the resolve")?;
        }
        let result = self.steps.fix_pr(number, out, warn);
        let fix = record::fix_id(number, &head).and_then(|id| {
            record::read(&record::task_dir(&self.base_dir, &id).join("task.json")).ok()
        });
        let review = fix.as_ref().and_then(|r| r.review.clone());
        let outcome = match (&result, &review) {
            (Err(e), _) => self.left_to_user(
                number,
                &format!("its resolve failed: {}", first_line(&format!("{e:#}"))),
                out,
            )?,
            (Ok(()), Some(review)) if review.must_fix > 0 => self.left_to_user(
                number,
                &format!("its resolve's review left {} must-fix", review.must_fix),
                out,
            )?,
            (Ok(()), None) => self.left_to_user(number, "its resolve's review is unknown", out)?,
            (Ok(()), Some(review)) => {
                let own = self
                    .record_of(task)
                    .is_some_and(|rec| passes_arm_checks(&rec));
                let current = record::fix_id(number, &head).unwrap_or_default();
                let eligible = armed || (own && self.earlier_resolves_pass(number, &current));
                match self.rearm(number, eligible, review, out, warn)? {
                    Some(note) => format!("resolved, {note}"),
                    None => "resolved".to_owned(),
                }
            }
        };
        let duration = |agent: Option<&record::Agent>| agent?.run.as_ref().map(|r| r.duration_s);
        let row = Row {
            issue: number,
            title: format!("resolve: conflicts with {branch} (task {task})"),
            size: None,
            started,
            ready: None,
            worker_s: fix.as_ref().and_then(|r| duration(r.worker.as_ref())),
            reviewer_s: fix.as_ref().and_then(|r| duration(r.reviewer.as_ref())),
            rounds: review.as_ref().map(|r| r.round),
            must_fix: review.as_ref().map(|r| r.must_fix),
            outcome: outcome.clone(),
            stops: Vec::new(),
            idle_before: None,
        };
        writeln!(out, "PR #{number}: {outcome}")?;
        if let Err(e) = queue::append_resolve_row(&self.log, &row) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that log row"),
            )?;
        }
        Ok(())
    }

    /// A failed resolve: said once, and the PR left to the user for the rest of the run.
    fn left_to_user(&mut self, number: u32, why: &str, out: &mut dyn Write) -> Result<String> {
        self.unresolved.insert(number);
        self.say_once(
            out,
            format!(
                "PR #{number} awaits you: {why}; its fix task is kept (see `sbxm task status`), \
                 and the loop leaves the PR"
            ),
        )?;
        Ok(format!("not resolved: {why}"))
    }

    /// Whether every resolve already pushed onto PR `number` (a finished fix task of it other
    /// than `current`) passed [`Self::arm`]'s checks, as its own task must: a narrow review of a
    /// later merge doesn't clear the risk an earlier one left (review M-1). Read from the fix
    /// tasks' records, so a restarted loop sees them too; one that can't be read counts as not
    /// passing.
    fn earlier_resolves_pass(&self, number: u32, current: &str) -> bool {
        let Ok(folders) = record::task_folders(&self.base_dir) else {
            return false;
        };
        let prefix = format!("fix-{number}-");
        folders.iter().all(|dir| {
            let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
                return true;
            };
            if !id.starts_with(&prefix) || id == current {
                return true;
            }
            if check_own_folder(&self.base_dir, dir).is_err() {
                return false;
            }
            match record::read(&dir.join("task.json")) {
                Ok(rec) if rec.kind != Kind::Fix || rec.repo != self.repo => true,
                Ok(rec) if rec.stage != Stage::Finished => true,
                Ok(rec) => passes_arm_checks(&rec),
                Err(_) => false,
            }
        })
    }

    /// A task's record by its id.
    fn record_of(&self, id: &str) -> Option<Record> {
        record::read(&record::task_dir(&self.base_dir, id).join("task.json")).ok()
    }

    /// Arms auto-merge again after a clean resolve (decision 298): only for a PR that `eligible`
    /// (armed before, or its own task and every earlier resolve pushed onto it passed
    /// [`Self::arm`]'s checks), when the fix's review is
    /// low risk with 0 must-fix and covered the PR's new head. Returns the log row's note.
    fn rearm(
        &mut self,
        number: u32,
        eligible: bool,
        review: &record::ReviewResult,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        if !eligible {
            writeln!(
                out,
                "PR #{number} awaits you: its own review didn't allow auto-merge"
            )?;
            return Ok(None);
        }
        if self.paused(warn)? {
            return Ok(Some(Self::not_armed_paused(number, out)?));
        }
        if review.risk != Level::Low {
            writeln!(
                out,
                "PR #{number} awaits you: its resolve's review is risk {}",
                review.risk.word()
            )?;
            return Ok(Some(format!(
                "auto-merge not armed: risk {}",
                review.risk.word()
            )));
        }
        let Some(reviewed) = review.commit.as_deref() else {
            writeln!(
                out,
                "PR #{number} awaits you: the commit its resolve's review covered is unknown"
            )?;
            return Ok(Some(
                "auto-merge not armed: reviewed commit unknown".to_owned(),
            ));
        };
        let head = match self.github.pr(&self.repo, number) {
            Ok(pr) => pr.head_oid,
            Err(e) => return self.arming_failed(number, &e, out, warn),
        };
        if head != reviewed {
            let note = "auto-merge not armed: the PR's head isn't the commit the resolve's review \
                        covered";
            writeln!(
                out,
                "PR #{number}: {note} (reviewed {reviewed}, head {head}); it awaits you"
            )?;
            return Ok(Some(note.to_owned()));
        }
        match self.github.pr_arm_auto_merge(&self.repo, number, reviewed) {
            Ok(()) => {
                self.armed.insert(number);
                writeln!(out, "PR #{number}: auto-merge armed again on {reviewed}")?;
                Ok(Some("auto-merge armed".to_owned()))
            }
            Err(e) => self.arming_failed(number, &e, out, warn),
        }
    }

    /// Whether the loop is paused on a red `main` (decision 299), read from the pause file each
    /// time, so the user's `--clear-pause` counts at once. A file that can't be read counts as a
    /// pause, with a warning naming it.
    fn paused(&mut self, warn: &mut dyn Write) -> Result<bool> {
        match loop_pause::read(&self.base_dir) {
            loop_pause::State::None => Ok(false),
            loop_pause::State::Set(_) => Ok(true),
            loop_pause::State::Unreadable(why) => {
                self.warn_once(
                    warn,
                    format!(
                        "{why}; counted as a pause on a red main: nothing is armed until you run \
                         `sbxm task loop --clear-pause`"
                    ),
                )?;
                Ok(true)
            }
        }
    }

    /// A PR not armed because of the pause: said, and the log row's note.
    fn not_armed_paused(number: u32, out: &mut dyn Write) -> Result<String> {
        writeln!(
            out,
            "PR #{number} awaits you: the loop is paused on a red main and arms nothing until you \
             run `sbxm task loop --clear-pause`"
        )?;
        Ok("auto-merge not armed: paused on a red main".to_owned())
    }

    /// The pass over workflow runs (decision 299), after the resolves and before the wait for
    /// PRs: `main` first, so a pause set now disarms the PRs in the same pass.
    fn check_runs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.check_main(out, warn)?;
        if let loop_pause::State::Set(pause) = loop_pause::read(&self.base_dir) {
            self.paused_for.insert((pause.commit.clone(), pause.run_id));
            self.say_once(out, paused_line(&pause))?;
        }
        self.check_prs(out, warn)
    }

    /// Reads `commit`'s runs (`what` names it in messages): the failed jobs of a run's first
    /// failure are re-run once. Returns the runs that failed again after a re-run, and whether
    /// every run is over (none going, none just re-run); `None` when the runs can't be read.
    fn read_runs(
        &mut self,
        commit: &str,
        what: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<(Vec<WorkflowRun>, bool)>> {
        let runs = match self.github.commit_runs(&self.repo, commit) {
            Ok(runs) => runs,
            Err(e) => {
                self.warn_once(
                    warn,
                    format!(
                        "the workflow runs of {what} ({commit}) can't be read ({e:#}); they are \
                         read again at the next pass"
                    ),
                )?;
                return Ok(None);
            }
        };
        let mut failed = Vec::new();
        let mut over = true;
        for run in runs {
            match run.status {
                RunStatus::Passed => {}
                RunStatus::Going => over = false,
                RunStatus::Failed if run.attempt > 1 => failed.push(run),
                RunStatus::Failed => {
                    over = false;
                    if self.reruns.contains(&run.id) {
                        continue;
                    }
                    match self.github.run_rerun_failed(&self.repo, run.id) {
                        Ok(()) => {
                            self.reruns.insert(run.id);
                            writeln!(
                                out,
                                "{what}: {} failed ({}); its failed jobs are re-run once",
                                run.workflow, run.url
                            )?;
                        }
                        Err(e) => self.warn_once(
                            warn,
                            format!(
                                "the failed jobs of {what}'s run {} could not be re-run ({e:#}); \
                                 tried again at the next pass",
                                run.url
                            ),
                        )?,
                    }
                }
            }
        }
        Ok(Some((failed, over)))
    }

    /// `main`'s side of the pass (decision 299, answer 4): the merge commits of the PRs this run
    /// armed that merged since, and `main`'s tip whoever merged it. A second failure sets the
    /// pause; a merge commit whose runs aren't over is read again at the next pass.
    fn check_main(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let mut armed: Vec<u32> = self.armed.iter().copied().collect();
        armed.sort_unstable();
        for number in armed {
            match self.github.pr_merge_commit(&self.repo, number) {
                Ok(Some(commit)) => {
                    self.armed.remove(&number);
                    self.merges.push((number, commit));
                }
                Ok(None) => {}
                Err(e) => self.warn_once(
                    warn,
                    format!(
                        "PR #{number}'s merge can't be read ({e:#}); it is read again at the next \
                         pass"
                    ),
                )?,
            }
        }
        let mut commits: Vec<(String, Option<u32>)> = self
            .merges
            .iter()
            .map(|(number, commit)| (commit.clone(), Some(*number)))
            .collect();
        match self.base_tip() {
            Ok((_, tip)) => {
                if !commits.iter().any(|(commit, _)| *commit == tip) {
                    commits.push((tip, None));
                }
            }
            Err(e) => self.warn_once(
                warn,
                format!(
                    "the base's head could not be read ({e:#}); its workflow runs are read at a \
                     later pass"
                ),
            )?,
        }
        for (commit, pr) in commits {
            let what = match pr {
                Some(number) => format!("PR #{number}'s merge commit"),
                None => "main's tip".to_owned(),
            };
            let Some((failed, over)) = self.read_runs(&commit, &what, out, warn)? else {
                continue;
            };
            if let Some(run) = failed.first() {
                self.pause(&commit, pr, run, out)?;
            }
            if over || !failed.is_empty() {
                self.merges.retain(|(_, c)| *c != commit);
            }
        }
        Ok(())
    }

    /// Sets the pause for `run`, which failed again on `main`'s `commit` (`pr`: the PR this run
    /// armed that merged into it), unless a pause is there already or the user cleared one for
    /// this run. A pause that can't be written stops the loop, so nothing more is armed.
    fn pause(
        &mut self,
        commit: &str,
        pr: Option<u32>,
        run: &WorkflowRun,
        out: &mut dyn Write,
    ) -> Result<()> {
        if !self.paused_for.insert((commit.to_owned(), run.id)) {
            return Ok(());
        }
        if loop_pause::read(&self.base_dir).paused() {
            return Ok(());
        }
        let merged_by = match pr {
            Some(number) => format!("the loop's auto-merge of PR #{number}"),
            None if run.actor.is_empty() => "someone unknown".to_owned(),
            None => run.actor.clone(),
        };
        let pause = Pause {
            commit: commit.to_owned(),
            run_id: run.id,
            run_url: run.url.clone(),
            workflow: run.workflow.clone(),
            merged_by,
            at: format_timestamp(self.now()),
        };
        loop_pause::write(&self.base_dir, &pause).context(
            "main failed again after a re-run but the loop can't record its pause; the loop \
             stops so that nothing more is armed",
        )?;
        self.say_once(out, paused_line(&pause))
    }

    /// The PRs' side of the pass (decision 299): every open PR the loop opened, drafts included.
    /// While paused, armed ones are disarmed. A run that failed again after its re-run disarms
    /// the PR if armed; the PR then awaits the user, said once with a log row.
    fn check_prs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let paused = self.paused(warn)?;
        let mut prs: Vec<u32> = queue::loop_prs(&self.base_dir, &self.repo)?
            .into_iter()
            .filter_map(|pr| pr.number)
            .collect();
        prs.sort_unstable();
        prs.dedup();
        for number in prs {
            // A PR that can't be read is warned about among the PRs awaiting the user.
            let Ok(state) = self.github.pr_merge_state(&self.repo, number) else {
                continue;
            };
            if state.state != PrState::Open {
                continue;
            }
            let mut armed = state.auto_merge;
            if armed && paused {
                match self.github.pr_disarm_auto_merge(&self.repo, number) {
                    Ok(()) => {
                        self.armed.remove(&number);
                        armed = false;
                        writeln!(
                            out,
                            "PR #{number}: auto-merge disarmed while the loop is paused on a red \
                             main"
                        )?;
                    }
                    Err(e) => self.warn_once(
                        warn,
                        format!(
                            "auto-merge could not be disarmed on PR #{number} while the loop is \
                             paused ({e:#}); tried again at the next pass"
                        ),
                    )?,
                }
            }
            let head = match self.github.pr(&self.repo, number) {
                Ok(pr) => pr.head_oid,
                Err(e) => {
                    self.warn_once(
                        warn,
                        format!(
                            "PR #{number} can't be read ({e:#}); its workflow runs are read at a \
                             later pass"
                        ),
                    )?;
                    continue;
                }
            };
            let what = format!("PR #{number}");
            let Some((failed, _)) = self.read_runs(&head, &what, out, warn)? else {
                continue;
            };
            let Some(run) = failed.first() else {
                continue;
            };
            if self.failed_twice.contains(&(number, run.id)) {
                continue;
            }
            if armed {
                if let Err(e) = self.github.pr_disarm_auto_merge(&self.repo, number) {
                    self.warn_once(
                        warn,
                        format!(
                            "auto-merge could not be disarmed on PR #{number}, whose {} failed \
                             again after a re-run ({e:#}); tried again at the next pass",
                            run.workflow
                        ),
                    )?;
                    continue;
                }
                self.armed.remove(&number);
            }
            self.failed_twice.insert((number, run.id));
            let outcome = if armed {
                "auto-merge disarmed; awaits you"
            } else {
                "awaits you"
            };
            writeln!(
                out,
                "PR #{number} awaits you: its {} failed again after a re-run ({}){}",
                run.workflow,
                run.url,
                if armed { "; auto-merge disarmed" } else { "" }
            )?;
            let row = Row {
                issue: number,
                title: format!("checks: {} failed again after a re-run", run.workflow),
                size: None,
                started: self.now(),
                ready: None,
                worker_s: None,
                reviewer_s: None,
                rounds: None,
                must_fix: None,
                outcome: outcome.to_owned(),
                stops: Vec::new(),
                idle_before: None,
            };
            if let Err(e) = queue::append_resolve_row(&self.log, &row) {
                self.warn_once(
                    warn,
                    format!("{e:#}; the loop goes on without that log row"),
                )?;
            }
        }
        Ok(())
    }

    fn append(&mut self, row: &Row, warn: &mut dyn Write) -> Result<()> {
        if let Err(e) = queue::append_row(&self.log, row) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that log row"),
            )?;
        }
        Ok(())
    }
}

/// [`Loop::arm`]'s checks on a task's record: no work stop, a review with 0 must-fix and low risk
/// after the path-rule floor.
fn passes_arm_checks(rec: &Record) -> bool {
    rec.stopped.is_none()
        && rec
            .review
            .as_ref()
            .is_some_and(|r| r.must_fix == 0 && r.risk == Level::Low)
}

/// The line saying the loop is paused on a red `main`.
fn paused_line(pause: &Pause) -> String {
    format!(
        "paused on a red main: {}; the loop arms nothing until you run `sbxm task loop \
         --clear-pause`",
        pause.describe()
    )
}

/// `sbxm task loop --clear-pause` (decision 299): lifts the pause a red `main` set, saying what
/// it removed, or that there was none.
pub fn clear_pause(config_dir: &Path, out: &mut dyn Write) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    loop_pause::clear(&base_dir, out)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

/// Runs the loop (issue 320). Everything is checked before the first write: the config, the
/// repo, the task folders, then the lock is taken.
#[allow(clippy::too_many_arguments)]
pub fn run(
    config_dir: &Path,
    opts: &Options,
    steps: &dyn Steps,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    if opts.cap == Some(0) {
        writeln!(out, "--cap 0: no task runs")?;
        return Ok(());
    }
    if opts.issues.contains(&0) {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    let settings = TaskConfig::load(&opts.repo_root)?.task_loop;
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    // As `task rm --merged` refuses it (decision 195): every task folder is read through it.
    let tasks = record::tasks_root(&base_dir);
    if fs::symlink_metadata(&tasks).is_ok() {
        check_own_folder(&base_dir, &tasks)?;
    }
    let _lock = take_lock(&base_dir, probe, warn)?;

    let started_at = (env.now)();
    let date = format_timestamp(started_at);
    let date = date.split_once('T').map_or(date.as_str(), |(d, _)| d);
    let mut lp = Loop {
        config_dir,
        base_dir,
        repo,
        settings,
        steps,
        github,
        probe,
        env,
        usage_waited: Duration::ZERO,
        warned: HashSet::new(),
        said: HashSet::new(),
        resolves: HashSet::new(),
        unresolved: HashSet::new(),
        armed: HashSet::new(),
        merges: Vec::new(),
        reruns: HashSet::new(),
        failed_twice: HashSet::new(),
        paused_for: HashSet::new(),
        log: opts
            .repo_root
            .join("sdlc")
            .join("batches")
            .join(format!("{date}-loop.md")),
    };

    let mut tried = HashSet::new();
    let mut refused = Vec::new();
    let mut tasks_run = 0u32;
    let mut parked_in_row = 0;
    let mut idle_from: Option<u64> = None;
    // Tasks whose lock a command run by hand held (issue 409): `held` waits for the loop's next
    // task, then moves to `due`, tried before the next new issue. A task held again goes back to
    // `held`, so the loop never tries one twice with nothing run in between.
    let mut held: Vec<Held> = Vec::new();
    let mut due: VecDeque<Held> = VecDeque::new();
    let left = |held: &[Held], due: &VecDeque<Held>, out: &mut dyn Write| -> Result<()> {
        for h in due.iter().chain(held) {
            writeln!(out, "{}", h.left())?;
        }
        Ok(())
    };
    loop {
        if (env.stop_requested)() {
            writeln!(out, "stopped on request")?;
            left(&held, &due, out)?;
            return Ok(());
        }
        if opts.cap.is_some_and(|cap| tasks_run >= cap) {
            writeln!(out, "--cap {} reached", opts.cap.unwrap_or_default())?;
            left(&held, &due, out)?;
            return Ok(());
        }
        lp.clean_merged(out, warn)?;
        let go_on = lp.resolve_conflicts(out, warn)? && {
            lp.check_runs(out, warn)?;
            lp.wait_for_prs(out, warn)?
        };
        if !go_on {
            writeln!(out, "stopped on request")?;
            left(&held, &due, out)?;
            return Ok(());
        }
        let (issue, title, size, started, stops, ended) = if let Some(again) = due.pop_front() {
            let Held {
                issue,
                title,
                size,
                started,
                mut stops,
                retry,
            } = again;
            writeln!(
                out,
                "task {}: #{issue} {title} (size {}), tried again",
                tasks_run + 1,
                size.as_deref().unwrap_or("unknown")
            )?;
            let ended = lp.again(issue, retry, &mut stops, out, warn);
            (issue, title, size, started, stops, ended)
        } else {
            let open = github.issues_open(&lp.repo)?;
            let (next, waiting) = lp.next(&open, &opts.issues, &mut tried, &mut refused, out)?;
            let Some(issue) = next else {
                let skipped: Vec<Skipped> = refused.iter().chain(&waiting).cloned().collect();
                if opts.issues.is_empty() {
                    writeln!(out, "the queue is empty; the loop stops")?;
                    write!(
                        out,
                        "{}",
                        queue::report(&Report {
                            open: &open,
                            skipped: &skipped,
                        })
                    )?;
                } else {
                    writeln!(
                        out,
                        "every named issue was taken or skipped; the loop stops"
                    )?;
                }
                left(&held, &due, out)?;
                return Ok(());
            };
            let found = open.iter().find(|i| i.number == issue);
            let title = found.map_or_else(|| format!("issue {issue}"), |i| i.title.clone());
            let size = found
                .and_then(|i| queue::size(&i.labels))
                .map(str::to_owned);
            let labeled = found.is_some_and(|i| i.labels.iter().any(|l| l == READY_LABEL));

            let started = lp.now();
            writeln!(
                out,
                "task {}: #{issue} {title} (size {})",
                tasks_run + 1,
                size.as_deref().unwrap_or("unknown")
            )?;
            let mut stops = Vec::new();
            let ended = lp.task(issue, labeled, 0, &mut stops, out, warn);
            (issue, title, size, started, stops, ended)
        };
        let end = match ended {
            Ok(end) => end,
            Err(Halt::Error(e)) => return Err(e),
            Err(Halt::Stopped) => {
                writeln!(out, "stopped on request")?;
                left(&held, &due, out)?;
                return Ok(());
            }
            Err(Halt::UsageLimit(total)) => {
                let rec = lp.record(issue);
                let stage = rec.map_or("no record".to_owned(), |r| r.stage.name().to_owned());
                lp.append(
                    &Row {
                        issue,
                        title,
                        size,
                        started,
                        ready: None,
                        worker_s: None,
                        reviewer_s: None,
                        rounds: None,
                        must_fix: None,
                        outcome: format!("stopped at {stage}: usage limit"),
                        stops,
                        idle_before: idle_from.map(|end| started.saturating_sub(end)),
                    },
                    warn,
                )?;
                bail!(
                    "the usage limit held through {} of waits in a row; the loop stops; continue \
                     with `sbxm task resume --issue {issue}` once it resets",
                    queue::span(total.as_secs())
                );
            }
        };
        let (ready, outcome, parked) = match end {
            End::Refused(reason) => {
                writeln!(out, "skipped #{issue}: refused before any write: {reason}")?;
                refused.push(Skipped {
                    issue,
                    reason: format!("refused before any write: {reason}"),
                });
                continue;
            }
            End::Taken(line) => {
                writeln!(out, "{line}")?;
                continue;
            }
            End::Held(retry) => {
                held.push(Held {
                    issue,
                    title,
                    size,
                    started,
                    stops,
                    retry,
                });
                continue;
            }
            End::Done { ready, outcome } => (Some(ready), outcome, false),
            End::Parked { ready, outcome } => (ready, outcome, true),
        };
        due.extend(held.drain(..));
        tasks_run += 1;
        let rec = lp.record(issue);
        let duration = |agent: Option<&record::Agent>| agent?.run.as_ref().map(|r| r.duration_s);
        let row = Row {
            issue,
            title,
            size,
            started,
            ready,
            worker_s: rec.as_ref().and_then(|r| duration(r.worker.as_ref())),
            reviewer_s: rec.as_ref().and_then(|r| duration(r.reviewer.as_ref())),
            rounds: rec
                .as_ref()
                .and_then(|r| r.review.as_ref())
                .map(|r| r.round),
            must_fix: rec
                .as_ref()
                .and_then(|r| r.review.as_ref())
                .map(|r| r.must_fix),
            outcome: outcome.clone(),
            stops,
            idle_before: idle_from.map(|end| started.saturating_sub(end)),
        };
        writeln!(out, "#{issue}: {outcome}")?;
        lp.append(&row, warn)?;
        // Idle runs from a task's ready to the next start (decision 249, review S-1), so the
        // loop's own finish and findings filing count, a failed finish too; a task that never got
        // ready, from its end.
        idle_from = Some(ready.unwrap_or_else(|| lp.now()));
        if parked {
            parked_in_row += 1;
            if parked_in_row >= 2 {
                bail!(
                    "two tasks parked in a row; the loop stops; see `sbxm task status` for what \
                     each left"
                );
            }
        } else {
            parked_in_row = 0;
        }
    }
}

/// The real phases: the `task` commands themselves, each teeing its output into its task's
/// `run.log` as when run by hand.
pub struct TaskSteps<'a> {
    pub config_dir: &'a Path,
    pub repo_root: PathBuf,
    pub repo: Option<String>,
    pub backend: &'a dyn SandboxBackend,
    pub github: &'a dyn GitHubBackend,
    pub probe: &'a dyn ProcessProbe,
    pub host: &'a dyn HostRunner,
    /// The free-space check of the sandbox drive (decision 194).
    pub disk: disk::Guard,
}

impl TaskSteps<'_> {
    /// `out` and `warn` teed into `issue-<n>`'s `run.log`, as `main` does for a `task` command.
    fn logged(
        &self,
        issue: u32,
        out: &mut dyn Write,
        warn: &mut dyn Write,
        step: &dyn Fn(&mut dyn Write, &mut dyn Write) -> Result<()>,
    ) -> Result<()> {
        self.logged_as(format!("issue-{issue}"), out, warn, step)
    }

    /// `out` and `warn` teed into task `id`'s `run.log`.
    fn logged_as(
        &self,
        id: String,
        out: &mut dyn Write,
        warn: &mut dyn Write,
        step: &dyn Fn(&mut dyn Write, &mut dyn Write) -> Result<()>,
    ) -> Result<()> {
        let log: Arc<Mutex<RunLog>> =
            task_log::open(self.config_dir, vec![id], None).unwrap_or_else(|_| task_log::none());
        let mut out = Tee::new(out, Arc::clone(&log));
        let mut warn = Tee::new(warn, log);
        let result = step(&mut out, &mut warn);
        if let Err(e) = &result {
            let _ = writeln!(warn, "Error: {e:#}");
        }
        result
    }

    /// Gives up this process's own claim on `issue-<n>`'s record before `task resume` (review
    /// M-1): the phase that left it ran here and has returned, so the loop being alive doesn't
    /// mean the task is running. A task another process owns keeps its owner, and `task resume`
    /// refuses it while that process lives. The task's lock (issue 58) is held around the read
    /// and the write and given up before `task resume` takes its own (issue 409); a live holder
    /// refuses with [`lock::InUse`], writing nothing.
    fn release(&self, issue: u32) -> Result<()> {
        let base_dir = GlobalConfig::load(self.config_dir)?.base_dir;
        let id = format!("issue-{issue}");
        let _lock = pipeline::lock_task(&base_dir, &id, self.probe)?;
        let dir = record::task_dir(&base_dir, &id);
        let Ok(mut rec) = record::read(&dir.join("task.json")) else {
            return Ok(());
        };
        if rec.process.as_ref() == Some(&Process::current(self.probe)) {
            record::write_change(&dir, &mut rec, |r| r.process = None)?;
        }
        Ok(())
    }
}

impl Steps for TaskSteps<'_> {
    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.logged(issue, out, warn, &|out, warn| {
            task_run::run(
                self.config_dir,
                &task_run::Options {
                    repo_root: self.repo_root.clone(),
                    issue,
                    worker_harness: None,
                    worker_model: None,
                    reviewer_harness: None,
                    reviewer_model: None,
                    time_limit: None,
                    reviewer_time_limit: None,
                    profile: None,
                    base: None,
                    repo: self.repo.clone(),
                    clone_source: None,
                    identity: None,
                    disk: self.disk.clone(),
                    location: None,
                },
                None,
                self.backend,
                self.github,
                self.probe,
                self.host,
                out,
                warn,
            )
        })
    }

    fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.release(issue)?;
        self.logged(issue, out, warn, &|out, warn| {
            task_resume::run(
                self.config_dir,
                &task_resume::Options {
                    repo_root: self.repo_root.clone(),
                    target: task_resume::Target::Issue(issue),
                    rounds: None,
                    disk: self.disk.clone(),
                    identity: None,
                    forget_ambiguous: false,
                },
                self.backend,
                self.probe,
                self.host,
                out,
                warn,
            )
        })
    }

    fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.logged(issue, out, warn, &|out, _warn| {
            task_finish::run(
                self.config_dir,
                &task_finish::Options {
                    target: task_finish::Target::Issue(issue),
                    repo_root: self.repo_root.clone(),
                    push_unresolved: false,
                },
                self.backend,
                self.github,
                self.probe,
                out,
            )
        })
    }

    fn file_findings(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let base_dir = GlobalConfig::load(self.config_dir)?.base_dir;
        let review = record::task_dir(&base_dir, &format!("issue-{issue}")).join("review.md");
        let text = fs::read_to_string(&review)
            .with_context(|| format!("cannot read {}", review.display()))?;
        // Should-fix findings alone (decision 245(d)): a clean review has no must-fix, and its
        // questions stay in the PR's review text for the user.
        let only: Vec<String> = findings::parse(&text)
            .findings
            .into_iter()
            .filter(|f| f.label == "should-fix")
            .map(|f| f.id)
            .collect();
        if only.is_empty() {
            return Ok(());
        }
        self.logged(issue, out, warn, &|out, warn| {
            task_file_findings::run(
                &task_file_findings::Options {
                    source: task_file_findings::Source::Task {
                        base_dir: base_dir.clone(),
                        kind: Kind::Issue,
                        number: issue,
                    },
                    repo_root: self.repo_root.clone(),
                    repo: self.repo.clone(),
                    create: true,
                    pr: None,
                    standard_criteria: false,
                    keep_paths: false,
                    only: only.clone(),
                },
                self.github,
                out,
                warn,
            )
        })
    }

    fn fix_pr(&self, pr: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        // The fix task's id names the PR's head, so its run.log is named before `task fix` runs.
        let repo = resolve_repo(self.repo.as_deref(), &self.repo_root)?;
        let head = self.github.pr(&repo, pr)?.head_oid;
        let Some(id) = record::fix_id(pr, &head) else {
            bail!("PR #{pr}'s head {head:?} isn't a full commit id; see `gh pr view {pr}`");
        };
        self.logged_as(id, out, warn, &|out, warn| {
            task_fix::run(
                self.config_dir,
                &task_fix::Options {
                    repo_root: self.repo_root.clone(),
                    pr,
                    repo: self.repo.clone(),
                    base: None,
                    clone_source: None,
                    identity: None,
                    disk: self.disk.clone(),
                },
                self.backend,
                self.github,
                self.probe,
                self.host,
                out,
                warn,
            )
        })
    }

    fn rm_merged(&self, out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        task_rm::run(
            self.config_dir,
            &task_rm::Options {
                target: task_rm::Target::Merged,
                yes: true,
                force: false,
                repo_root: self.repo_root.clone(),
                dry_run: false,
                forget_ambiguous: false,
            },
            self.backend,
            self.github,
            self.probe,
            &crate::confirm::Terminal,
            out,
        )
    }

    fn room(&self) -> Result<()> {
        let config = TaskConfig::load(&self.repo_root)?;
        if pipeline::choose_location(None, &config) == Location::Cloud {
            return Ok(());
        }
        disk::require(
            &GlobalConfig::load(self.config_dir)?,
            self.config_dir,
            &self.disk,
        )
    }
}
