//! `sbxm task loop [--cap N] [--issue N ...]` (issue 320, decision 245 (b) to (h)): works through
//! the open issues labeled `ready-for-worker` one task at a time, end to end: `task run`, then
//! `task finish` (a draft PR for a work stop), then `task file-findings --create` for a clean
//! task. It pauses while 5 PRs await the user or the disk is low, resumes a task once after an
//! environment stop and parks it after a second, waits out a usage limit, and stops on an empty
//! queue (with a read-only report), two parked tasks in a row, `--cap`, the usage waits' total,
//! or a stop request between phases. One log row per task goes to
//! `sdlc/batches/<date>-loop.md`.
//!
//! The phases go through [`Steps`] so the loop's rules are tested without sandboxes;
//! [`TaskSteps`] runs the real commands.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::task_start::resolve_repo;
use super::{task_file_findings, task_finish, task_log, task_resume, task_run};
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::{GitHubBackend, Issue, PrState};
use crate::run::results::format_timestamp;
use crate::task::config::TaskConfig;
use crate::task::finish::check_own_folder;
use crate::task::gates::HostRunner;
use crate::task::queue::{self, READY_LABEL, Report, Row, Skipped};
use crate::task::record::{self, Kind, Process, ProcessProbe, Record, Stage};
use crate::task::runlog::{RunLog, Tee};
use crate::task::{findings, pipeline};

