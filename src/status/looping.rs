//! The Loop section of `sbxm status` (decision 294, issue 376): whether a `task loop` runs, the
//! task it is on and its stage, the tasks it finished, the PRs awaiting the user (the loop's cap
//! of 5, counted by the same [`queue::awaits_user`] the loop pauses on) and the queue of labeled
//! issues in the order the loop takes them (for a loop started with `--issue`, its list from the
//! lock instead, as [`Listed`], issue 477), the pause on a red `main` (issue 442) and the
//! loop's job files, queued and running ([`Jobs`], issue 528). State
//! comes from the loop's lock, its pause file, its job files and the task records alone, with no write; only the queue and the PR count need GitHub, one
//! fetch at most once a minute ([`Cache`]), made only while a loop runs.

use std::fs;
use std::path::Path;

use crate::github::GitHubBackend;
use crate::run::results::{format_timestamp, parse_timestamp};
use crate::status::tasks::truncate;
use crate::task::finish::check_own_folder;
use crate::task::jobs::{self, Job, Running};
use crate::task::loop_pause;
use crate::task::merged;
use crate::task::queue::{self, LOOP_HOOK, LockFile, Outcome, Progress};
use crate::task::record::{self, Kind, Process, ProcessProbe, Record, Stage, Status};

/// The longest queue shown on a terminal; the rest is "… N more". Output that isn't a terminal
/// shows every issue.
pub const MAX_QUEUE_LINES: usize = 10;
/// How long a GitHub snapshot is reused, so `--watch` calls GitHub at most once a minute.
pub const REFRESH_SECS: u64 = 60;

/// What the loop's lock says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lock {
    /// No lock file.
    Absent,
    /// Held by a process that is alive.
    Running(Process),
    /// Left by a process that is gone (the same pid and start-time check as `run.log`, decision
    /// 163).
    Stale(Process),
    /// A lock file that can't be read; the first line of why.
    Unreadable(String),
}

/// Reads `<base>/.sbxm/task-loop.lock`.
pub fn read_lock(base: &Path, probe: &dyn ProcessProbe) -> Lock {
    read_lock_file(base, probe).0
}

/// [`read_lock`], with the whole lock file it read when that parsed: for a loop started with
/// `--issue`, its list and its `--cap` too (issue 477). One read, so the list is the process's.
pub fn read_lock_file(base: &Path, probe: &dyn ProcessProbe) -> (Lock, Option<LockFile>) {
    let path = queue::lock_path(base);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Lock::Absent, None),
        Err(e) => {
            return (
                Lock::Unreadable(format!("cannot read {}: {e}", path.display())),
                None,
            );
        }
    };
    match serde_json::from_str::<LockFile>(&text) {
        Ok(file) if file.process.alive(probe) => (Lock::Running(file.process.clone()), Some(file)),
        Ok(file) => (Lock::Stale(file.process.clone()), Some(file)),
        Err(e) => (
            Lock::Unreadable(format!("cannot parse {}: {e}", path.display())),
            None,
        ),
    }
}

/// The task a running loop is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Current {
    pub id: String,
    pub stage: &'static str,
}

/// What the task records say about a running loop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    pub current: Option<Current>,
    /// The tasks the loop finished since it started, by id.
    pub done: Vec<String>,
    /// The title of the current task and of each one done, by id (issue 246).
    pub titles: std::collections::BTreeMap<String, String>,
    /// For a loop started with `--issue` (issue 477): where it is in its list. `None` for a loop
    /// over the labeled queue, or one whose lock an older binary wrote.
    pub listed: Option<Listed>,
}

