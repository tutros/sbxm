//! `sbxm task loop [--cap N] [--issue N ...]` (issue 320, decision 245 (b) to (h)): works through
//! the open issues labeled `ready-for-worker` one task at a time, end to end: `task run`, then
//! `task finish` (a draft PR for a work stop), then `task file-findings --create` for a clean
//! task; with `[loop] should_fix = "fix"` a clean task with should-fix findings sized S first gets
//! one fix round on them, `task resume --rounds 1 --should-fix` (decision 306, issue 510). It
//! pauses while 5 PRs await the user or the disk is low, resumes a task once after an
//! environment stop and parks it after a second, waits out a usage limit, and stops on an empty
//! queue (with a read-only report), two parked tasks in a row, `--cap`, the usage waits' total,
//! or a stop request between phases. One log row per task goes to the loop's own file under
//! `<base>/.sbxm/loop-logs/`, posted at the stop to the open `loop-log` issue (decision 305,
//! [`loop_log`]). Before each new issue it resolves the open PRs it opened that
//! GitHub reports conflicting with their base, with `task fix --pr N` (decision 298), then reads
//! the workflow runs of its PRs and of `main` (decision 299): a failed run is re-run once; a
//! second failure disarms the PR, or on `main` sets the pause that stops all arming until the
//! user runs `task loop --clear-pause`. A PR's second failure first gets one log-fix, ever, with
//! `task fix --pr N --check-run <id>` (decision 300).
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
use super::{
    task_file_findings, task_finish, task_fix, task_log, task_resume, task_review, task_rm,
    task_run, template_build,
};
use crate::backend::{Location, SandboxBackend};
use crate::config::GlobalConfig;
use crate::disk;
use crate::github::{GitHubBackend, Issue, Mergeable, PrState, RunStatus, WorkflowRun};
use crate::harness::Harness;
use crate::notify::{self, Notification, Notifier, Shown};
use crate::run::results::format_timestamp;
use crate::task::config::{ShouldFix, TaskConfig};
use crate::task::finish::check_own_folder;
use crate::task::gates::HostRunner;
use crate::task::loop_pause::{self, Pause};
use crate::task::queue::{self, Outcome, READY_LABEL, Report, Row, Skipped};
use crate::task::record::{self, Kind, Process, ProcessProbe, Record, Stage};
use crate::task::risk::Level;
use crate::task::runlog::{RunLog, Tee};
use crate::task::{
    findings, finish, lock, loop_log, medium, next, pipeline, pr_fix, real_check, resume,
};

mod check;
mod jobs;
pub use check::check;
pub(crate) use check::{reasons, skip_reason};

/// How many open PRs the loop opened may await the user before it pauses (decision 245(e)).
pub const PR_CAP: usize = 5;
pub use queue::LOOP_HOOK;
/// The key in a task record's `hooks` for a task a loop was running when it died (decision 310):
/// the dead process while its one resume is owed, `true` once the loop took it. One cut off again
/// after that is parked, never resumed a second time.
pub const RESUMED_HOOK: &str = "task_loop_resumed";

/// What a task record's [`RESUMED_HOOK`] says.
enum Owed {
    /// Nothing: no loop owes it a resume.
    None,
    /// Its resume is owed, for the cut-off process named.
    Due(Process),
    /// The loop took its resume already.
    Taken,
}

impl Owed {
    fn of(rec: &Record) -> Self {
        match rec.hooks.get(RESUMED_HOOK) {
            Some(serde_json::Value::Bool(true)) => Self::Taken,
            Some(value) => serde_json::from_value(value.clone()).map_or(Self::None, Self::Due),
            None => Self::None,
        }
    }
}
/// How often a pause looks again.
pub const PAUSE_POLL: Duration = Duration::from_secs(5 * 60);
/// How often a task's first phase is looked at for its record, to take its label off.
const LABEL_POLL: Duration = Duration::from_secs(2);
/// Why an issue whose body still has a `## Open points` heading is passed over (issue 511).
const OPEN_POINTS: &str = "open points not answered";

pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is (the loop writes nothing there).
    pub repo_root: PathBuf,
    /// `--issue N`, in the user's order; empty means the `ready-for-worker` queue.
    pub issues: Vec<u32>,
    /// `--cap N`: at most this many tasks.
    pub cap: Option<u32>,
    /// For tests: the GitHub repo, `owner/name`, instead of the checkout's origin.
    pub repo: Option<String>,
    /// `--wait` (issue 527, decision 310): on an empty queue, check again every [`PAUSE_POLL`]
    /// for a job file or a labelled issue instead of stopping, until a stop request.
    pub wait: bool,
}