/// How many open PRs the loop opened may await the user before it pauses (decision 245(e)).
pub const PR_CAP: usize = 5;
/// How often a pause looks again.
pub const PAUSE_POLL: Duration = Duration::from_secs(5 * 60);

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
    /// `task file-findings --issue N --create`: the review's findings as sized issues, without
    /// the loop's label; nothing when the review has none.
    fn file_findings(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
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

fn lock_path(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("task-loop.lock")
}

/// Takes the loop's lock for this process, as `template build` takes its own (decision 212): a
/// lock whose process is alive refuses the second loop; one whose process is gone is taken over.
fn take_lock(base_dir: &Path, probe: &dyn ProcessProbe, warn: &mut dyn Write) -> Result<LoopLock> {
    let path = lock_path(base_dir);
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
    /// Stopped short of `ready` twice, or its finish failed: left as it is for the user.
    Parked { outcome: String },
    /// Refused before anything was written: not a task.
    Refused(String),
}

/// Why the whole loop stops early.
enum Halt {
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
    env: &'a Env<'a>,
    /// The waits in a row for a usage limit (decision 6).
    usage_waited: Duration,
    /// Warnings already given, so a pause's polls don't repeat them.
    warned: HashSet<String>,
    log: PathBuf,
}

impl Loop<'_> {
    fn now(&self) -> u64 {
        (self.env.now)()
    }

    fn warn_once(&mut self, warn: &mut dyn Write, text: String) -> Result<()> {
        if self.warned.insert(text.clone()) {
            writeln!(warn, "warning: {text}")?;
        }
        Ok(())
    }

    /// The open PRs the loop's tasks opened that await the user (decision 3): every task record
    /// naming a PR that is open and has no auto-merge armed. A PR that can't be read (gone, or
    /// `gh` failing) is left out with a warning.
    fn awaiting_prs(&mut self, warn: &mut dyn Write) -> Result<Vec<u32>> {
        let mut awaiting = Vec::new();
        for dir in record::task_folders(&self.base_dir)? {
            if check_own_folder(&self.base_dir, &dir).is_err() {
                continue;
            }
            let Ok(rec) = record::read(&dir.join("task.json")) else {
                continue;
            };
            let Some(url) = rec.pr.as_deref() else {
                continue;
            };
            let Some(number) = queue::pr_number(url) else {
                self.warn_once(
                    warn,
                    format!(
                        "task {} names a PR this loop can't read the number of ({url}); not \
                         counted among the PRs awaiting you",
                        rec.id
                    ),
                )?;
                continue;
            };
            match self.github.pr_merge_state(&rec.repo, number) {
                Ok(state) if state.state == PrState::Open && !state.auto_merge => {
                    awaiting.push(number);
                }
                Ok(_) => {}
                Err(e) => self.warn_once(
                    warn,
                    format!(
                        "task {} names PR #{number}, which can't be read ({e:#}); not counted \
                         among the PRs awaiting you",
                        rec.id
                    ),
                )?,
            }
        }
        awaiting.sort_unstable();
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

    /// Waits while the disk is low (decision 245(g)); returns how long it waited.
    fn wait_for_room(&mut self, out: &mut dyn Write) -> Result<u64> {
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
        }
        Ok(self.now().saturating_sub(start))
    }

    fn task_dir(&self, issue: u32) -> PathBuf {
        record::task_dir(&self.base_dir, &format!("issue-{issue}"))
    }

    fn record(&self, issue: u32) -> Option<Record> {
        record::read(&self.task_dir(issue).join("task.json")).ok()
    }

    /// The next issue to start and the labeled ones passed over on the way (decision 2, 245(e)):
    /// one whose `Depends on:` names an unmerged issue or PR is skipped, one that `task run`
    /// would refuse before any write is reported. `None` when nothing is left.
    fn next(
        &mut self,
        open: &[Issue],
        named: &[u32],
        tried: &mut HashSet<u32>,
        skipped: &mut Vec<Skipped>,
        out: &mut dyn Write,
    ) -> Result<Option<u32>> {
        let candidates = if named.is_empty() {
            queue::queue_order(open)
        } else {
            named.to_vec()
        };
        for number in candidates {
            if !tried.insert(number) {
                continue;
            }
            let title = open
                .iter()
                .find(|i| i.number == number)
                .map_or_else(String::new, |i| format!(" {}", i.title));
            let body = open
                .iter()
                .find(|i| i.number == number)
                .map_or("", |i| i.body.as_str());
            let unmerged = self.unmerged_dependencies(open, body);
            if !unmerged.is_empty() {
                let reason = format!("depends on {}, not merged", unmerged.join(", "));
                writeln!(out, "skipped #{number}{title}: {reason}")?;
                skipped.push(Skipped {
                    issue: number,
                    reason,
                });
                continue;
            }
            let problems =
                pipeline::preflight_issues(self.config_dir, &self.repo, self.github, &[number])?;
            if let Some(reason) = problems.get(&number) {
                let reason = format!("refused before any write: {reason}");
                writeln!(out, "skipped #{number}{title}: {reason}")?;
                skipped.push(Skipped {
                    issue: number,
                    reason,
                });
                continue;
            }
            return Ok(Some(number));
        }
        Ok(None)
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

    /// One task, to its PR or its parking (decision 245(d)).
    fn task(
        &mut self,
        issue: u32,
        stops: &mut Vec<(String, u64)>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> std::result::Result<End, Halt> {
        let io = |e: std::io::Error| Halt::Error(e.into());
        let mut failures = 0;
        let mut pending: Option<(String, u64)> = None;
        loop {
            let paused = self.wait_for_room(out).map_err(Halt::Error)?;
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
                let result = if resuming {
                    self.steps.resume(issue, &mut out, &mut warn)
                } else {
                    self.steps.run(issue, &mut out, &mut warn)
                };
                captured.extend(out.text);
                captured.extend(warn.text);
                result
            };
            let ready = self.record(issue).filter(|r| r.stage == Stage::Ready);
            let e = match (result, ready) {
                (Ok(()), Some(_)) => {
                    self.usage_waited = Duration::ZERO;
                    return self.finish(issue, out, warn).map_err(Halt::Error);
                }
                (Ok(()), None) => anyhow!("the task did not reach ready"),
                (Err(e), _) => e,
            };
            let text = format!("{e:#}\n{}", String::from_utf8_lossy(&captured));
            if e.is::<pipeline::MissingSecret>() {
                return Err(Halt::Error(e));
            }
            if queue::is_usage_limit(&text) {
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
                (self.env.sleep)(wait);
                self.usage_waited += wait;
                stops.push(("usage limit".to_owned(), wait.as_secs()));
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
                    outcome: format!("parked at {stage}"),
                });
            }
            writeln!(out, "#{issue}: stopped ({e:#}); resuming it once").map_err(io)?;
            pending = Some((cause, self.now()));
            if (self.env.stop_requested)() {
                return Ok(End::Parked {
                    outcome: "stopped on request before its resume".to_owned(),
                });
            }
        }
    }

    /// A ready task: `task finish`, then for a clean one its findings filed.
    fn finish(&mut self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<End> {
        let ready = self.now();
        let rec = self.record(issue);
        let must_fix = rec
            .as_ref()
            .and_then(|r| r.review.as_ref())
            .map_or(0, |r| r.must_fix);
        let stopped = rec.as_ref().and_then(|r| r.stopped);
        let clean = must_fix == 0 && stopped.is_none();
        if let Err(e) = self.steps.finish(issue, out, warn) {
            writeln!(
                out,
                "#{issue}: parked at ready, its finish failed: {e:#}; run `sbxm task finish \
                 --issue {issue}`"
            )?;
            return Ok(End::Parked {
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
        let outcome = if clean {
            if let Err(e) = self.steps.file_findings(issue, out, warn) {
                writeln!(
                    warn,
                    "warning: #{issue}'s findings were not filed: {e:#}; run `sbxm task \
                     file-findings --issue {issue} --create`"
                )?;
            }
            format!("{pr}, clean")
        } else {
            let why = match stopped {
                Some(stopped) => format!("work stop: {}", stopped.name()),
                None => "work stop".to_owned(),
            };
            format!("draft {pr}, {why}, must-fix {must_fix}")
        };
        Ok(End::Done { ready, outcome })
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
        env,
        usage_waited: Duration::ZERO,
        warned: HashSet::new(),
        log: opts
            .repo_root
            .join("sdlc")
            .join("batches")
            .join(format!("{date}-loop.md")),
    };

    let mut tried = HashSet::new();
    let mut skipped = Vec::new();
    let mut tasks_run = 0u32;
    let mut parked_in_row = 0;
    let mut last_end: Option<u64> = None;
    loop {
        if (env.stop_requested)() {
            writeln!(out, "stopped on request")?;
            return Ok(());
        }
        if opts.cap.is_some_and(|cap| tasks_run >= cap) {
            writeln!(out, "--cap {} reached", opts.cap.unwrap_or_default())?;
            return Ok(());
        }
        if !lp.wait_for_prs(out, warn)? {
            writeln!(out, "stopped on request")?;
            return Ok(());
        }
        let open = github.issues_open(&lp.repo)?;
        let Some(issue) = lp.next(&open, &opts.issues, &mut tried, &mut skipped, out)? else {
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
            return Ok(());
        };
        let found = open.iter().find(|i| i.number == issue);
        let title = found.map_or_else(|| format!("issue {issue}"), |i| i.title.clone());
        let size = found
            .and_then(|i| queue::size(&i.labels))
            .map(str::to_owned);
        if found.is_some_and(|i| i.labels.iter().any(|l| l == READY_LABEL))
            && let Err(e) = github.issue_labels(&lp.repo, issue, &[], &[READY_LABEL.to_owned()])
        {
            lp.warn_once(
                warn,
                format!("#{issue}'s {READY_LABEL} label could not be removed: {e:#}"),
            )?;
        }

        let started = lp.now();
        writeln!(
            out,
            "task {}: #{issue} {title} (size {})",
            tasks_run + 1,
            size.as_deref().unwrap_or("unknown")
        )?;
        let mut stops = Vec::new();
        let end = match lp.task(issue, &mut stops, out, warn) {
            Ok(end) => end,
            Err(Halt::Error(e)) => return Err(e),
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
                        idle_before: last_end.map(|end| started.saturating_sub(end)),
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
                skipped.push(Skipped {
                    issue,
                    reason: format!("refused before any write: {reason}"),
                });
                continue;
            }
            End::Done { ready, outcome } => (Some(ready), outcome, false),
            End::Parked { outcome } => (None, outcome, true),
        };
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
            idle_before: last_end.map(|end| started.saturating_sub(end)),
        };
        writeln!(out, "#{issue}: {outcome}")?;
        lp.append(&row, warn)?;
        last_end = Some(lp.now());
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
        let log: Arc<Mutex<RunLog>> =
            task_log::open(self.config_dir, vec![format!("issue-{issue}")], None)
                .unwrap_or_else(|_| task_log::none());
        let mut out = Tee::new(out, Arc::clone(&log));
        let mut warn = Tee::new(warn, log);
        let result = step(&mut out, &mut warn);
        if let Err(e) = &result {
            let _ = writeln!(warn, "Error: {e:#}");
        }
        result
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
        if findings::parse(&text).findings.is_empty() {
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
                    only: Vec::new(),
                },
                self.github,
                out,
                warn,
            )
        })
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