/// Where a loop started with `--issue` is in its list (issue 477), each issue by number in the
/// list's order. A lock with the loop's recorded [`Progress`] (issue 493) says what happened to
/// each, and how many tasks `--cap` counted; see [`recorded`]. For a lock without it (an older
/// binary's), the list alone, the task records say what happened to each:
/// the issue running, the ones finished (as [`Work::done`]) and the ones parked (a task left
/// short of finished by the loop's process). The furthest of those in the list is how far the
/// loop got: an issue before it with none of these was skipped (refused, waiting on a
/// dependency, or taken by a command run by hand), never pending; the issues after it are
/// pending, as many as `--cap` leaves to run, and the rest are not run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub current: Option<u32>,
    pub done: Vec<u32>,
    pub parked: Vec<u32>,
    pub skipped: Vec<u32>,
    /// Issues whose task's lock a command run by hand held, tried again after the running task
    /// (issue 493), as many as `--cap` leaves to run; only from a recorded [`Progress`].
    pub held: Vec<u32>,
    pub pending: Vec<u32>,
    /// The issues past `--cap`.
    pub not_run: Vec<u32>,
    pub cap: Option<u32>,
    /// The tasks counted toward `--cap`, as the loop recorded it (issue 493).
    pub counted: Option<u32>,
    /// Each listed issue's title from its task record, where it has one (issue 246).
    pub titles: std::collections::BTreeMap<u32, String>,
}

impl Listed {
    /// The tasks the loop ran from the list so far, the running one not counted.
    fn ran(&self) -> usize {
        self.counted
            .map_or(self.done.len() + self.parked.len(), |n| n as usize)
    }

    /// M of "task N of M": the tasks the loop runs from the list, those it ran, the one running,
    /// the held ones and the ones pending; skipped and not-run issues are not counted.
    pub fn total(&self) -> usize {
        self.ran() + usize::from(self.current.is_some()) + self.held.len() + self.pending.len()
    }

    /// N of "task N of M", while a task runs.
    pub fn position(&self) -> Option<usize> {
        self.current.map(|_| self.ran() + 1)
    }
}

/// The loop's task and the tasks it finished, from the records under `base` (never `sbx`).
/// The task is the issue task still `running` that the lock's process owns: the loop recorded
/// in it by the loop's child that runs it, else its recorded process (decision 322, amending
/// 294(1)); the loop's marker (`hooks.task_loop`) is written only when it claims a ready task
/// for `task finish`, so it can't identify work in progress (decision 294, approved by the user
/// 2026-11-06). The tasks done are the marked records finished since the lock's process
/// started, the archived ones included: the loop's own merged-task cleanup archives a task and
/// removes its folder once its PR merges (decision 195), and that task is still done in this
/// window (review M-1 of PR 437). A task in both places, between the archive and the removal,
/// counts once; only the active records say which task runs.
pub fn work(base: &Path, lock: &Process) -> anyhow::Result<Work> {
    work_for(
        base,
        &LockFile {
            process: lock.clone(),
            issues: Vec::new(),
            cap: None,
            progress: None,
        },
    )
}

/// [`work`] for the whole lock file: with [`Work::listed`] for a loop started with `--issue`.
pub fn work_for(base: &Path, file: &LockFile) -> anyhow::Result<Work> {
    let lock = &file.process;
    let started = parse_timestamp(&lock.started_at);
    let mut out = Work::default();
    let active = record::load_all(base)?;
    let mut finished = Vec::new();
    for rec in &active {
        if rec.kind != Kind::Issue {
            continue;
        }
        if rec.stage == Stage::Finished {
            finished.push(rec);
        } else if rec.status == Status::Running && rec.owner() == Some(lock) {
            out.current = Some(Current {
                id: rec.id.clone(),
                stage: rec.stage.name(),
            });
            out.titles.insert(rec.id.clone(), rec.title.clone());
        }
    }
    let mut archived = archived(base);
    archived.retain(|rec| !active.iter().any(|a| a.id == rec.id));
    finished.extend(
        archived
            .iter()
            .filter(|rec| rec.kind == Kind::Issue && rec.stage == Stage::Finished),
    );
    finished.sort_by(|a, b| (a.number, &a.id).cmp(&(b.number, &b.id)));
    for rec in finished {
        let marked = rec.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true));
        let finished_at = rec
            .stages
            .iter()
            .rev()
            .find(|s| s.stage == Stage::Finished)
            .and_then(|s| parse_timestamp(&s.at));
        let since = matches!((finished_at, started), (Some(at), Some(from)) if at >= from);
        // One task can have several archives (`<id>`, `<id>-2`, ..., issue 595): it is done once,
        // checked after the window and the marker so an archive outside them never hides it.
        if since && marked && !out.done.contains(&rec.id) {
            out.done.push(rec.id.clone());
            out.titles.insert(rec.id.clone(), rec.title.clone());
        }
    }
    if !file.issues.is_empty() {
        out.listed = Some(match &file.progress {
            Some(progress) => recorded(file, progress, &mut out, &active, &archived),
            None => listed(file, &out, &active, &archived),
        });
    }
    Ok(out)
}