/// One task's phases, as the loop runs them. Each writes its own screen output.
pub trait Steps {
    /// `task run --issue N`: start, gates, review and fix rounds, to `ready` or a stop.
    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task resume --issue N`: the task again from its recorded stage, to `ready`.
    fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task resume --issue N --rounds 1 --should-fix` (issue 510, decision 306): one fix round
    /// on a clean ready task's should-fix findings sized S, to `ready` again. Only called with
    /// `[loop] should_fix = "fix"`; a `Steps` that never runs with it needs no other.
    fn should_fix(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        bail!("no should-fix round for #{issue} here")
    }
    /// `task finish --issue N`: its PR, a draft when something is left unresolved.
    fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task file-findings --issue N --create`: the review's should-fix findings as sized
    /// issues, without the loop's label; nothing when the review has none.
    fn file_findings(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task fix --pr N`: the base merged into the PR's branch in a sandbox, the gates and a
    /// narrow review, then the push (decisions 292, 298).
    fn fix_pr(&self, pr: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task fix --pr N --check-run <run>`: the PR's failed run fixed from its log in a sandbox,
    /// the gates and the in-task review, then the push (decision 300).
    fn fix_check(&self, pr: u32, run: u64, out: &mut dyn Write, warn: &mut dyn Write)
    -> Result<()>;
    /// `task rm --merged --yes`: removes the tasks whose PRs GitHub merged, archiving their text
    /// records first (decisions 195, 293).
    fn rm_merged(&self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()>;
    /// `task review --pr N`, for a queued `review` job (issue 525). A `Steps` that never runs
    /// one needs no other.
    fn review_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        bail!("no review of PR #{pr} here")
    }
    /// `template build <profile> --harness <harness>`, with `warm` `--warm <repo>`, for a queued
    /// `template` job (issue 524): the new template's tag. A `Steps` that never runs one needs no
    /// other.
    fn template_build(
        &self,
        profile: &str,
        _harness: Harness,
        _warm: Option<&Path>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<String> {
        bail!("no template build of {profile} here")
    }
    /// `task run --spec FILE`, for a queued `spec` job (issue 524): to `ready` or a stop. A
    /// `Steps` that never runs one needs none of the spec steps.
    fn run_spec(&self, spec: &Path, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        bail!("no spec task of {} here", spec.display())
    }
    /// `task resume --spec FILE --rounds 1 --should-fix`: a spec job's should-fix round
    /// (decision 306).
    fn should_fix_spec(
        &self,
        spec: &Path,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        bail!("no should-fix round of {} here", spec.display())
    }
    /// `task finish --spec FILE`: through the `[finish] sink` of the checkout's `sbxm-task.toml`.
    fn finish_spec(&self, spec: &Path, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        bail!("no finish of {} here", spec.display())
    }
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
    file: queue::LockFile,
}

impl LoopLock {
    /// Changes the progress a loop started with `--issue` records (issue 493) and rewrites the
    /// lock, through a temporary file renamed over it, since `sbxm status` reads it meanwhile.
    /// A labeled-queue loop records none, so nothing is written. A lock no longer this process's
    /// (removed by hand) is not written again.
    fn record(&mut self, change: impl FnOnce(&mut queue::Progress)) -> Result<()> {
        let Some(progress) = self.file.progress.as_mut() else {
            return Ok(());
        };
        change(progress);
        if !fs::read_to_string(&self.path).is_ok_and(|text| text == self.me) {
            return Ok(());
        }
        let me = serde_json::to_string(&self.file)?;
        let tmp = self.path.with_extension("lock.tmp");
        fs::write(&tmp, &me).with_context(|| format!("cannot write {}", tmp.display()))?;
        if let Err(e) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("cannot write {}", self.path.display()));
        }
        self.me = me;
        Ok(())
    }
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
/// A loop started with `--issue` writes its list and its `--cap` into the lock too, for `sbxm
/// status` (issue 477); a labeled-queue loop writes its process alone, as before.
/// Returns the lock and, when it took over a lock whose process is gone, that dead loop's
/// process (decision 310: the tasks it was running are resumed).
fn take_lock(
    base_dir: &Path,
    opts: &Options,
    probe: &dyn ProcessProbe,
    warn: &mut dyn Write,
) -> Result<(LoopLock, Option<Process>)> {
    let path = queue::lock_path(base_dir);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let listed = !opts.issues.is_empty();
    let contents = queue::LockFile {
        process: Process::current(probe),
        issues: opts.issues.clone(),
        cap: opts.cap.filter(|_| listed),
        progress: listed.then(queue::Progress::default),
    };
    let me = serde_json::to_string(&contents)?;
    let mut dead = None;
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
                    file: contents.clone(),
                };
                file.write_all(me.as_bytes())
                    .with_context(|| format!("cannot write {}", lock.path.display()))?;
                return Ok((lock, dead));
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
            dead = Some(held);
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

/// The line for a task a command run by hand finished, into `pr` when one is recorded; `named`
/// is its issue with the title (issue 246).
fn finished_by_hand(named: &str, pr: Option<&str>) -> String {
    let into = pr.map_or_else(String::new, |url| format!(" into {url}"));
    format!("{named}: finished by a command run by hand{into}; the loop leaves it to that command")
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
    /// Cut off when a loop died (decision 310): marked as taken when the loop takes it.
    cut_off: bool,
}

impl Held {
    /// The line naming what is left when the loop stops before trying it again.
    fn left(&self) -> String {
        let issue = self.issue;
        if self.cut_off {
            return format!(
                "{} was cut off when a loop stopped and not resumed yet; the next loop resumes it, \
                 or continue it with `sbxm task resume --issue {issue}`",
                crate::title::named(&format!("#{issue}"), &self.title)
            );
        }
        let command = match self.retry {
            Retry::Finish { .. } => "finish",
            Retry::Resume { .. } => "resume",
        };
        format!(
            "{} was not tried again after its lock was held; continue it with `sbxm task \
             {command} --issue {issue}`",
            crate::title::named(&format!("#{issue}"), &self.title)
        )
    }
}

/// The line for a task whose `phase` was skipped because a live command holds its lock; `named`
/// is its issue with the title (issue 246).
fn held_line(named: &str, phase: &str, e: &anyhow::Error) -> String {
    let why = e
        .downcast_ref::<lock::InUse>()
        .map_or_else(|| format!("{e:#}"), lock::InUse::describe);
    format!(
        "{named}: its `task {phase}` is skipped for now: {why}; the loop tries it again after \
         its next task"
    )
}

/// Why the whole loop stops early.
enum Halt {
    /// A stop was asked before the task started: nothing to log.
    Stopped,
    UsageLimit(Duration),
    /// A lapsed sign-in, `sbx`'s or a harness's (issue 445): it never fixes itself.
    SignIn(anyhow::Error),
    Error(anyhow::Error),
}

struct Loop<'a> {
    config_dir: &'a Path,
    /// The checkout the loop runs from: a queued warm template build warms from it (issue 524).
    repo_root: PathBuf,
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
    /// The commits of `main` watched until each of their runs is over and acted on (review M-1
    /// to M-3): merge commits of PRs this run armed, and every tip seen.
    watched: Vec<Watched>,
    /// The runs whose failed jobs this run re-ran, so a lagging answer isn't re-run twice.
    reruns: HashSet<u64>,
    /// The (PR, run) second failures already acted on: said once.
    failed_twice: HashSet<(u32, u64)>,
    /// The PRs a log-fix was tried for in this run (decision 300), whether or not it got as far
    /// as a task record: one per PR, ever, with the records counting across runs.
    log_fixed: HashSet<u32>,
    /// The PRs disarmed on a second failure whose log-fix is still to come (review M-1): they
    /// count as armed for its re-arm (decision 298).
    disarmed_for_log_fix: HashSet<u32>,
    /// The (commit, run) second failures on `main` already paused for (or found paused): a pause
    /// the user cleared isn't set again for the same run.
    paused_for: HashSet<(String, u64)>,
    log: PathBuf,
    /// Whether this loop wrote a row to [`log`](Self::log): only then is it posted.
    logged: bool,
    /// The notifier and the keys shown (issue 530); `None` with `[notify] toasts = false` or no
    /// notifier wired: nothing shown, nothing written.
    notify: Option<(&'a dyn Notifier, Shown)>,
}

impl Loop<'_> {
    fn now(&self) -> u64 {
        (self.env.now)()
    }

    /// Shows `note` unless its key was shown before (issue 530, answer 4). A notification that
    /// fails to show is one warning, and its key is recorded all the same (answer 5).
    fn notify(
        &mut self,
        note: Notification,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let Some((notifier, shown)) = self.notify.as_mut() else {
            return Ok(());
        };
        if shown.contains(&note.key) {
            return Ok(());
        }
        if let Err(e) = notifier.show(&note, out) {
            writeln!(
                warn,
                "warning: the notification \"{}\" could not be shown: {e:#}; the loop goes on",
                note.title
            )?;
        }
        if let Err(e) = shown.record(&note.key) {
            self.warn_once(
                warn,
                format!("{e:#}; a notification may be shown again after a restart"),
            )?;
        }
        Ok(())
    }

    /// What a notification's click opens when it has no URL: the loop's log file.
    fn log_click(&self) -> String {
        notify::file_url(&self.log)
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
                format!(
                    "{issue_named}'s {READY_LABEL} label could not be removed: {e:#}",
                    issue_named = self.issue_named(issue)
                ),
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
                        "task {} names {pr_named}, which can't be read ({e:#}); not counted \
                         among the PRs awaiting you",
                        pr.task,
                        pr_named = self.pr_named(number)
                    ),
                )?,
            }
        }
        awaiting.sort_unstable();
        awaiting.dedup();
        Ok(awaiting)
    }

    /// (a) of issue 530: a notification for each PR a loop opened or continued that awaits the
    /// user's merge because the loop won't arm it: a draft, or a risk above low after the path
    /// rules. Keyed by the PR and its head, so a new push after a fix round notifies again. A PR
    /// that can't be read is left for the next step ([`awaiting_prs`](Self::awaiting_prs) warns
    /// about it).
    fn notify_awaiting(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        if self.notify.is_none() {
            return Ok(());
        }
        for pr in queue::loop_finished_prs(&self.base_dir, &self.repo)? {
            let Some(number) = pr.number else {
                continue;
            };
            let Ok(state) = self.github.pr_merge_state(&self.repo, number) else {
                continue;
            };
            if !queue::awaits_user(state) {
                continue;
            }
            let Some(own) = self.record_of(&pr.task) else {
                continue;
            };
            let head = match self.github.pr(&self.repo, number) {
                Ok(info) => info.head_oid,
                Err(e) => {
                    self.warn_once(
                        warn,
                        format!(
                            "{pr_named}'s head can't be read ({e:#}); its notification waits for \
                             the next step",
                            pr_named = self.pr_named(number)
                        ),
                    )?;
                    continue;
                }
            };
            // The PR's draft state is that of the task whose review covered its head (review M-1
            // of issue 530): a resolve or log-fix pushed onto it, else the task that opened it.
            // Its risk is the highest of its whole review history, the task that opened it and
            // every fix pushed onto it: a later narrow review doesn't clear the risk an earlier
            // one left (decision 298(3), second review of issue 530).
            let risk = self.history_risk(number, Some(&own));
            let check = real_check_of(&self.base_dir, Some(&own)).ok().flatten();
            let rec = self
                .fixes_of(number)
                .into_iter()
                .find(|fix| {
                    fix.review
                        .as_ref()
                        .is_some_and(|r| r.commit.as_deref() == Some(head.as_str()))
                })
                .unwrap_or(own);
            let Some(review) = rec.review.as_ref() else {
                continue;
            };
            let risk = risk
                .into_iter()
                .chain([review.risk])
                .max_by_key(|&risk| awaiting_rank(risk))
                .unwrap_or(review.risk);
            let (what, reason, next) =
                if state.draft || rec.stopped.is_some() || review.must_fix > 0 {
                    let reason = match rec.stopped {
                        Some(stopped) => format!("a draft: work stop {}", stopped.name()),
                        None => format!("a draft with {} must-fix left", review.must_fix),
                    };
                    let next = format!("see `sbxm task status {}`", next::record_flag(&rec));
                    ("draft", reason, next)
                } else if risk == Level::Low {
                    // Held for its real check (issue 547): one named, and no `success` on the head.
                    let Some(check) = check else {
                        continue;
                    };
                    match self
                        .github
                        .commit_status_state(&self.repo, &head, real_check::CONTEXT)
                    {
                        Ok(Some(state)) if state == "success" => continue,
                        Ok(_) => {}
                        // Read again at the next step.
                        Err(_) => continue,
                    }
                    let next =
                        format!("run it, then sbxm task real-check --pr {number} --pass \"...\"");
                    ("real check", format!("real check: {check}"), next)
                } else {
                    let word = risk.word();
                    let reason =
                        format!("risk {word} after the path rules, so the loop doesn't arm it");
                    (word, reason, "review it, then merge or close it".to_owned())
                };
            let note = Notification {
                key: format!("pr:{number}:{head}"),
                title: format!("PR #{number} awaits your merge ({what})"),
                body: format!("{reason}; next: {next}"),
                click: pr.url.clone(),
            };
            self.notify(note, out, warn)?;
        }
        Ok(())
    }

    /// The highest risk of PR `number`'s review history after the path rules: its own task's
    /// review (`own`) and every finished fix pushed onto it (decision 298(3)); `None` with no
    /// review. A later narrow review doesn't clear the risk an earlier one left.
    fn history_risk(&self, number: u32, own: Option<&Record>) -> Option<Level> {
        self.fixes_of(number)
            .iter()
            .filter(|fix| fix.stage == Stage::Finished)
            .chain(own)
            .filter_map(|rec| rec.review.as_ref().map(|r| r.risk))
            .max_by_key(|&risk| awaiting_rank(risk))
    }

    /// The fix tasks of PR `number` (resolves and log-fixes, `fix-<n>-<head>`) of this repo, in
    /// task folders that are sbxm's own; a record that can't be read is left out.
    fn fixes_of(&self, number: u32) -> Vec<Record> {
        let prefix = format!("fix-{number}-");
        record::task_folders(&self.base_dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|dir| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|id| id.starts_with(&prefix))
                    && check_own_folder(&self.base_dir, dir).is_ok()
            })
            .filter_map(|dir| record::read(&dir.join("task.json")).ok())
            .filter(|rec| rec.kind == Kind::Fix && rec.repo == self.repo)
            .collect()
    }

    /// (e) of issue 530: a notification for each question task `id` hands the user, once per
    /// task and question: its worker's "Needs a decision" in `result.md`, and each question
    /// finding of its reviewer's `review.md` (review M-2). The click opens its PR, else its issue.
    fn notify_question(
        &mut self,
        id: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        if self.notify.is_none() {
            return Ok(());
        }
        let dir = record::task_dir(&self.base_dir, id);
        // (what hashes into the key, the body's line, where to answer it)
        let mut questions: Vec<(String, String, &str)> = Vec::new();
        if let Some(question) = fs::read_to_string(dir.join("result.md"))
            .ok()
            .and_then(|text| finish::handed_back(&text))
        {
            let first = question
                .lines()
                .map(|line| line.trim().trim_start_matches(['-', '*']).trim())
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_owned();
            questions.push((question, first, "the PR's \"Needs a decision\""));
        }
        if let Ok(text) = fs::read_to_string(dir.join("review.md")) {
            for finding in findings::parse(&text).findings {
                if finding.label == "question" {
                    let line = format!("{} {}", finding.id, finding.title);
                    questions.push((format!("review: {line}"), line, "the PR's review"));
                }
            }
        }
        if questions.is_empty() {
            return Ok(());
        }
        let click = match self.record_of(id) {
            Some(Record { pr: Some(url), .. }) => url,
            Some(rec) if matches!(rec.kind, Kind::Pr | Kind::Fix) => {
                format!("https://github.com/{}/pull/{}", rec.repo, rec.number)
            }
            Some(rec) if rec.kind == Kind::Issue => {
                format!("https://github.com/{}/issues/{}", rec.repo, rec.number)
            }
            _ => self.log_click(),
        };
        for (keyed, line, place) in questions {
            let hash = crate::config::sha256_hex(keyed.as_bytes());
            let note = Notification {
                key: format!("question:{id}:{}", &hash[..16]),
                title: format!("Task {id} has a question for you"),
                body: format!("{line}; next: answer it in {place}"),
                click: click.clone(),
            };
            self.notify(note, out, warn)?;
        }
        Ok(())
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
    fn stopped(&self, issue: u32, out: &mut dyn Write) -> std::io::Result<End> {
        writeln!(
            out,
            "{}: stopped on request; continue it with `sbxm task resume --issue {issue}`",
            self.issue_named(issue)
        )?;
        Ok(End::Parked {
            ready: None,
            outcome: "stopped on request before its resume".to_owned(),
        })
    }

    fn task_dir(&self, issue: u32) -> PathBuf {
        record::task_dir(&self.base_dir, &format!("issue-{issue}"))
    }

    /// `PR #9 (Add titles)`: the PR with the title of the task record that names it (issue 246),
    /// or its number alone once that record is gone.
    fn pr_named(&self, number: u32) -> String {
        crate::title::pr_named(&self.base_dir, &self.repo, number)
    }

    /// `#41 (Add titles)`: the issue with its task's title (issue 246), or its number alone
    /// without a task record.
    fn issue_named(&self, issue: u32) -> String {
        let title = self.record(issue).map(|r| r.title).unwrap_or_default();
        crate::title::named(&format!("#{issue}"), &title)
    }

    /// A PR's log row text (the row shows `#<number>` before it): `what`, after the PR's title
    /// in brackets when a task record still names it (issue 246), as [`Self::pr_named`] shows it.
    fn pr_row(&self, number: u32, what: String) -> String {
        match crate::title::pr_title(&self.base_dir, &self.repo, number) {
            Some(title) => format!("({title}) {what}"),
            None => what,
        }
    }

    fn record(&self, issue: u32) -> Option<Record> {
        record::read(&self.task_dir(issue).join("task.json")).ok()
    }

    /// Whether the task's next phase is its should-fix round (issue 510, decision 306): `[loop]
    /// should_fix = "fix"`, and `task resume --rounds 1 --should-fix` would take it (it is
    /// `ready` with a clean review that has should-fix findings sized S, and had no such round).
    fn should_fix_due(&self, issue: u32) -> bool {
        self.should_fix_due_of(&format!("issue-{issue}"))
    }

    /// [`Self::should_fix_due`] of task `id`, an issue's or a queued spec job's (issue 524).
    fn should_fix_due_of(&self, id: &str) -> bool {
        self.settings.should_fix == ShouldFix::Fix
            && self.record_of(id).is_some_and(|rec| {
                let dir = record::task_dir(&self.base_dir, id);
                resume::check_should_fix(&rec, &dir, Some(1)).is_ok()
            })
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
            // The label alone isn't enough (issue 511): a worker can't answer open points.
            if queue::has_open_points(body) {
                tried.insert(number);
                let reason = OPEN_POINTS.to_owned();
                writeln!(out, "skipped #{number}{title}: {reason}")?;
                refused.push(Skipped {
                    issue: number,
                    reason,
                });
                continue;
            }
            let unmerged = self.unmerged_dependencies(open, body);
            if !unmerged.is_empty() {
                let reason = waiting_reason(&unmerged);
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
                let reason = refused_reason(reason);
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

    fn unmerged_dependencies(&self, open: &[Issue], body: &str) -> Vec<String> {
        unmerged_dependencies(self.github, &self.repo, open, body)
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
        // Whether this call already ran the task's should-fix round (issue 510): once it ended
        // `ready`, never again, whatever its record says. One that failed is tried again while
        // it is still due, as any failed phase is (once, then parked).
        let mut should_fix_ran = false;
        loop {
            let Some(paused) = self.wait_for_room(out).map_err(Halt::Error)? else {
                if self.record(issue).is_none() {
                    return Err(Halt::Stopped);
                }
                return self.stopped(issue, out).map_err(io);
            };
            if paused > 0 {
                stops.push(("disk".to_owned(), paused));
            }
            if let Some((cause, since)) = pending.take() {
                stops.push((cause, self.now().saturating_sub(since)));
            }
            let resuming = self.record(issue).is_some();
            let should_fix = self.should_fix_due(issue);
            should_fix_ran |= should_fix;
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
                let (result, removed) = if should_fix {
                    (self.steps.should_fix(issue, &mut out, &mut warn), None)
                } else if resuming {
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
                    // A clean task with should-fix findings sized S gets its one should-fix
                    // round first (decision 306), as the next phase of this loop.
                    if !should_fix_ran && self.should_fix_due(issue) {
                        continue;
                    }
                    return self.finish(issue, None, out, warn).map_err(Halt::Error);
                }
                (Ok(()), None) => anyhow!("the task did not reach ready"),
                (Err(e), _) => e,
            };
            // A live command run by hand holds the task (issue 409): its resume is skipped, not
            // failed, and the loop moves on.
            if resuming && e.is::<lock::InUse>() {
                writeln!(out, "{}", held_line(&self.issue_named(issue), "resume", &e))
                    .map_err(io)?;
                return Ok(End::Held(Retry::Resume { failures }));
            }
            if e.is::<pipeline::MissingSecret>() {
                return Err(Halt::Error(needs_user(Cause::Secret, e)));
            }
            if says_sign_in(&e, &String::from_utf8_lossy(&captured)) {
                stops.push(("sign-in".to_owned(), 0));
                return Err(Halt::SignIn(e));
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
                    "{issue_named}: usage limit; waiting {} before `task resume` ({} of {} waited in a \
                     row)",
                    queue::span(wait.as_secs()),
                    queue::span(self.usage_waited.as_secs()),
                    queue::span(total.as_secs()), issue_named = self.issue_named(issue))
                .map_err(io)?;
                let (slept, asked) = self.usage_sleep(wait);
                self.usage_waited += slept;
                stops.push(("usage limit".to_owned(), slept.as_secs()));
                if asked {
                    return self.stopped(issue, out).map_err(io);
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
                    "{issue_named}: parked at {stage} after a second failure: {e:#}; see `sbxm task \
                     status --issue {issue}`", issue_named = self.issue_named(issue))
                .map_err(io)?;
                return Ok(End::Parked {
                    ready: None,
                    outcome: format!("parked at {stage}"),
                });
            }
            writeln!(
                out,
                "{issue_named}: stopped ({e:#}); resuming it once",
                issue_named = self.issue_named(issue)
            )
            .map_err(io)?;
            pending = Some((cause, self.now()));
            if (self.env.stop_requested)() {
                return self.stopped(issue, out).map_err(io);
            }
        }
    }

    /// The tasks a loop was running when it died (decision 310), to be continued once, as the
    /// loop's one resume after an environment stop: this repo's issue tasks not finished and
    /// interrupted as `task status` decides it, that are either recorded as worked on by `dead`,
    /// the loop whose lock this loop took over, or still owed their resume from an earlier start
    /// ([`Owed`]). A newly found one is marked as owed, under its lock, before anything else
    /// runs, so a loop that stops or fails before the resume leaves it to the next loop. One whose
    /// resume was taken already, and was cut off again, is parked with its log row instead.
    fn cut_off(
        &mut self,
        dead: Option<&Process>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<VecDeque<Held>> {
        let mut found = Vec::new();
        for dir in record::task_folders(&self.base_dir)? {
            if check_own_folder(&self.base_dir, &dir).is_err() {
                continue;
            }
            let Ok(rec) = record::read(&dir.join("task.json")) else {
                continue;
            };
            let ours = rec.kind == Kind::Issue
                && rec.repo == self.repo
                && rec.id == format!("issue-{}", rec.number)
                && rec.stage != Stage::Finished
                && rec.pr.is_none()
                && rec.is_interrupted(self.probe);
            let owed = Owed::of(&rec);
            let by_dead = dead.is_some() && rec.process.as_ref() == dead;
            // Owed for the process still recorded: a command run by hand since then took it.
            let owed_here = matches!(&owed, Owed::Due(p) if rec.process.as_ref() == Some(p));
            if ours && (by_dead || owed_here) {
                found.push((rec, owed));
            }
        }
        found.sort_by_key(|(rec, _)| rec.number);
        let mut due = VecDeque::new();
        for (rec, owed) in found {
            let issue = rec.number;
            let named = crate::title::named(&format!("#{issue}"), &rec.title);
            let stage = rec.stage.name();
            match owed {
                Owed::Taken => {
                    writeln!(
                        out,
                        "{named}: cut off at {stage} again after the loop resumed it once; \
                         parked, see `sbxm task status --issue {issue}`"
                    )?;
                    let now = self.now();
                    self.append(
                        &Row {
                            issue,
                            title: rec.title.clone(),
                            size: None,
                            started: now,
                            ready: None,
                            worker_s: None,
                            reviewer_s: None,
                            rounds: None,
                            must_fix: None,
                            outcome: format!("parked at {stage}: cut off again after its resume"),
                            stops: Vec::new(),
                            idle_before: None,
                        },
                        warn,
                    )?;
                    continue;
                }
                Owed::Due(_) => {}
                Owed::None => {
                    let Some(process) = rec.process.as_ref() else {
                        continue;
                    };
                    let owe = serde_json::to_value(process)?;
                    if let Err(e) = self.mark_resume(issue, owe) {
                        let Some(in_use) = e.downcast_ref::<lock::InUse>() else {
                            return Err(e);
                        };
                        writeln!(out, "{named}: {}; the loop leaves it", in_use.describe())?;
                        continue;
                    }
                }
            }
            writeln!(
                out,
                "{named}: cut off at {stage} when a loop stopped; resuming it once"
            )?;
            due.push_back(Held {
                issue,
                title: rec.title,
                size: None,
                started: self.now(),
                stops: vec![("loop stopped".to_owned(), 0)],
                retry: Retry::Resume { failures: 1 },
                cut_off: true,
            });
        }
        Ok(due)
    }

    /// Sets the task's [`RESUMED_HOOK`] to `value` under the task's lock, as [`Self::claim`]
    /// marks one: the process its resume is owed for, or `true` once the loop takes it.
    fn mark_resume(&self, issue: u32, value: serde_json::Value) -> Result<()> {
        let _lock = pipeline::lock_task(&self.base_dir, &format!("issue-{issue}"), self.probe)?;
        let dir = self.task_dir(issue);
        let mut rec = record::read(&dir.join("task.json"))?;
        record::write_change(&dir, &mut rec, |r| {
            if !r.hooks.is_object() {
                r.hooks = serde_json::Value::Object(serde_json::Map::new());
            }
            r.hooks[RESUMED_HOOK] = value;
        })?;
        Ok(())
    }

    /// Marks a ready task as the loop's before its `task finish`, so a PR that finish opens counts
    /// toward [`PR_CAP`], after a restart too ([`opened_by_loop`]).
    /// Holds the task's lock (issue 58) around the read and the write, and gives it up before
    /// `task finish` takes its own (issue 409); a live holder refuses with [`lock::InUse`].
    /// A task already finished, by a command run by hand that held the lock, is left unmarked
    /// (review M-1 of issue 409).
    /// `id`: the task's id, `issue-<n>` or a queued spec job's (issue 524).
    fn claim(&self, id: &str) -> Result<Claimed> {
        let _lock = pipeline::lock_task(&self.base_dir, id, self.probe)?;
        let dir = record::task_dir(&self.base_dir, id);
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
                "{issue_named}: its task was removed by a command run by hand; the loop leaves it",
                issue_named = self.issue_named(issue)
            )));
        };
        if rec.pr.is_some() || rec.stage == Stage::Finished {
            return Ok(End::Taken(finished_by_hand(
                &self.issue_named(issue),
                rec.pr.as_deref(),
            )));
        }
        match (rec.stage, retry) {
            // Its should-fix round comes before its finish (issue 510).
            (Stage::Ready, Retry::Finish { .. }) if self.should_fix_due(issue) => {
                self.task(issue, false, 0, stops, out, warn)
            }
            (Stage::Ready, Retry::Resume { failures }) if self.should_fix_due(issue) => {
                self.task(issue, false, failures, stops, out, warn)
            }
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
        match self.claim(&format!("issue-{issue}")) {
            Ok(Claimed::Marked) => {}
            Ok(Claimed::Finished(pr)) => {
                return Ok(End::Taken(finished_by_hand(
                    &self.issue_named(issue),
                    pr.as_deref(),
                )));
            }
            Err(e) if e.is::<lock::InUse>() => {
                writeln!(out, "{}", held_line(&self.issue_named(issue), "finish", &e))?;
                return Ok(End::Held(Retry::Finish { ready }));
            }
            Err(e) => self.warn_once(
                warn,
                format!(
                    "{issue_named} could not be marked as the loop's ({e:#}); its PR won't count \
                     among the PRs awaiting you",
                    issue_named = self.issue_named(issue)
                ),
            )?,
        }
        if let Err(e) = self.steps.finish(issue, out, warn) {
            writeln!(
                out,
                "{issue_named}: parked at ready, its finish failed: {e:#}; run `sbxm task finish \
                 --issue {issue}`",
                issue_named = self.issue_named(issue)
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
        let armed = self.arm(&format!("issue-{issue}"), out, warn)?;
        self.notify_question(&format!("issue-{issue}"), out, warn)?;
        let outcome = if clean {
            if let Err(e) = self.steps.file_findings(issue, out, warn) {
                writeln!(
                    warn,
                    "warning: {issue_named}'s findings were not filed: {e:#}; run `sbxm task \
                     file-findings --issue {issue} --create`",
                    issue_named = self.issue_named(issue)
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
    /// the review covered. A medium-risk PR's note also gives [`medium::refusals`] (issue 549).
    /// Nothing is checked on CI: GitHub's `--auto` waits for the required
    /// checks. A PR not armed stays among those awaiting the user, so every refusal is only a
    /// note. Returns the note for the loop's log row, when there is one. `id`: the task's id,
    /// `issue-<n>` or a queued spec job's (issue 524).
    fn arm(
        &mut self,
        id: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        let Some(rec) = self.record_of(id) else {
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
        // Worked out first, so a refusal that comes before its own check still names it (review
        // S-1 of issues 545 and 547): the checks not run, then the real check.
        let not_run = not_run_reason(review);
        let real = self.real_check_now(Some(&rec), number, warn)?;
        let leading = leading_reasons(not_run.as_deref(), real.as_deref());
        let waits = |why: String, out: &mut dyn Write| -> Result<Option<String>> {
            let note = leading.as_deref().map(|n| not_armed(n, Some(&why)));
            writeln!(
                out,
                "{pr_named} awaits you: {}",
                note.as_deref().unwrap_or(&why),
                pr_named = self.pr_named(number)
            )?;
            Ok(note)
        };
        if rec.stopped.is_some() || review.must_fix > 0 {
            return waits("a draft with must-fix left".to_owned(), out);
        }
        if review.risk != Level::Low {
            let reasons = self.medium_reasons(review.risk, medium_source(&rec), number);
            return waits(
                with_reasons(format!("risk {}", review.risk.word()), &reasons),
                out,
            );
        }
        if let (Some(_), Some(leading)) = (&not_run, leading.as_deref()) {
            let note = not_armed(leading, None);
            writeln!(
                out,
                "{pr_named} awaits you: {note}",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(note));
        }
        let Some(reviewed) = review.commit.as_deref() else {
            return waits("the commit the review covered is unknown".to_owned(), out);
        };
        if self.paused(warn)? {
            // Only a low-risk PR gets here: nothing else but its real check keeps it unarmed.
            let also: Vec<String> = real.iter().cloned().collect();
            let note = self.not_armed_paused(number, &also, out)?;
            return Ok(Some(with_reasons(note, &also)));
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
            let note = with_reasons(
                "auto-merge not armed: the PR's head moved after the review".to_owned(),
                &real.iter().cloned().collect::<Vec<_>>(),
            );
            writeln!(
                out,
                "{pr_named}: {note} (reviewed {reviewed}, head {head}); it awaits you",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(note));
        }
        if let Some(why) = self.real_check_refusal(Some(&rec), number, &head, warn)? {
            let note = not_armed(&why, None);
            writeln!(
                out,
                "{pr_named} awaits you: {note}",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(note));
        }
        match self.github.pr_arm_auto_merge(&self.repo, number, reviewed) {
            Ok(()) => {
                self.armed.insert(number);
                writeln!(
                    out,
                    "{pr_named}: auto-merge armed on {reviewed}",
                    pr_named = self.pr_named(number)
                )?;
                Ok(Some("auto-merge armed".to_owned()))
            }
            Err(e) => self.arming_failed(number, &e, out, warn),
        }
    }

    /// For a PR of `risk` medium, every reason of [`medium::refusals`] not to arm it besides its
    /// risk (issue 549); none for any other risk. Until #550 a medium PR is never armed either
    /// way, so these only go into its "awaits you" line.
    fn medium_reasons(&self, risk: Level, source: medium::Source, number: u32) -> Vec<String> {
        if risk == Level::Medium {
            medium::refusals(self.github, &self.repo, source, number)
        } else {
            Vec::new()
        }
    }

    /// [`Self::real_check_refusal`] on PR `number`'s head as GitHub has it now, read only when the
    /// issue of its own task `own` names a real check: for the refusals that come before arming
    /// reads the head itself (review S-1 of issue 547), so they name it too.
    fn real_check_now(
        &mut self,
        own: Option<&Record>,
        number: u32,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        match real_check_of(&self.base_dir, own) {
            Ok(None) => return Ok(None),
            Ok(Some(_)) => {}
            Err(why) => return Ok(Some(why)),
        }
        match self.github.pr(&self.repo, number) {
            Ok(pr) => self.real_check_refusal(own, number, &pr.head_oid, warn),
            Err(e) => {
                self.warn_once(
                    warn,
                    format!(
                        "{pr_named}'s head can't be read for its real check: {e:#}; it awaits you",
                        pr_named = self.pr_named(number)
                    ),
                )?;
                Ok(Some("real check status unreadable".to_owned()))
            }
        }
    }

    /// Why PR `number`'s real check keeps it unarmed, at any risk (issue 547): the issue of its
    /// own task `own` names one ([`real_check::required`]) and its head `head` hasn't
    /// [`real_check::CONTEXT`] = `success` as its latest status of that context. `None` when it
    /// may be armed as far as the real check goes.
    fn real_check_refusal(
        &mut self,
        own: Option<&Record>,
        number: u32,
        head: &str,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        match real_check_of(&self.base_dir, own) {
            Ok(None) => return Ok(None),
            Ok(Some(_)) => {}
            Err(why) => return Ok(Some(why)),
        }
        match self
            .github
            .commit_status_state(&self.repo, head, real_check::CONTEXT)
        {
            Ok(Some(state)) if state == "success" => Ok(None),
            Ok(Some(state)) if state == "failure" || state == "error" => {
                Ok(Some("real check failed".to_owned()))
            }
            Ok(_) => Ok(Some("real check not recorded".to_owned())),
            Err(e) => {
                self.warn_once(
                    warn,
                    format!(
                        "{pr_named}'s {} status can't be read: {e:#}; it awaits you",
                        real_check::CONTEXT,
                        pr_named = self.pr_named(number)
                    ),
                )?;
                Ok(Some("real check status unreadable".to_owned()))
            }
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
            format!(
                "auto-merge could not be armed on {pr_named}: {e:#}; it awaits you",
                pr_named = self.pr_named(number)
            ),
        )?;
        writeln!(
            out,
            "{pr_named}: auto-merge not armed: arming failed",
            pr_named = self.pr_named(number)
        )?;
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
                        "{pr_named} conflicts with its base but is a draft; it awaits you, and \
                         the loop leaves it",
                        pr_named = self.pr_named(number)
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
                        "{pr_named} can't be read ({e:#}); it is not resolved until {branch} \
                         moves",
                        pr_named = self.pr_named(number)
                    ),
                );
            }
        };
        writeln!(
            out,
            "{pr_named} conflicts with {branch}; resolving it with `sbxm task fix --pr {number}`",
            pr_named = self.pr_named(number)
        )?;
        if armed {
            if let Err(e) = self.github.pr_disarm_auto_merge(&self.repo, number) {
                return self.warn_once(
                    warn,
                    format!(
                        "auto-merge could not be disarmed on {pr_named} ({e:#}); it is not \
                         resolved until {branch} moves",
                        pr_named = self.pr_named(number)
                    ),
                );
            }
            writeln!(
                out,
                "{pr_named}: auto-merge disarmed for the resolve",
                pr_named = self.pr_named(number)
            )?;
        }
        let result = self.steps.fix_pr(number, out, warn);
        if let Some(id) = record::fix_id(number, &head) {
            self.notify_question(&id, out, warn)?;
        }
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
                match self.rearm(number, task, "resolve", eligible, review, out, warn)? {
                    Some(note) => format!("resolved, {note}"),
                    None => "resolved".to_owned(),
                }
            }
        };
        let duration = |agent: Option<&record::Agent>| agent?.run.as_ref().map(|r| r.duration_s);
        let row = Row {
            issue: number,
            title: self.pr_row(
                number,
                format!("resolve: conflicts with {branch} (task {task})"),
            ),
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
        writeln!(
            out,
            "{pr_named}: {outcome}",
            pr_named = self.pr_named(number)
        )?;
        self.log_row(&row, false, warn)?;
        Ok(())
    }

    /// A failed resolve: said once, and the PR left to the user for the rest of the run.
    fn left_to_user(&mut self, number: u32, why: &str, out: &mut dyn Write) -> Result<String> {
        self.unresolved.insert(number);
        self.say_once(
            out,
            format!(
                "{pr_named} awaits you: {why}; its fix task is kept (see `sbxm task status`), \
                 and the loop leaves the PR",
                pr_named = self.pr_named(number)
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
                // A log-fix that isn't clean left the PR to the user, whatever came after it
                // (review M-2): it passes only finished and clean.
                Ok(rec) if dir.join(pr_fix::CHECK_LOG).is_file() => {
                    log_fix_left(dir, Some(&rec)).is_none() && passes_arm_checks(&rec)
                }
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

    /// Arms auto-merge again after a clean resolve (decision 298) or log-fix (decision 300; `what`
    /// names which in its lines): only for a PR that `eligible`
    /// (armed before, or its own task and every earlier resolve pushed onto it passed
    /// [`Self::arm`]'s checks), when the fix's review is
    /// low risk with 0 must-fix and covered the PR's new head. Returns the log row's note. `task`:
    /// the PR's own task, whose issue gives a medium PR's size (issue 549).
    #[allow(clippy::too_many_arguments)]
    fn rearm(
        &mut self,
        number: u32,
        task: &str,
        what: &str,
        eligible: bool,
        review: &record::ReviewResult,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<String>> {
        // Worked out first, so a refusal that comes before its own check still names it (review
        // S-1 of issues 545 and 547): said on a line of its own, and first in the log row's note.
        let own = self.record_of(task);
        let not_run = not_run_reason(review);
        let real = self.real_check_now(own.as_ref(), number, warn)?;
        let leading = leading_reasons(not_run.as_deref(), real.as_deref());
        let refused = |this: &Self, why: Option<&str>, out: &mut dyn Write| {
            let Some(leading) = leading.as_deref() else {
                return Ok(why.map(|w| not_armed(w, None)));
            };
            writeln!(
                out,
                "{pr_named} awaits you after its {what}: {}",
                not_armed(leading, None),
                pr_named = this.pr_named(number)
            )?;
            Ok::<_, anyhow::Error>(Some(not_armed(leading, why)))
        };
        // A medium PR's reasons go into every line below that leaves it to the user (review S-1
        // of issue 549), for its risk over its whole review history, this fix's included.
        let source = own.as_ref().map_or(medium::Source::Unknown, medium_source);
        let risk = self
            .history_risk(number, own.as_ref())
            .into_iter()
            .chain([review.risk])
            .max_by_key(|&risk| awaiting_rank(risk))
            .unwrap_or(review.risk);
        if !eligible {
            let why = with_reasons(
                "its own review didn't allow auto-merge".to_owned(),
                &self.medium_reasons(risk, source, number),
            );
            writeln!(
                out,
                "{pr_named} awaits you: {why}",
                pr_named = self.pr_named(number)
            )?;
            return refused(self, None, out);
        }
        if self.paused(warn)? {
            let reasons = self.medium_reasons(risk, source, number);
            let also: Vec<String> = if reasons.is_empty() {
                Vec::new()
            } else {
                std::iter::once(format!("risk {}", risk.word()))
                    .chain(reasons)
                    .collect()
            };
            self.not_armed_paused(number, &also, out)?;
            return refused(self, Some(PAUSED_REASON), out);
        }
        if review.risk != Level::Low {
            let why = with_reasons(
                format!("its {what}'s review is risk {}", review.risk.word()),
                &self.medium_reasons(risk, source, number),
            );
            writeln!(
                out,
                "{pr_named} awaits you: {why}",
                pr_named = self.pr_named(number)
            )?;
            return refused(self, Some(&format!("risk {}", review.risk.word())), out);
        }
        if not_run.is_some() {
            return refused(self, None, out);
        }
        let Some(reviewed) = review.commit.as_deref() else {
            writeln!(
                out,
                "{pr_named} awaits you: the commit its {what}'s review covered is unknown",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(with_reasons(
                "auto-merge not armed: reviewed commit unknown".to_owned(),
                &real.iter().cloned().collect::<Vec<_>>(),
            )));
        };
        let head = match self.github.pr(&self.repo, number) {
            Ok(pr) => pr.head_oid,
            Err(e) => return self.arming_failed(number, &e, out, warn),
        };
        if head != reviewed {
            let note = with_reasons(
                format!(
                    "auto-merge not armed: the PR's head isn't the commit the {what}'s review \
                     covered"
                ),
                &real.iter().cloned().collect::<Vec<_>>(),
            );
            writeln!(
                out,
                "{pr_named}: {note} (reviewed {reviewed}, head {head}); it awaits you",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(note));
        }
        if let Some(why) = self.real_check_refusal(own.as_ref(), number, &head, warn)? {
            let note = not_armed(&why, None);
            writeln!(
                out,
                "{pr_named} awaits you: {note}",
                pr_named = self.pr_named(number)
            )?;
            return Ok(Some(note));
        }
        match self.github.pr_arm_auto_merge(&self.repo, number, reviewed) {
            Ok(()) => {
                self.armed.insert(number);
                writeln!(
                    out,
                    "{pr_named}: auto-merge armed again on {reviewed}",
                    pr_named = self.pr_named(number)
                )?;
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
    /// `also`: what else keeps the PR from being armed, said after the pause (issue 549).
    fn not_armed_paused(
        &self,
        number: u32,
        also: &[String],
        out: &mut dyn Write,
    ) -> Result<String> {
        let why = with_reasons(
            "the loop is paused on a red main and arms nothing until you run `sbxm task loop \
             --clear-pause`"
                .to_owned(),
            also,
        );
        writeln!(out, "{} awaits you: {why}", self.pr_named(number))?;
        Ok(not_armed(PAUSED_REASON, None))
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
    /// failure are re-run once. `None` when the runs can't be read.
    fn read_runs(
        &mut self,
        commit: &str,
        what: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<RunsRead>> {
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
        let listed = !runs.is_empty();
        let mut over = listed;
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
        Ok(Some(RunsRead {
            failed,
            over,
            listed,
        }))
    }

    /// `main`'s side of the pass (decision 299, answer 4): the merge commits of the PRs this run
    /// armed that merged since, and `main`'s tip whoever merged it. A second failure sets the
    /// pause. Each such commit stays watched, after `main` moves on too, until every run on it is
    /// over and each second failure got its pause (review M-1 to M-3).
    fn check_main(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let mut armed: Vec<u32> = self.armed.iter().copied().collect();
        armed.sort_unstable();
        for number in armed {
            match self.github.pr_merge_commit(&self.repo, number) {
                Ok(Some(commit)) => {
                    self.armed.remove(&number);
                    self.watch(commit, Some(number));
                }
                Ok(None) => {}
                Err(e) => self.warn_once(
                    warn,
                    format!(
                        "{pr_named}'s merge can't be read ({e:#}); it is read again at the next \
                         pass",
                        pr_named = self.pr_named(number)
                    ),
                )?,
            }
        }
        match self.base_tip() {
            Ok((_, tip)) => self.watch(tip, None),
            Err(e) => self.warn_once(
                warn,
                format!(
                    "the base's head could not be read ({e:#}); its workflow runs are read at a \
                     later pass"
                ),
            )?,
        }
        let commits: Vec<(String, Option<u32>)> = self
            .watched
            .iter()
            .map(|w| (w.commit.clone(), w.pr))
            .collect();
        for (commit, pr) in commits {
            let what = match pr {
                Some(number) => format!(
                    "{pr_named}'s merge commit",
                    pr_named = self.pr_named(number)
                ),
                None => format!("main's commit {commit}"),
            };
            let Some(read) = self.read_runs(&commit, &what, out, warn)? else {
                continue;
            };
            for run in &read.failed {
                self.pause(&commit, pr, run, out, warn)?;
            }
            let acted = read
                .failed
                .iter()
                .all(|run| self.paused_for.contains(&(commit.clone(), run.id)));
            self.note_listing(&commit, read.listed, out)?;
            if read.over && acted {
                self.watched.retain(|w| w.commit != commit);
            }
        }
        Ok(())
    }

    /// Adds `commit` to the commits of `main` read at every pass, once; `pr`: the PR this run
    /// armed that merged into it.
    fn watch(&mut self, commit: String, pr: Option<u32>) {
        match self.watched.iter_mut().find(|w| w.commit == commit) {
            Some(watched) => watched.pr = watched.pr.or(pr),
            None => {
                let since = self.now();
                self.watched.push(Watched {
                    commit,
                    pr,
                    since,
                    runs_seen: false,
                    said_no_runs: false,
                });
            }
        }
    }

    /// Notes whether a read listed runs for the watched `commit`; one with none listed ever,
    /// [`NO_RUNS_AFTER`] after the loop first watched it, is said once and stays watched.
    fn note_listing(&mut self, commit: &str, listed: bool, out: &mut dyn Write) -> Result<()> {
        let now = self.now();
        let Some(watched) = self.watched.iter_mut().find(|w| w.commit == commit) else {
            return Ok(());
        };
        if listed {
            watched.runs_seen = true;
            return Ok(());
        }
        if watched.runs_seen
            || watched.said_no_runs
            || now.saturating_sub(watched.since) < NO_RUNS_AFTER
        {
            return Ok(());
        }
        watched.said_no_runs = true;
        writeln!(
            out,
            "no CI runs seen for {commit} after 1 h; still watching"
        )?;
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
        warn: &mut dyn Write,
    ) -> Result<()> {
        // Only the run a pause is for is marked (review M-3): another run's failure seen while a
        // pause stands gets its own pause once the user clears this one.
        if self.paused_for.contains(&(commit.to_owned(), run.id))
            || loop_pause::read(&self.base_dir).paused()
        {
            return Ok(());
        }
        let merged_by = match pr {
            Some(number) => format!(
                "the loop's auto-merge of {pr_named}",
                pr_named = self.pr_named(number)
            ),
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
        self.paused_for.insert((commit.to_owned(), run.id));
        self.say_once(out, paused_line(&pause))?;
        // (d) of issue 530: once per commit of `main`.
        let note = Notification {
            key: format!("main:{commit}"),
            title: "main is red: auto-merge paused".to_owned(),
            body: format!(
                "{} failed again after a re-run; next: fix main, then run `sbxm task loop \
                 --clear-pause`",
                pause.describe()
            ),
            click: if run.url.is_empty() {
                self.log_click()
            } else {
                run.url.clone()
            },
        };
        self.notify(note, out, warn)
    }

    /// The PRs' side of the pass (decision 299): every open PR the loop opened, drafts included.
    /// While paused, armed ones are disarmed. A run that failed again after its re-run disarms
    /// the PR if armed; the PR then awaits the user, said once with a log row.
    fn check_prs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let paused = self.paused(warn)?;
        let mut prs: Vec<(u32, String)> = queue::loop_prs(&self.base_dir, &self.repo)?
            .into_iter()
            .filter_map(|pr| Some((pr.number?, pr.task)))
            .collect();
        prs.sort();
        prs.dedup_by_key(|(number, _)| *number);
        for (number, task) in prs {
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
                            "{pr_named}: auto-merge disarmed while the loop is paused on a red \
                             main",
                            pr_named = self.pr_named(number)
                        )?;
                    }
                    Err(e) => self.warn_once(
                        warn,
                        format!(
                            "auto-merge could not be disarmed on {pr_named} while the loop is \
                             paused ({e:#}); tried again at the next pass",
                            pr_named = self.pr_named(number)
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
                            "{pr_named} can't be read ({e:#}); its workflow runs are read at a \
                             later pass",
                            pr_named = self.pr_named(number)
                        ),
                    )?;
                    continue;
                }
            };
            let what = self.pr_named(number);
            let Some(read) = self.read_runs(&head, &what, out, warn)? else {
                continue;
            };
            // Each second failure once (review M-3): one already said doesn't hide a newer one.
            let new: Vec<WorkflowRun> = read
                .failed
                .into_iter()
                .filter(|run| !self.failed_twice.contains(&(number, run.id)))
                .collect();
            let Some(run) = new.first().cloned() else {
                continue;
            };
            // One log-fix per PR, ever (decision 300): tried before the PR is left to the user.
            let log_fix = !self.log_fixed.contains(&number) && !self.has_log_fix(number);
            if armed {
                if let Err(e) = self.github.pr_disarm_auto_merge(&self.repo, number) {
                    self.warn_once(
                        warn,
                        format!(
                            "auto-merge could not be disarmed on {pr_named}, whose {} failed \
                             again after a re-run ({e:#}); tried again at the next pass",
                            run.workflow,
                            pr_named = self.pr_named(number)
                        ),
                    )?;
                    continue;
                }
                self.armed.remove(&number);
                if log_fix {
                    self.disarmed_for_log_fix.insert(number);
                }
            }
            // A low disk or a stop request leaves the log-fix for a later pass, never the disarm
            // (review M-1).
            if log_fix {
                let waits = if self.steps.room().is_err() {
                    Some("room on the disk")
                } else if (self.env.stop_requested)() {
                    Some("the next loop (this one is stopping)")
                } else {
                    None
                };
                if let Some(waits) = waits {
                    self.say_once(
                        out,
                        format!(
                            "{pr_named}: its {} failed twice ({}){}; its log-fix waits for {waits}",
                            run.workflow,
                            run.url,
                            if self.disarmed_for_log_fix.contains(&number) {
                                "; auto-merge disarmed"
                            } else {
                                ""
                            },
                            pr_named = self.pr_named(number)
                        ),
                    )?;
                    continue;
                }
            }
            if log_fix {
                let armed = armed || self.disarmed_for_log_fix.remove(&number);
                for run in &new {
                    self.failed_twice.insert((number, run.id));
                }
                self.log_fix(number, &task, armed, &head, &run, out, warn)?;
                continue;
            }
            for (i, run) in new.iter().enumerate() {
                // The disarm goes with the first new failure's line and row.
                let disarmed = armed && i == 0;
                self.failed_twice.insert((number, run.id));
                writeln!(
                    out,
                    "{pr_named} awaits you: its {} failed again after a re-run ({}){}",
                    run.workflow,
                    run.url,
                    if disarmed {
                        "; auto-merge disarmed"
                    } else {
                        ""
                    },
                    pr_named = self.pr_named(number)
                )?;
                let row = Row {
                    issue: number,
                    title: self.pr_row(
                        number,
                        format!("checks: {} failed again after a re-run", run.workflow),
                    ),
                    size: None,
                    started: self.now(),
                    ready: None,
                    worker_s: None,
                    reviewer_s: None,
                    rounds: None,
                    must_fix: None,
                    outcome: if disarmed {
                        "auto-merge disarmed; awaits you"
                    } else {
                        "awaits you"
                    }
                    .to_owned(),
                    stops: Vec::new(),
                    idle_before: None,
                };
                self.log_row(&row, false, warn)?;
            }
        }
        Ok(())
    }

    /// Whether PR `number` had its log-fix (decision 300): a fix task of it whose folder holds
    /// `check-log.md`, read from the task folders so a restarted loop sees it too. A folder that
    /// isn't sbxm's own doesn't count; one whose record can't be read does, so a PR is never tried
    /// twice.
    fn has_log_fix(&self, number: u32) -> bool {
        let Ok(folders) = record::task_folders(&self.base_dir) else {
            return true;
        };
        let prefix = format!("fix-{number}-");
        folders.iter().any(|dir| {
            let named = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|id| id.starts_with(&prefix));
            if !named
                || check_own_folder(&self.base_dir, dir).is_err()
                || !dir.join(pr_fix::CHECK_LOG).is_file()
            {
                return false;
            }
            match record::read(&dir.join("task.json")) {
                Ok(rec) => rec.kind == Kind::Fix && rec.repo == self.repo,
                Err(_) => true,
            }
        })
    }

    /// The one log-fix of PR `number` (decision 300), after its `run` of `head` failed again
    /// after a re-run (auto-merge already disarmed if `armed`): `task fix --pr N --check-run
    /// <id>`, then armed again on [`Self::resolve`]'s rules, read from the fix task's record. One
    /// that doesn't end clean (an error, a stop, must-fix left, a question handed back) leaves the
    /// PR to the user. One log row either way.
    #[allow(clippy::too_many_arguments)]
    fn log_fix(
        &mut self,
        number: u32,
        task: &str,
        armed: bool,
        head: &str,
        run: &WorkflowRun,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        self.log_fixed.insert(number);
        let started = self.now();
        writeln!(
            out,
            "{pr_named}: its {} failed twice ({}){}; fixing it once from the run's log with \
             `sbxm task fix --pr {number} --check-run {}`",
            run.workflow,
            run.url,
            if armed { "; auto-merge disarmed" } else { "" },
            run.id,
            pr_named = self.pr_named(number)
        )?;
        let result = self.steps.fix_check(number, run.id, out, warn);
        let id = record::fix_id(number, head);
        if let Some(id) = id.as_deref() {
            self.notify_question(id, out, warn)?;
        }
        let dir = id.as_deref().map(|id| record::task_dir(&self.base_dir, id));
        let fix = dir
            .as_ref()
            .and_then(|dir| record::read(&dir.join("task.json")).ok());
        let review = fix.as_ref().and_then(|r| r.review.clone());
        let clean = match (&result, &dir) {
            (Err(e), _) => Err(format!("failed: {}", first_line(&format!("{e:#}")))),
            (Ok(()), None) => Err("has no task folder".to_owned()),
            (Ok(()), Some(dir)) => match log_fix_left(dir, fix.as_ref()) {
                Some(why) => Err(why),
                None => review
                    .as_ref()
                    .ok_or_else(|| "has no review result".to_owned()),
            },
        };
        let outcome = match clean {
            Err(why) => {
                // Left to the user for the rest of the run: no resolve of it either (review M-2).
                self.unresolved.insert(number);
                writeln!(
                    out,
                    "{pr_named} awaits you: its {} failed again after a re-run ({}) and its \
                     log-fix {why}; its fix task is kept (see `sbxm task status`)",
                    run.workflow,
                    run.url,
                    pr_named = self.pr_named(number)
                )?;
                format!("log-fix {why}; awaits you")
            }
            Ok(review) => {
                let own = self
                    .record_of(task)
                    .is_some_and(|rec| passes_arm_checks(&rec));
                let current = id.clone().unwrap_or_default();
                let eligible = armed || (own && self.earlier_resolves_pass(number, &current));
                let outcome =
                    match self.rearm(number, task, "log-fix", eligible, review, out, warn)? {
                        Some(note) => format!("fixed from its log, {note}"),
                        None => "fixed from its log".to_owned(),
                    };
                writeln!(
                    out,
                    "{pr_named}: {outcome}",
                    pr_named = self.pr_named(number)
                )?;
                outcome
            }
        };
        let duration = |agent: Option<&record::Agent>| agent?.run.as_ref().map(|r| r.duration_s);
        let row = Row {
            issue: number,
            title: self.pr_row(
                number,
                format!(
                    "checks: {} failed again after a re-run; log-fix (task {task})",
                    run.workflow
                ),
            ),
            size: None,
            started,
            ready: None,
            worker_s: fix.as_ref().and_then(|r| duration(r.worker.as_ref())),
            reviewer_s: fix.as_ref().and_then(|r| duration(r.reviewer.as_ref())),
            rounds: review.as_ref().map(|r| r.round),
            must_fix: review.as_ref().map(|r| r.must_fix),
            outcome,
            stops: Vec::new(),
            idle_before: None,
        };
        self.log_row(&row, false, warn)?;
        Ok(())
    }

    fn append(&mut self, row: &Row, warn: &mut dyn Write) -> Result<()> {
        self.log_row(row, true, warn)
    }

    /// Every row of the loop's log is written here, so [`logged`](Self::logged) says whether this
    /// loop wrote one. `once`: a task's row, skipped when its issue has one already; a resolve's
    /// or a log-fix's row is written each time.
    fn log_row(&mut self, row: &Row, once: bool, warn: &mut dyn Write) -> Result<()> {
        let written = if once {
            queue::append_row(&self.log, row)
        } else {
            queue::append_resolve_row(&self.log, row).map(|()| true)
        };
        match written {
            Ok(written) => self.logged |= written,
            Err(e) => self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that log row"),
            )?,
        }
        Ok(())
    }
}

/// A commit of `main` the loop reads at every pass until it is done with it (decision 299).
struct Watched {
    commit: String,
    /// The PR this run armed that merged into it.
    pr: Option<u32>,
    /// When the loop first watched it, kept while it stays watched, after `main` moves on too.
    since: u64,
    /// Whether any read listed a run for it.
    runs_seen: bool,
    /// Whether [`NO_RUNS_AFTER`]'s line was said for it.
    said_no_runs: bool,
}

/// How long a watched commit may go without any run listed before the loop says so, once, and
/// goes on watching (the user's answer on issue 442, 2026-11-05).
const NO_RUNS_AFTER: u64 = 60 * 60;

/// What one read of a commit's runs found.
struct RunsRead {
    /// The runs that failed again after a re-run.
    failed: Vec<WorkflowRun>,
    /// At least one run listed, none going and none just re-run. A commit with no runs listed
    /// isn't over: GitHub lists them some time after the push (review M-2), so its watch goes on.
    over: bool,
    /// At least one run listed.
    listed: bool,
}

/// Whether a failed phase stopped on a lapsed sign-in (issue 445): `sbx`'s, or a harness's, said
/// in its error or, as a usage limit is, in a line of its output that reports a failure.
fn says_sign_in(e: &anyhow::Error, output: &str) -> bool {
    e.downcast_ref::<pipeline::NotSignedIn>().is_some()
        || e.downcast_ref::<pipeline::HarnessSignedOut>().is_some()
        || pipeline::says_harness_signed_out(&format!("{e:#}"))
        || output.lines().any(|line| {
            line.to_lowercase().contains("failed") && pipeline::says_harness_signed_out(line)
        })
}

/// The loop's last words on a sign-in stop (issue 445, answer 3): sign in, then `task resume` for
/// the stopped task when it exists (its label is gone, so a new loop wouldn't pick it up), then
/// `task loop` for the rest. A task refused before its record keeps its label.
fn sign_in_stop(e: anyhow::Error, resume: Option<String>) -> anyhow::Error {
    let mut then = Vec::new();
    if let Some(resume) = resume {
        then.push(resume);
    }
    then.push("sbxm task loop".to_owned());
    match e.downcast_ref::<pipeline::NotSignedIn>() {
        Some(stop) => anyhow!("the loop stops: {}", stop.then(then)),
        None => anyhow!(
            "the loop stops: {e:#}; once signed in again, run `{}`",
            then.join("`, then `")
        ),
    }
}

/// A stop of the loop that needs the user (decision 310): a lapsed sign-in, a missing secret, two
/// parked tasks in a row or the usage waits' total. `task loop` exits [`NEEDS_USER`] for it, so
/// the scheduled start doesn't restart it. It reads as the error it wraps.
#[derive(Debug)]
pub struct NeedsUser(Cause, anyhow::Error);

/// What a [`NeedsUser`] stop is for, named in its toast's title (issue 530).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    SignIn,
    Secret,
    Parked,
    UsageLimit,
}

impl Cause {
    /// The key's word for it.
    fn key(self) -> &'static str {
        match self {
            Self::SignIn => "sign-in",
            Self::Secret => "secret",
            Self::Parked => "parked",
            Self::UsageLimit => "usage-limit",
        }
    }

    /// The toast's title.
    fn title(self) -> &'static str {
        match self {
            Self::SignIn => "Loop stopped: sbx sign-in lapsed",
            Self::Secret => "Loop stopped: a secret is missing",
            Self::Parked => "Loop stopped: two tasks parked in a row",
            Self::UsageLimit => "Loop stopped: the usage limit held",
        }
    }
}

impl std::fmt::Display for NeedsUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.1, f)
    }
}

