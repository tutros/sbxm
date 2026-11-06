//! The Loop section of `sbxm status` (decision 294, issue 376): whether a `task loop` runs, the
//! task it is on and its stage, the tasks it finished, the PRs awaiting the user (the loop's cap
//! of 5, counted by the same [`queue::awaits_user`] the loop pauses on) and the queue of labeled
//! issues in the order the loop takes them, and the pause on a red `main` (issue 442). State
//! comes from the loop's lock, its pause file and the task records alone, with no write; only the queue and the PR count need GitHub, one
//! fetch at most once a minute ([`Cache`]), made only while a loop runs.

use std::fs;
use std::path::Path;

use crate::github::GitHubBackend;
use crate::run::results::parse_timestamp;
use crate::status::tasks::truncate;
use crate::task::finish::check_own_folder;
use crate::task::loop_pause;
use crate::task::merged;
use crate::task::queue::{self, LOOP_HOOK};
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
    let path = queue::lock_path(base);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Lock::Absent,
        Err(e) => return Lock::Unreadable(format!("cannot read {}: {e}", path.display())),
    };
    match serde_json::from_str::<Process>(&text) {
        Ok(held) if held.alive(probe) => Lock::Running(held),
        Ok(held) => Lock::Stale(held),
        Err(e) => Lock::Unreadable(format!("cannot parse {}: {e}", path.display())),
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
}

/// The loop's task and the tasks it finished, from the records under `base` (never `sbx`).
/// The task is the issue task still `running` whose recorded process equals the lock's: the loop
/// runs its phases in its own process, and its marker (`hooks.task_loop`) is written only when
/// it claims a ready task for `task finish`, so it can't identify work in progress (decision
/// 294, approved by the user 2026-11-06). The tasks done are the marked records finished since
/// the lock's process started, the archived ones included: the loop's own merged-task cleanup
/// archives a task and removes its folder once its PR merges (decision 195), and that task is
/// still done in this window (review M-1 of PR 437). A task in both places, between the archive
/// and the removal, counts once; only the active records say which task runs.
pub fn work(base: &Path, lock: &Process) -> anyhow::Result<Work> {
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
        } else if rec.status == Status::Running && rec.process.as_ref() == Some(lock) {
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
        if since && marked {
            out.done.push(rec.id.clone());
            out.titles.insert(rec.id.clone(), rec.title.clone());
        }
    }
    Ok(out)
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
}

fn first_line(error: &anyhow::Error) -> String {
    format!("{error:#}")
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Asks GitHub for the queue (`gh issue list`) and for the state of each PR a loop opened.
pub fn fetch(github: &dyn GitHubBackend, repo: &Result<String, String>, base: &Path) -> Snapshot {
    let repo = match repo {
        Ok(repo) => repo,
        Err(why) => {
            return Snapshot {
                queue: Err(why.clone()),
                awaiting: Err(why.clone()),
                pr_titles: Default::default(),
            };
        }
    };
    let queue = github.issues_open(repo).map(|open| {
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
        .filter_map(|&n| Some((n, crate::title::pr_title(base, n)?)))
        .collect();
    Snapshot {
        queue: queue.map_err(|e| first_line(&e)),
        awaiting: awaiting.map_err(|e| first_line(&e)),
        pr_titles,
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
        loop_pause::State::Set(pause) => lines.push(format!(
            "  paused: {}; nothing is armed until you run `sbxm task loop --clear-pause`",
            pause.describe()
        )),
        loop_pause::State::Unreadable(why) => lines.push(format!(
            "  paused: the pause file can't be read ({why}); nothing is armed until you run \
             `sbxm task loop --clear-pause`"
        )),
    }
    if let Some(work) = &view.work {
        let named =
            |id: &str| crate::title::named(id, work.titles.get(id).map_or("", String::as_str));
        lines.push(match &work.current {
            Some(c) => format!("  task: {} at {}", named(&c.id), c.stage),
            None => "  task: none running".to_owned(),
        });
        lines.push(if work.done.is_empty() {
            "  done: none yet".to_owned()
        } else {
            let done: Vec<String> = work.done.iter().map(|id| named(id)).collect();
            format!("  done: {} ({})", work.done.len(), done.join(", "))
        });
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
        match &snapshot.queue {
            Err(why) => lines.push(format!("  queue: unreadable ({why})")),
            Ok(items) if items.is_empty() => lines.push("  queue: empty".to_owned()),
            Ok(items) => {
                lines.push(format!("  queue: {}", plural(items.len(), "issue")));
                let shown = if width.is_some() {
                    items.len().min(MAX_QUEUE_LINES)
                } else {
                    items.len()
                };
                for item in &items[..shown] {
                    let size = item.size.map_or(String::new(), |s| format!(" (size {s})"));
                    let item_titled =
                        crate::title::titled(&format!("#{}", item.number), &item.title);
                    lines.push(format!("    {item_titled}{size}"));
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

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