/// [`Listed`] from the outcomes the loop recorded in its lock (issue 493), which also say the
/// tasks done ([`Work::done`]) and, when no record shows it running, the issue the loop is on:
/// a resume clears the record's process before it takes the task again. The issues with no
/// outcome are still to come: the held ones first, in the order the loop recorded them, which is
/// the order it tries them again in, before a new issue (review M-1); then the rest in the list's
/// order; as many as `--cap` leaves to run, and the others are not run.
fn recorded(
    file: &LockFile,
    progress: &Progress,
    work: &mut Work,
    active: &[Record],
    archived: &[Record],
) -> Listed {
    let record_of = |n: u32| {
        active
            .iter()
            .chain(archived)
            .find(|r| r.kind == Kind::Issue && r.number == n)
    };
    let current = progress.current.filter(|n| file.issues.contains(n));
    if work.current.is_none()
        && let Some(rec) = current.and_then(|n| {
            active
                .iter()
                .find(|r| r.kind == Kind::Issue && r.number == n)
        })
    {
        work.current = Some(Current {
            id: rec.id.clone(),
            stage: rec.stage.name(),
        });
        work.titles.insert(rec.id.clone(), rec.title.clone());
    }
    let mut out = Listed {
        current,
        cap: file.cap,
        counted: Some(progress.counted),
        ..Listed::default()
    };
    work.done.clear();
    let held: Vec<u32> = progress
        .outcomes
        .iter()
        .filter(|o| o.outcome == Outcome::Held && Some(o.issue) != current)
        .map(|o| o.issue)
        .filter(|n| file.issues.contains(n))
        .collect();
    let mut later = Vec::new();
    for &n in &file.issues {
        let seen = [&out.done, &out.parked, &out.skipped, &held, &later];
        if Some(n) == current || seen.iter().any(|list| list.contains(&n)) {
            continue;
        }
        match progress.outcome(n) {
            Some(Outcome::Done) => {
                out.done.push(n);
                let (id, title) = record_of(n).map_or_else(
                    || (format!("issue-{n}"), String::new()),
                    |r| (r.id.clone(), r.title.clone()),
                );
                work.titles.insert(id.clone(), title);
                work.done.push(id);
            }
            Some(Outcome::Parked) => out.parked.push(n),
            Some(Outcome::Skipped) => out.skipped.push(n),
            Some(Outcome::Held) => {}
            None => later.push(n),
        }
    }
    let mut left = file.cap.map_or(usize::MAX, |cap| {
        (cap as usize).saturating_sub(out.ran() + usize::from(current.is_some()))
    });
    for (to_come, shown) in [(held, &mut out.held), (later, &mut out.pending)] {
        for n in to_come {
            if left > 0 {
                left -= 1;
                shown.push(n);
            } else {
                out.not_run.push(n);
            }
        }
    }
    out.not_run
        .sort_by_key(|n| file.issues.iter().position(|i| i == n));
    for &n in &file.issues {
        if let Some(rec) = record_of(n).filter(|r| !r.title.is_empty()) {
            out.titles.entry(n).or_insert_with(|| rec.title.clone());
        }
    }
    out
}

