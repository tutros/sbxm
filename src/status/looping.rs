//! The Loop section of `sbxm status` (decision 294, issue 376): whether a `task loop` runs, the
//! task it is on and its stage, the tasks it finished, the PRs awaiting the user (the loop's cap
//! of 5, counted by the same [`queue::awaits_user`] the loop pauses on) and the queue of labeled
//! issues in the order the loop takes them. State comes from the loop's lock and the task
//! records alone, no new file and no write; only the queue and the PR count need GitHub, one
//! fetch at most once a minute ([`Cache`]), made only while a loop runs.

use std::fs;
use std::path::Path;

use crate::github::GitHubBackend;
use crate::run::results::parse_timestamp;
use crate::status::tasks::truncate;
use crate::task::queue::{self, LOOP_HOOK};
use crate::task::record::{self, Kind, Process, ProcessProbe, Stage, Status};

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
}

/// The loop's task and the tasks it finished, from the records under `base` (never `sbx`).
/// The task is the marked (`hooks.task_loop`) issue task still running: `running` with its
/// process alive (decision 294). The tasks done are the marked records finished since the lock's
/// process started.
pub fn work(base: &Path, lock: &Process, probe: &dyn ProcessProbe) -> anyhow::Result<Work> {
    let started = parse_timestamp(&lock.started_at);
    let mut out = Work::default();
    for rec in record::load_all(base)? {
        if rec.kind != Kind::Issue {
            continue;
        }
        let marked = rec.hooks.get(LOOP_HOOK) == Some(&serde_json::Value::Bool(true));
        if rec.stage != Stage::Finished {
            if marked && rec.status == Status::Running && !rec.is_interrupted(probe) {
                out.current = Some(Current {
                    id: rec.id.clone(),
                    stage: rec.stage.name(),
                });
            }
            continue;
        }
        let finished_at = rec
            .stages
            .iter()
            .rev()
            .find(|s| s.stage == Stage::Finished)
            .and_then(|s| parse_timestamp(&s.at));
        let since = matches!((finished_at, started), (Some(at), Some(from)) if at >= from);
        if since && marked {
            out.done.push(rec.id.clone());
        }
    }
    Ok(out)
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
    Snapshot {
        queue: queue.map_err(|e| first_line(&e)),
        awaiting: awaiting.map_err(|e| first_line(&e)),
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
    if let Some(work) = &view.work {
        lines.push(match &work.current {
            Some(c) => format!("  task: {} at {}", c.id, c.stage),
            None => "  task: none running".to_owned(),
        });
        lines.push(if work.done.is_empty() {
            "  done: none yet".to_owned()
        } else {
            format!("  done: {} ({})", work.done.len(), work.done.join(", "))
        });
    }
    if let Some(snapshot) = &view.snapshot {
        lines.push(match &snapshot.awaiting {
            Ok(prs) => {
                let list: Vec<String> = prs.iter().map(|n| format!("#{n}")).collect();
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
                    lines.push(format!("    #{} {}{size}", item.number, item.title));
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