impl std::error::Error for NeedsUser {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.1.source()
    }
}

/// The exit code of a loop that stopped on [`NeedsUser`] (decision 310).
pub const NEEDS_USER: i32 = 3;

/// `e` marked as a stop that needs the user, for `cause`.
fn needs_user(cause: Cause, e: anyhow::Error) -> anyhow::Error {
    if e.is::<NeedsUser>() {
        e
    } else {
        anyhow::Error::new(NeedsUser(cause, e))
    }
}

/// The exit code of `task loop` for its result (decision 310): 0 when it stopped as asked or on an
/// empty queue, [`NEEDS_USER`] for a stop that needs the user, 1 for any other error.
pub fn exit_code(result: &Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) if e.is::<NeedsUser>() => NEEDS_USER,
        Err(_) => 1,
    }
}

/// `why`, then each of `reasons`, separated by `; `.
fn with_reasons(why: String, reasons: &[String]) -> String {
    std::iter::once(why)
        .chain(reasons.iter().cloned())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The refusals that every "not armed" note of a PR starts with, worked out before the other
/// checks (review S-1 of issues 545 and 547): its checks not run, then its real check; `None`
/// with neither.
fn leading_reasons(not_run: Option<&str>, real: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = not_run.into_iter().chain(real).collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The real check the issue of a PR's own task `own` names (issue 547), from the issue text its
/// start saved (`issue.md`), as `task finish` reads `**Closes on merge:**`: `None` for a task of
/// another kind or without the file. `Err` with the refusal when it can't be known: no readable
/// task, or a file that can't be read.
fn real_check_of(
    base_dir: &Path,
    own: Option<&Record>,
) -> std::result::Result<Option<String>, String> {
    let Some(own) = own else {
        return Err("real check unknown: the PR's task can't be read".to_owned());
    };
    if own.kind != Kind::Issue {
        return Ok(None);
    }
    match fs::read_to_string(record::task_dir(base_dir, &own.id).join("issue.md")) {
        Ok(text) => Ok(real_check::required(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("real check unknown: its issue text can't be read".to_owned()),
    }
}

/// Where the task `rec` came from, for the size [`medium::refusals`] reads.
fn medium_source(rec: &Record) -> medium::Source {
    match rec.kind {
        Kind::Issue => medium::Source::Issue(rec.number),
        Kind::Spec => medium::Source::Spec,
        Kind::Pr | Kind::Fix => medium::Source::Unknown,
    }
}

/// [`Loop::arm`]'s checks on a task's record: no work stop, a review with 0 must-fix, low risk
/// after the path-rule floor and no check left not run.
fn passes_arm_checks(rec: &Record) -> bool {
    rec.stopped.is_none()
        && rec
            .review
            .as_ref()
            .is_some_and(|r| r.must_fix == 0 && r.risk == Level::Low && not_run_reason(r).is_none())
}

/// Why `review`'s `## Not run` list keeps auto-merge unarmed (issue 545), at any risk: a check
/// neither the reviewer nor CI ran, or a list that is unknown (missing, empty, or a record from
/// before the list). `None` for the empty list, `- none`.
fn not_run_reason(review: &record::ReviewResult) -> Option<String> {
    match review.not_run.as_deref() {
        None => Some("not-run list unknown".to_owned()),
        Some([]) => None,
        Some([_]) => Some("1 check not run".to_owned()),
        Some(checks) => Some(format!("{} checks not run", checks.len())),
    }
}

/// The log row's note for a PR not armed for `reason`, then `also` when another refusal holds too
/// (review S-1 of issue 545): `auto-merge not armed: 1 check not run; risk medium`.
fn not_armed(reason: &str, also: Option<&str>) -> String {
    match also {
        Some(also) => format!("auto-merge not armed: {reason}; {also}"),
        None => format!("auto-merge not armed: {reason}"),
    }
}

/// Why a PR isn't armed while the loop is paused on a red `main` (decision 299).
const PAUSED_REASON: &str = "paused on a red main";

/// How far `risk` keeps a PR from the loop's arming, for the highest of its review history: an
/// unknown risk fails [`passes_arm_checks`] as medium does, so it outranks low (where
/// [`Level`]'s own order puts it below).
fn awaiting_rank(risk: Level) -> u8 {
    match risk {
        Level::Low => 0,
        Level::Unknown => 1,
        Level::Medium => 2,
        Level::High => 3,
    }
}

/// Why the log-fix task in `dir`, recorded as `rec`, didn't end clean (decision 300), in words
/// that follow "its log-fix"; `None` when it did: finished, no stop, a review with 0 must-fix and
/// no question handed back in its `result.md`. The one rule for the log-fix's own outcome and for
/// every later re-arm that looks back at it (review M-2).
fn log_fix_left(dir: &Path, rec: Option<&Record>) -> Option<String> {
    let Some(rec) = rec else {
        return Some("has no readable record".to_owned());
    };
    if rec.stopped.is_some() {
        return Some("stopped before its review was clean".to_owned());
    }
    let Some(review) = &rec.review else {
        return Some("has no review result".to_owned());
    };
    if review.must_fix > 0 {
        return Some(format!("left {} must-fix", review.must_fix));
    }
    let handed_back =
        fs::read_to_string(dir.join("result.md")).is_ok_and(|text| finish::hands_back(&text));
    if handed_back {
        return Some("handed a question back".to_owned());
    }
    if rec.stage != Stage::Finished {
        return Some(format!("stopped at {}", rec.stage.name()));
    }
    None
}

/// The line saying the loop is paused on a red `main`.
fn paused_line(pause: &Pause) -> String {
    format!(
        "paused on a red main: {}; the loop arms nothing until you run `sbxm task loop \
         --clear-pause`",
        pause.describe()
    )
}

/// Why an issue waiting on [`unmerged_dependencies`] is passed over.
fn waiting_reason(unmerged: &[String]) -> String {
    format!("depends on {}, not merged", unmerged.join(", "))
}

/// Why an issue `task run` would refuse before any write is passed over.
fn refused_reason(reason: &str) -> String {
    format!("refused before any write: {reason}")
}

/// The `Depends on:` numbers that are an open issue or a PR not merged. A number that is
/// neither an open issue nor a readable PR is taken for a closed issue.
fn unmerged_dependencies(
    github: &dyn GitHubBackend,
    repo: &str,
    open: &[Issue],
    body: &str,
) -> Vec<String> {
    queue::depends_on(body)
        .into_iter()
        .filter_map(|n| {
            if open.iter().any(|i| i.number == n) {
                return Some(format!("#{n} (an open issue)"));
            }
            match github.pr(repo, n) {
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

/// `<base>/.sbxm/task-loop.stop`, next to the loop lock: a stop request `task loop --stop` writes
/// for the running loop (issue 527, decision 310), which treats it as a first Ctrl-C press and
/// removes it when it stops. One left from before, when no loop ran, is removed at the next start.
pub fn stop_path(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("task-loop.stop")
}

/// `sbxm task loop --stop` (issue 527, decision 310): asks the running loop to stop, by writing
/// [`stop_path`]. With no loop running (no lock, or one whose process is gone) it says so and
/// writes nothing.
pub fn stop(config_dir: &Path, probe: &dyn ProcessProbe, out: &mut dyn Write) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let lock = queue::lock_path(&base_dir);
    let text = match fs::read_to_string(&lock) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            writeln!(out, "no `sbxm task loop` is running; nothing to stop")?;
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", lock.display())),
    };
    let held: Process = serde_json::from_str(&text).with_context(|| {
        format!(
            "cannot read the loop lock {}; if no `sbxm task loop` is running, delete it",
            lock.display()
        )
    })?;
    if !held.alive(probe) {
        writeln!(
            out,
            "no `sbxm task loop` is running (its lock names process {}, which is gone); nothing \
             to stop",
            held.pid
        )?;
        return Ok(());
    }
    let path = stop_path(&base_dir);
    fs::write(&path, "").with_context(|| format!("cannot write {}", path.display()))?;
    writeln!(
        out,
        "asked the `sbxm task loop` of process {} to stop: it stops after its current task, or \
         within {} while it waits or is paused",
        held.pid,
        queue::span(PAUSE_POLL.as_secs())
    )?;
    Ok(())
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

/// Runs the loop (issue 320) with no notifications. Everything is checked before the first
/// write: the config, the repo, the task folders, then the lock is taken.
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
    run_loop(config_dir, opts, steps, github, probe, env, None, out, warn)
}

/// [`run`], showing the user what needs them through `notifier` (issue 530, decision 311) unless
/// `config.toml` sets `[notify] toasts = false`. Only `main` passes the real one.
#[allow(clippy::too_many_arguments)]
pub fn run_notifying(
    config_dir: &Path,
    opts: &Options,
    steps: &dyn Steps,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    notifier: &dyn Notifier,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    run_loop(
        config_dir,
        opts,
        steps,
        github,
        probe,
        env,
        Some(notifier),
        out,
        warn,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    config_dir: &Path,
    opts: &Options,
    steps: &dyn Steps,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    env: &Env,
    notifier: Option<&dyn Notifier>,
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
    if opts.wait && !opts.issues.is_empty() {
        bail!("--wait can't be used with --issue: a list of issues has an end");
    }
    let settings = TaskConfig::load(&opts.repo_root)?.task_loop;
    let global = GlobalConfig::load(config_dir)?;
    let notifier = notifier.filter(|_| global.notify.toasts);
    let base_dir = global.base_dir;
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    // As `task rm --merged` refuses it (decision 195): every task folder is read through it.
    let tasks = record::tasks_root(&base_dir);
    if fs::symlink_metadata(&tasks).is_ok() {
        check_own_folder(&base_dir, &tasks)?;
    }
    // The job queue's folders too (issue 525): every job file is read through them.
    crate::task::jobs::check_queue(&base_dir)?;
    let (mut lock, dead) = take_lock(&base_dir, opts, probe, warn)?;
    // Under the lock, so no loop is running that a request left here could be for.
    let stop_file = stop_path(&base_dir);
    match fs::remove_file(&stop_file) {
        Ok(()) => writeln!(
            out,
            "removed a stop request left from before, when no loop was running ({}); not obeyed",
            stop_file.display()
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot remove {}", stop_file.display())),
    }
    // `task loop --stop`'s request counts as a first Ctrl-C press.
    let asked = || (env.stop_requested)() || fs::symlink_metadata(&stop_file).is_ok();
    let env = &Env {
        now: env.now,
        sleep: env.sleep,
        stop_requested: &asked,
    };

    let (started_at, log) = free_log(&base_dir, env)?;
    let notify = match notifier {
        Some(notifier) => Some((notifier, Shown::load(&base_dir, warn)?)),
        None => None,
    };
    let mut lp = Loop {
        config_dir,
        repo_root: opts.repo_root.clone(),
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
        watched: Vec::new(),
        reruns: HashSet::new(),
        failed_twice: HashSet::new(),
        log_fixed: HashSet::new(),
        disarmed_for_log_fix: HashSet::new(),
        paused_for: HashSet::new(),
        log,
        logged: false,
        notify,
    };

    // The cut-off tasks first: their owed resumes are written before anything else can fail.
    let result = lp.cut_off(dead.as_ref(), out, warn).and_then(|due| {
        lp.interrupted_jobs(out, warn)?;
        work(&mut lp, &mut lock, opts, due, out, warn)
    });
    // What waits for the user as the loop stops (issue 530): a PR it left, the stop itself.
    if let Err(e) = lp.notify_awaiting(out, warn) {
        let _ = writeln!(
            warn,
            "warning: the PRs awaiting you were not checked for notifications: {e:#}"
        );
    }
    if let Err(e) = &result
        && let Some(NeedsUser(cause, _)) = e.downcast_ref::<NeedsUser>()
    {
        let note = Notification {
            key: format!("stop:{started_at}:{}", cause.key()),
            title: cause.title().to_owned(),
            body: first_line(&format!("{e:#}")),
            click: lp.log_click(),
        };
        let _ = lp.notify(note, out, warn);
    }
    // Only rows this loop wrote are posted: a loop that ran no task posts nothing (decision 305).
    if lp.logged {
        let stop = match &result {
            Ok(stop) => stop.clone(),
            Err(e) => first_line(&format!("{e:#}")),
        };
        if let Err(e) = loop_log::post(github, &lp.repo, &lp.log, started_at, &stop) {
            // Never in place of the loop's own result: its exit code stays as it was.
            let _ = writeln!(
                warn,
                "could not post the loop's log to GitHub: {e:#}; it stays at {}",
                lp.log.display()
            );
        }
    }
    // The request is the loop's to remove once it stops (decision 310), whatever stopped it.
    if let Err(e) = fs::remove_file(&stop_file)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        let _ = writeln!(
            warn,
            "warning: cannot remove the stop request {}: {e}; the next loop removes it at its \
             start",
            stop_file.display()
        );
    }
    result.map(drop)
}

/// How many seconds the loop waits at most for a log name no earlier loop took.
const LOG_NAME_TRIES: u32 = 5;

/// The loop's start and its own log file (decision 305): a name an earlier loop that started in
/// the same second took is never shared, the loop waits for the next second instead (review M-2
/// of issue 463). Called under the loop lock, so no other loop takes the name meanwhile.
fn free_log(base_dir: &Path, env: &Env) -> Result<(u64, PathBuf)> {
    for _ in 0..LOG_NAME_TRIES {
        let started_at = (env.now)();
        let log = loop_log::path(base_dir, started_at);
        if fs::symlink_metadata(&log).is_err() {
            return Ok((started_at, log));
        }
        (env.sleep)(Duration::from_secs(1));
    }
    bail!(
        "every log name of the last {LOG_NAME_TRIES} seconds under {} is taken by an earlier \
         loop; try again",
        base_dir.join(".sbxm").join("loop-logs").display()
    )
}

/// The loop itself, from its first task to its stop; returns the stop's message.
/// `due`: the tasks a dead loop was running, resumed first (decision 310).
fn work(
    lp: &mut Loop,
    lock: &mut LoopLock,
    opts: &Options,
    mut due: VecDeque<Held>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<String> {
    let (github, env) = (lp.github, lp.env);
    let mut tried = HashSet::new();
    let mut refused = Vec::new();
    let mut tasks_run = 0u32;
    let mut parked_in_row = 0;
    let mut idle_from: Option<u64> = None;
    // Whether a `--wait` loop has said it waits on an empty queue since its last task.
    let mut idle = false;
    // Tasks whose lock a command run by hand held (issue 409): `held` waits for the loop's next
    // task, then moves to `due`, tried before the next new issue. A task held again goes back to
    // `held`, so the loop never tries one twice with nothing run in between.
    let mut held: Vec<Held> = Vec::new();
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
            return Ok("stopped on request".to_owned());
        }
        if opts.cap.is_some_and(|cap| tasks_run >= cap) {
            let stop = format!("--cap {} reached", opts.cap.unwrap_or_default());
            writeln!(out, "{stop}")?;
            left(&held, &due, out)?;
            return Ok(stop);
        }
        lp.clean_merged(out, warn)?;
        // The queued jobs run after the resolves and the checks of the runs, before the pause
        // for PRs awaiting the user, so a queued review or fix isn't held up by it (issue 525).
        let go_on = lp.resolve_conflicts(out, warn)? && {
            lp.check_runs(out, warn)?;
            lp.drain_jobs(out, warn)? && {
                lp.notify_awaiting(out, warn)?;
                lp.wait_for_prs(out, warn)?
            }
        };
        if !go_on {
            writeln!(out, "stopped on request")?;
            left(&held, &due, out)?;
            return Ok("stopped on request".to_owned());
        }
        let (issue, title, size, started, stops, ended) = if let Some(again) = due.pop_front() {
            let Held {
                issue,
                title,
                size,
                started,
                mut stops,
                retry,
                cut_off,
            } = again;
            // Taken only now: a loop that stops before this leaves the resume owed, for the next.
            if cut_off && let Err(e) = lp.mark_resume(issue, serde_json::Value::Bool(true)) {
                let Some(in_use) = e.downcast_ref::<lock::InUse>() else {
                    return Err(e);
                };
                writeln!(
                    out,
                    "{}: its resume is skipped: {}; the loop leaves it to that command",
                    crate::title::named(&format!("#{issue}"), &title),
                    in_use.describe()
                )?;
                continue;
            }
            writeln!(
                out,
                "task {}: #{issue} {title} (size {}), tried again",
                tasks_run + 1,
                size.as_deref().unwrap_or("unknown")
            )?;
            note(lock, lp, warn, |p| p.start(issue))?;
            let ended = lp.again(issue, retry, &mut stops, out, warn);
            (issue, title, size, started, stops, ended)
        } else {
            let open = github.issues_open(&lp.repo)?;
            let (next, waiting) = lp.next(&open, &opts.issues, &mut tried, &mut refused, out)?;
            let Some(issue) = next else {
                if opts.wait {
                    // Said once per idle spell: the next task, or the next stop, ends it.
                    if !idle {
                        writeln!(
                            out,
                            "the queue is empty; checking again every {} for a job file or a \
                             labelled issue",
                            queue::span(PAUSE_POLL.as_secs())
                        )?;
                        idle = true;
                    }
                    (env.sleep)(PAUSE_POLL);
                    continue;
                }
                let skipped: Vec<Skipped> = refused.iter().chain(&waiting).cloned().collect();
                let stop = if opts.issues.is_empty() {
                    "the queue is empty; the loop stops"
                } else {
                    "every named issue was taken or skipped; the loop stops"
                };
                writeln!(out, "{stop}")?;
                if opts.issues.is_empty() {
                    write!(
                        out,
                        "{}",
                        queue::report(&Report {
                            open: &open,
                            skipped: &skipped,
                        })
                    )?;
                }
                left(&held, &due, out)?;
                return Ok(stop.to_owned());
            };
            idle = false;
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
            note(lock, lp, warn, |p| {
                for skipped in refused.iter().chain(&waiting) {
                    p.set(skipped.issue, Outcome::Skipped);
                }
                p.start(issue);
            })?;
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
                return Ok("stopped on request".to_owned());
            }
            Err(Halt::SignIn(e)) => {
                let rec = lp.record(issue);
                let stage = rec
                    .as_ref()
                    .map_or("no record".to_owned(), |r| r.stage.name().to_owned());
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
                        outcome: format!("stopped at {stage}: sign-in"),
                        stops,
                        idle_before: idle_from.map(|end| started.saturating_sub(end)),
                    },
                    warn,
                )?;
                left(&held, &due, out)?;
                return Err(needs_user(
                    Cause::SignIn,
                    sign_in_stop(
                        e,
                        rec.is_some()
                            .then(|| format!("sbxm task resume --issue {issue}")),
                    ),
                ));
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
                return Err(needs_user(
                    Cause::UsageLimit,
                    anyhow!(
                        "the usage limit held through {} of waits in a row; the loop stops; continue \
                     with `sbxm task resume --issue {issue}` once it resets",
                        queue::span(total.as_secs())
                    ),
                ));
            }
        };
        let recorded = match &end {
            End::Done { .. } => Outcome::Done,
            End::Parked { .. } => Outcome::Parked,
            End::Refused(_) | End::Taken(_) => Outcome::Skipped,
            End::Held(_) => Outcome::Held,
        };
        note(lock, lp, warn, |p| {
            p.current = None;
            p.set(issue, recorded);
            if matches!(recorded, Outcome::Done | Outcome::Parked) {
                p.counted += 1;
            }
        })?;
        let (ready, outcome, parked) = match end {
            End::Refused(reason) => {
                // The line before named the issue with its title already.
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
                    cut_off: false,
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
        writeln!(out, "{}: {outcome}", lp.issue_named(issue))?;
        lp.append(&row, warn)?;
        // Idle runs from a task's ready to the next start (decision 249, review S-1), so the
        // loop's own finish and findings filing count, a failed finish too; a task that never got
        // ready, from its end.
        idle_from = Some(ready.unwrap_or_else(|| lp.now()));
        if parked {
            parked_in_row += 1;
            if parked_in_row >= 2 {
                return Err(needs_user(
                    Cause::Parked,
                    anyhow!(
                        "two tasks parked in a row; the loop stops; see `sbxm task status` for what \
                     each left"
                    ),
                ));
            }
        } else {
            parked_in_row = 0;
        }
    }
}

/// Records the loop's progress in its lock (issue 493). A lock that can't be written is a warning,
/// not a stop: it only keeps `sbxm status` up to date.
fn note(
    lock: &mut LoopLock,
    lp: &mut Loop,
    warn: &mut dyn Write,
    change: impl FnOnce(&mut queue::Progress),
) -> Result<()> {
    if let Err(e) = lock.record(change) {
        lp.warn_once(
            warn,
            format!(
                "the loop's progress could not be written into its lock ({e:#}); `sbxm status` \
                 may show it behind"
            ),
        )?;
    }
    Ok(())
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

    /// `task fix --pr N`, from the review or with `check_run` a failed run's log, teed into the
    /// fix task's `run.log`. Its id names the PR's head, so the log is named before `task fix`
    /// runs.
    fn task_fix(
        &self,
        pr: u32,
        check_run: Option<u64>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
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
                    check_run,
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

    /// Gives up this process's own claim on `issue-<n>`'s record before `task resume` (review
    /// M-1): the phase that left it ran here and has returned, so the loop being alive doesn't
    /// mean the task is running. A task another process owns keeps its owner, and `task resume`
    /// refuses it while that process lives. The task's lock (issue 58) is held around the read
    /// and the write and given up before `task resume` takes its own (issue 409); a live holder
    /// refuses with [`lock::InUse`], writing nothing.
    fn release(&self, id: &str) -> Result<()> {
        let base_dir = GlobalConfig::load(self.config_dir)?.base_dir;
        let _lock = pipeline::lock_task(&base_dir, id, self.probe)?;
        let dir = record::task_dir(&base_dir, id);
        let Ok(mut rec) = record::read(&dir.join("task.json")) else {
            return Ok(());
        };
        if rec.process.as_ref() == Some(&Process::current(self.probe)) {
            record::write_change(&dir, &mut rec, |r| r.process = None)?;
        }
        Ok(())
    }

    /// `task resume --issue N`, or with `should_fix` its should-fix round, `--rounds 1
    /// --should-fix` (issue 510).
    fn resume_with(
        &self,
        issue: u32,
        should_fix: bool,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        self.release(&format!("issue-{issue}"))?;
        self.logged(issue, out, warn, &|out, warn| {
            task_resume::run(
                self.config_dir,
                &task_resume::Options {
                    repo_root: self.repo_root.clone(),
                    target: task_resume::Target::Issue(issue),
                    rounds: should_fix.then_some(1),
                    should_fix,
                    disk: self.disk.clone(),
                    identity: None,
                    forget_ambiguous: false,
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
        self.resume_with(issue, false, out, warn)
    }

    fn should_fix(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.resume_with(issue, true, out, warn)
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
        self.task_fix(pr, None, out, warn)
    }

    fn fix_check(
        &self,
        pr: u32,
        run: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        self.task_fix(pr, Some(run), out, warn)
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

    fn review_pr(&self, pr: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.logged_as(format!("pr-{pr}"), out, warn, &|out, warn| {
            task_review::run(
                self.config_dir,
                &task_review::Options {
                    repo_root: self.repo_root.clone(),
                    target: task_review::Target::Pr(pr),
                    repo: self.repo.clone(),
                    base: None,
                    clone_source: None,
                    reviewer_harness: None,
                    reviewer_model: None,
                    reviewer_time_limit: None,
                    time_limit: None,
                    profile: None,
                    resolved_spec_id: None,
                    force: false,
                    disk: self.disk.clone(),
                    location: None,
                    identity: None,
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

    fn run_spec(&self, spec: &Path, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let id = record::spec_id(spec)?.0;
        self.logged_as(id, out, warn, &|out, warn| {
            task_run::run_spec(
                self.config_dir,
                &task_run::SpecOptions {
                    repo_root: self.repo_root.clone(),
                    spec: spec.to_owned(),
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

    fn should_fix_spec(
        &self,
        spec: &Path,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let id = record::spec_id(spec)?.0;
        self.release(&id)?;
        self.logged_as(id, out, warn, &|out, warn| {
            task_resume::run(
                self.config_dir,
                &task_resume::Options {
                    repo_root: self.repo_root.clone(),
                    target: task_resume::Target::Spec(spec.to_owned()),
                    rounds: Some(1),
                    should_fix: true,
                    disk: self.disk.clone(),
                    identity: None,
                    forget_ambiguous: false,
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

    fn finish_spec(&self, spec: &Path, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let id = record::spec_id(spec)?.0;
        self.logged_as(id, out, warn, &|out, _warn| {
            task_finish::run(
                self.config_dir,
                &task_finish::Options {
                    target: task_finish::Target::Spec(spec.to_owned()),
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

    fn template_build(
        &self,
        profile: &str,
        harness: Harness,
        warm: Option<&Path>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<String> {
        template_build::build_tag(
            self.config_dir,
            &template_build::Options {
                profile: profile.to_owned(),
                harness,
                disk: self.disk.clone(),
                now: None,
                warm: warm.map(Path::to_path_buf),
            },
            self.backend,
            self.probe,
            out,
            warn,
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