/// [`Listed`] from the lock's list, the records and what [`work_for`] found.
fn listed(file: &LockFile, work: &Work, active: &[Record], archived: &[Record]) -> Listed {
    let issue_of = |id: &str| {
        active
            .iter()
            .chain(archived)
            .find(|r| r.id == id && r.kind == Kind::Issue)
            .map(|r| r.number)
    };
    let current = work.current.as_ref().and_then(|c| issue_of(&c.id));
    let done: Vec<u32> = work.done.iter().filter_map(|id| issue_of(id)).collect();
    let parked = |n: u32| {
        active.iter().any(|r| {
            r.kind == Kind::Issue
                && r.number == n
                && r.stage != Stage::Finished
                && r.status != Status::Running
                && r.owner() == Some(&file.process)
        })
    };
    let reached = file
        .issues
        .iter()
        .rposition(|&n| Some(n) == current || done.contains(&n) || parked(n));
    let mut out = Listed {
        current: current.filter(|n| file.issues.contains(n)),
        cap: file.cap,
        ..Listed::default()
    };
    let mut later = Vec::new();
    for (i, &n) in file.issues.iter().enumerate() {
        if Some(n) == out.current || out.done.contains(&n) || out.parked.contains(&n) {
            continue;
        }
        if done.contains(&n) {
            out.done.push(n);
        } else if parked(n) {
            out.parked.push(n);
        } else if reached.is_some_and(|r| i < r) {
            out.skipped.push(n);
        } else if !later.contains(&n) {
            later.push(n);
        }
    }
    let left = file.cap.map_or(usize::MAX, |cap| {
        (cap as usize).saturating_sub(out.ran() + usize::from(out.current.is_some()))
    });
    let shown = later.len().min(left);
    out.not_run = later.split_off(shown);
    out.pending = later;
    for rec in active.iter().chain(archived) {
        if rec.kind == Kind::Issue && file.issues.contains(&rec.number) && !rec.title.is_empty() {
            out.titles
                .entry(rec.number)
                .or_insert_with(|| rec.title.clone());
        }
    }
    out
}

/// The records under [`merged::archive_root`], read as `task rm --merged` reads task folders:
/// never through a link or a folder whose real place is elsewhere ([`check_own_folder`]), at the
/// root, a task's folder or its `task.json`. Anything not a plain record, or that can't be read,
/// is skipped: the archive is history, and one bad entry must not hide the whole section.
fn archived(base: &Path) -> Vec<Record> {
    let root = merged::archive_root(base);
    if fs::symlink_metadata(&root).is_err() || check_own_folder(base, &root).is_err() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let own = fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir())
            && check_own_folder(base, &dir).is_ok();
        let path = dir.join("task.json");
        let plain = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
        if own
            && plain
            && let Ok(rec) = record::read(&path)
        {
            records.push(rec);
        }
    }
    records
}

/// One issue in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub number: u32,
    pub title: String,
    pub size: Option<&'static str>,
}

/// What GitHub said: the queue and the PRs awaiting the user, each with its own failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub queue: Result<Vec<QueueItem>, String>,
    pub awaiting: Result<Vec<u32>, String>,
    /// The title of each PR in `awaiting` that a task record still names (issue 246), from
    /// [`crate::title::pr_title`]; one whose record is gone has none.
    pub pr_titles: std::collections::BTreeMap<u32, String>,
    /// The title of every open issue, by number, from the same fetch as `queue`: for the issues
    /// of a loop's `--issue` list that have no task record yet (issue 477).
    pub issue_titles: std::collections::BTreeMap<u32, String>,
    /// Why the loop would pass each issue checked over (issue 536): the text `task loop --check`
    /// gives after `skip: `, or `unknown (<first line of the error>)` when the check fails. An
    /// issue it would take has no entry.
    pub blocked: std::collections::BTreeMap<u32, String>,
}

fn first_line(error: &anyhow::Error) -> String {
    format!("{error:#}")
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Asks GitHub for the queue (`gh issue list`) and for the state of each PR a loop opened, and
/// checks each queued issue as `task loop --check` does, as with `cloud_slots` unset.
pub fn fetch(github: &dyn GitHubBackend, repo: &Result<String, String>, base: &Path) -> Snapshot {
    fetch_checking(github, repo, base, None, None)
}

/// [`fetch`], checking the issues of `listed` (a loop's `--issue` list still to come, issue 536)
/// instead of the queue's when given, with `cloud_slots` as `[resources]` sets it (issue 600: a
/// `run-cloud` issue needs it).
pub fn fetch_checking(
    github: &dyn GitHubBackend,
    repo: &Result<String, String>,
    base: &Path,
    listed: Option<&[u32]>,
    cloud_slots: Option<u32>,
) -> Snapshot {
    let repo = match repo {
        Ok(repo) => repo,
        Err(why) => {
            return Snapshot {
                queue: Err(why.clone()),
                awaiting: Err(why.clone()),
                pr_titles: Default::default(),
                issue_titles: Default::default(),
                blocked: Default::default(),
            };
        }
    };
    let open = github.issues_open(repo);
    let issue_titles = open
        .iter()
        .flatten()
        .map(|i| (i.number, i.title.clone()))
        .collect();
    let blocked = open
        .as_ref()
        .map(|open| {
            let checked = listed.map_or_else(|| queue::queue_order(open), <[u32]>::to_vec);
            checked
                .into_iter()
                .filter_map(|n| {
                    match crate::commands::task_loop::skip_reason(
                        base,
                        cloud_slots,
                        repo,
                        github,
                        open,
                        n,
                    ) {
                        Ok(None) => None,
                        Ok(Some(why)) => Some((n, why)),
                        Err(e) => Some((n, format!("unknown ({})", first_line(&e)))),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let queue = open.map(|open| {
        queue::queue_order(&open)
            .into_iter()
            .filter_map(|n| open.iter().find(|i| i.number == n))
            .map(|i| QueueItem {
                number: i.number,
                title: i.title.clone(),
                size: queue::size(&i.labels),
            })
            .collect()
    });
    let awaiting = (|| -> anyhow::Result<Vec<u32>> {
        let mut awaiting = Vec::new();
        for pr in queue::loop_prs(base, repo)? {
            let Some(number) = pr.number else { continue };
            if queue::awaits_user(github.pr_merge_state(repo, number)?) {
                awaiting.push(number);
            }
        }
        awaiting.sort_unstable();
        awaiting.dedup();
        Ok(awaiting)
    })();
    let pr_titles = awaiting
        .iter()
        .flatten()
        .filter_map(|&n| Some((n, crate::title::pr_title(base, repo, n)?)))
        .collect();
    Snapshot {
        queue: queue.map_err(|e| first_line(&e)),
        awaiting: awaiting.map_err(|e| first_line(&e)),
        pr_titles,
        issue_titles,
        blocked,
    }
}

/// The last [`Snapshot`], kept in memory between `--watch` frames.
#[derive(Default)]
pub struct Cache {
    taken: Option<(u64, Snapshot)>,
}

impl Cache {
    /// The cached snapshot while it is under [`REFRESH_SECS`] old at `now`, else a new one from
    /// `fetch`.
    pub fn get(&mut self, now: u64, fetch: impl FnOnce() -> Snapshot) -> &Snapshot {
        let fresh = matches!(&self.taken, Some((at, _)) if now.saturating_sub(*at) < REFRESH_SECS);
        if !fresh {
            self.taken = Some((now, fetch()));
        }
        &self.taken.as_ref().expect("just set").1
    }
}

/// Everything the section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub lock: Lock,
    /// Only for a running loop.
    pub work: Option<Work>,
    pub snapshot: Option<Snapshot>,
    /// The pause on a red `main` (issue 442), whether or not a loop runs.
    pub pause: loop_pause::State,
    /// The job files in the queue folder (issue 528), whether or not a loop runs.
    pub jobs: Jobs,
}

/// What the loop's job queue holds (issue 528, decision 309), read from its files alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jobs {
    /// Each queued job's text, in the order the loop takes them ([`jobs::pending`]), or why the
    /// folder can't be read.
    pub queued: Result<Vec<String>, String>,
    /// Each job in `queue/running/`: its text and its file's modified time.
    pub running: Vec<(String, u64)>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            queued: Ok(Vec::new()),
            running: Vec::new(),
        }
    }
}

/// A job's kind and target: `run #524`, `fix #465 (check run 42452742075)`, `rm-merged`,
/// `spec <file>`, `template <profile>`.
fn job_text(job: &Job) -> String {
    match job {
        Job::Run { issue } => format!("run #{issue}"),
        Job::Review { pr } => format!("review #{pr}"),
        Job::Fix {
            pr,
            check_run: None,
        } => format!("fix #{pr}"),
        Job::Fix {
            pr,
            check_run: Some(run),
        } => format!("fix #{pr} (check run {run})"),
        Job::RmMerged {} => "rm-merged".to_owned(),
        // The loop's own label (`job spec <file>`, `job template <profile> --harness codex
        // --warm`) less its `job ` prefix.
        Job::Spec { .. } | Job::Template { .. } => {
            let label = job.label();
            label.strip_prefix("job ").unwrap_or(&label).to_owned()
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn modified_secs(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Reads `<base>/.sbxm/queue/` and its `running/` folder (issue 528); writes nothing. A queued
/// file that isn't a job is named as one the loop will refuse.
pub fn read_jobs(base: &Path) -> Jobs {
    let queued = jobs::check_queue(base)
        .and_then(|()| jobs::pending(base))
        .map_err(|e| first_line(&e))
        .map(|paths| {
            paths
                .iter()
                .map(|path| {
                    // Read as the loop reads it, so it says what the loop will do.
                    match fs::read(path)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| jobs::parse(&String::from_utf8_lossy(&bytes)))
                    {
                        Ok(job) => job_text(&job),
                        Err(_) => format!("{} (not a job: will be refused)", file_name(path)),
                    }
                })
                .collect()
        });
    let running = match &queued {
        Err(_) => Vec::new(),
        Ok(_) => jobs::left_running(base)
            .unwrap_or_default()
            .iter()
            .map(|path| {
                let text = fs::read(path)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                let job = Running::read(&text)
                    .read
                    .job
                    .and_then(|v| serde_json::from_value::<Job>(v).ok());
                let shown = job.map_or_else(|| file_name(path), |job| job_text(&job));
                (shown, modified_secs(path))
            })
            .collect(),
    };
    Jobs { queued, running }
}

/// The `job:` lines of the jobs in `running/` and the `jobs:` line of the queued ones, at most
/// [`MAX_QUEUE_LINES`] on a terminal (issue 528).
fn job_lines(jobs: &Jobs, loop_runs: bool, width: Option<usize>, lines: &mut Vec<String>) {
    for (job, since) in &jobs.running {
        lines.push(if loop_runs {
            format!("  job: {job} (since {})", format_timestamp(*since))
        } else {
            format!("  job: {job} (interrupted: the next loop reports it)")
        });
    }
    match &jobs.queued {
        Err(why) => lines.push(format!("  jobs: unreadable ({why})")),
        Ok(queued) if queued.is_empty() => lines.push("  jobs: none".to_owned()),
        Ok(queued) => {
            lines.push(format!("  jobs: {}", plural(queued.len(), "job")));
            let shown = if width.is_some() {
                queued.len().min(MAX_QUEUE_LINES)
            } else {
                queued.len()
            };
            for job in &queued[..shown] {
                lines.push(format!("    {job}"));
            }
            if shown < queued.len() {
                lines.push(format!("    … {} more", queued.len() - shown));
            }
        }
    }
}

fn process(p: &Process) -> String {
    format!("process {}, started {}", p.pid, p.started_at)
}

/// The section, after a blank line. Each line is cut to `width` on a terminal (`None`: shown in
/// full), like the Tasks table; the queue shows [`MAX_QUEUE_LINES`] issues on a terminal and then
/// how many more there are.
pub fn render(view: &View, width: Option<usize>) -> String {
    let mut lines = Vec::new();
    match &view.lock {
        Lock::Absent => lines.push("Loop: not running".to_owned()),
        Lock::Stale(p) => lines.push(format!(
            "Loop: not running (stale lock: {} is gone)",
            process(p)
        )),
        Lock::Unreadable(why) => lines.push(format!("Loop: lock unreadable ({why})")),
        Lock::Running(p) => lines.push(format!("Loop: running ({})", process(p))),
    }
    match &view.pause {
        loop_pause::State::None => {}
        loop_pause::State::Set(pause) => lines.push(crate::hint::named(&format!(
            "  paused: {}; nothing is armed until you run `sbxm task loop --clear-pause`",
            pause.describe()
        ))),
        loop_pause::State::Unreadable(why) => lines.push(crate::hint::named(&format!(
            "  paused: the pause file can't be read ({why}); nothing is armed until you run \
             `sbxm task loop --clear-pause`"
        ))),
    }
    if let Some(work) = &view.work {
        let named =
            |id: &str| crate::title::named(id, work.titles.get(id).map_or("", String::as_str));
        let of = work.listed.as_ref().map(|l| (l.position(), l.total()));
        lines.push(match (&work.current, of) {
            (Some(c), Some((Some(n), m))) => {
                format!("  task: {} at {}, task {n} of {m}", named(&c.id), c.stage)
            }
            (Some(c), _) => format!("  task: {} at {}", named(&c.id), c.stage),
            // The loop's recorded issue before its task record exists (issue 493).
            (None, Some((Some(n), m))) => {
                let issue = work.listed.as_ref().and_then(|l| l.current);
                let title = issue
                    .and_then(|i| {
                        work.listed
                            .as_ref()
                            .and_then(|l| l.titles.get(&i))
                            .or_else(|| view.snapshot.as_ref()?.issue_titles.get(&i))
                    })
                    .map_or("", String::as_str);
                let issue = issue.map_or_else(String::new, |i| format!("#{i}"));
                format!(
                    "  task: {}, task {n} of {m}",
                    crate::title::named(&issue, title)
                )
            }
            (None, Some((_, m))) => format!(
                "  task: none running, {} of {m} run",
                work.listed.as_ref().map_or(0, Listed::ran)
            ),
            (None, None) => "  task: none running".to_owned(),
        });
        lines.push(if work.done.is_empty() {
            "  done: none yet".to_owned()
        } else {
            let done: Vec<String> = work.done.iter().map(|id| named(id)).collect();
            format!("  done: {} ({})", work.done.len(), done.join(", "))
        });
        job_lines(&view.jobs, true, width, &mut lines);
        if let Some(listed) = &work.listed {
            list_lines(listed, view.snapshot.as_ref(), width, &mut lines);
        }
    } else {
        let loop_runs = matches!(view.lock, Lock::Running(_));
        job_lines(&view.jobs, loop_runs, width, &mut lines);
    }
    if let Some(snapshot) = &view.snapshot {
        lines.push(match &snapshot.awaiting {
            Ok(prs) => {
                let list: Vec<String> = prs
                    .iter()
                    .map(|n| {
                        let title = snapshot.pr_titles.get(n).map_or("", String::as_str);
                        crate::title::titled(&format!("#{n}"), title)
                    })
                    .collect();
                if prs.is_empty() {
                    format!(
                        "  awaiting you: 0 of {} PRs",
                        crate::commands::task_loop::PR_CAP
                    )
                } else {
                    format!(
                        "  awaiting you: {} of {} PRs ({})",
                        prs.len(),
                        crate::commands::task_loop::PR_CAP,
                        list.join(", ")
                    )
                }
            }
            Err(why) => format!("  awaiting you: unreadable ({why})"),
        });
        // A loop over an `--issue` list doesn't take the labeled queue (issue 477), but the same
        // read gives its pending issues' titles, so its failure is still said (review M-1).
        let listed = view.work.as_ref().is_some_and(|w| w.listed.is_some());
        match &snapshot.queue {
            Err(why) if listed => lines.push(format!("  issue titles: unreadable ({why})")),
            _ if listed => {}
            Err(why) => lines.push(format!("  queue: unreadable ({why})")),
            Ok(items) if items.is_empty() => lines.push("  queue: empty".to_owned()),
            Ok(items) => {
                let numbers: Vec<u32> = items.iter().map(|i| i.number).collect();
                lines.push(format!(
                    "  queue: {}{}",
                    plural(items.len(), "issue"),
                    blocked_count(&numbers, Some(snapshot))
                ));
                let shown = if width.is_some() {
                    items.len().min(MAX_QUEUE_LINES)
                } else {
                    items.len()
                };
                for item in &items[..shown] {
                    let size = item.size.map_or(String::new(), |s| format!(" (size {s})"));
                    let item_titled =
                        crate::title::titled(&format!("#{}", item.number), &item.title);
                    let why = blocked(item.number, Some(snapshot));
                    lines.push(format!("    {item_titled}{size}{why}"));
                }
                if shown < items.len() {
                    lines.push(format!("    … {} more", items.len() - shown));
                }
            }
        }
    }
    let mut out = String::from("\n");
    for line in lines {
        match width {
            Some(width) => out.push_str(&truncate(&line, width)),
            None => out.push_str(&line),
        }
        out.push('\n');
    }
    out
}

/// The lines of a loop's `--issue` list (issue 477): its parked, skipped, pending and not-run
/// issues, each with its title from its task record or else from GitHub (issue 246). The pending
/// issues head their own lines, as the queue's do, at most [`MAX_QUEUE_LINES`] on a terminal.
fn list_lines(
    listed: &Listed,
    snapshot: Option<&Snapshot>,
    width: Option<usize>,
    lines: &mut Vec<String>,
) {
    let title = |n: u32| {
        listed
            .titles
            .get(&n)
            .or_else(|| snapshot.and_then(|s| s.issue_titles.get(&n)))
            .map_or("", String::as_str)
    };
    let inline = |issues: &[u32]| {
        issues
            .iter()
            .map(|&n| crate::title::named(&format!("#{n}"), title(n)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !listed.parked.is_empty() {
        lines.push(format!("  parked: {}", inline(&listed.parked)));
    }
    if !listed.skipped.is_empty() {
        lines.push(format!("  skipped: {}", inline(&listed.skipped)));
    }
    if !listed.held.is_empty() {
        lines.push(format!(
            "  held: {}, tried again after the running task",
            inline(&listed.held)
        ));
    }
    if listed.pending.is_empty() {
        lines.push("  pending: none".to_owned());
    } else {
        lines.push(format!(
            "  pending: {}{}",
            plural(listed.pending.len(), "issue"),
            blocked_count(&listed.pending, snapshot)
        ));
        let shown = if width.is_some() {
            listed.pending.len().min(MAX_QUEUE_LINES)
        } else {
            listed.pending.len()
        };
        for &n in &listed.pending[..shown] {
            lines.push(format!(
                "    {}{}",
                crate::title::titled(&format!("#{n}"), title(n)),
                blocked(n, snapshot)
            ));
        }
        if shown < listed.pending.len() {
            lines.push(format!("    … {} more", listed.pending.len() - shown));
        }
    }
    if !listed.not_run.is_empty() {
        lines.push(format!(
            "  not run (--cap {}): {}",
            listed.cap.unwrap_or_default(),
            inline(&listed.not_run)
        ));
    }
}

/// ` (N blocked)` for the issues of `numbers` the loop would pass over, or nothing when none is
/// (issue 536).
fn blocked_count(numbers: &[u32], snapshot: Option<&Snapshot>) -> String {
    let n = numbers
        .iter()
        .filter(|n| snapshot.is_some_and(|s| s.blocked.contains_key(n)))
        .count();
    if n == 0 {
        String::new()
    } else {
        format!(" ({n} blocked)")
    }
}

/// ` — blocked: <why>` after an issue the loop would pass over, or nothing (issue 536).
fn blocked(number: u32, snapshot: Option<&Snapshot>) -> String {
    snapshot
        .and_then(|s| s.blocked.get(&number))
        .map_or_else(String::new, |why| format!(" — blocked: {why}"))
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
