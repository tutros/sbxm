//! `run.log` in a task's folder (decision 170, part 1): a copy of what a task command prints, one
//! stamped line per line with the task's stage, and a header for each invocation. Nothing is
//! logged that the screen did not show, and values that look like secrets are masked anyway.
//! The lines of an `sbx create` or `sbx rm` go to the screen and, through [`task_line`], to the
//! log of the task whose sandbox it is, and only that task's (issue 114).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;

use super::record::{self, is_valid_id};
use crate::run::results::{format_timestamp, now};

/// The stage shown for a line printed before the task has a folder (or when its record can't be read).
const NO_STAGE: &str = "-";

/// Values that look like an API key or token: `sk-…`, `ghp_…` (and the other `gh*_` kinds),
/// `github_pat_…` and Google `AIza…` keys.
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|AIza[A-Za-z0-9_-]{16,})")
        .expect("the secret pattern is valid")
});

/// `text` with every value that looks like a secret replaced by `[redacted]`.
pub fn redact(text: &str) -> String {
    SECRET.replace_all(text, "[redacted]").into_owned()
}

/// The first line of each invocation's section: when, the command line, the executable, its
/// SHA-256, the commit if the build knows it, and the process id.
pub fn header(
    secs: u64,
    args: &[String],
    exe: &Path,
    sha256: &str,
    commit: Option<&str>,
    pid: u32,
) -> String {
    redact(&format!(
        "# {} {} | exe {} | sha256 {} | commit {} | pid {}",
        format_timestamp(secs),
        args.join(" "),
        exe.display(),
        sha256,
        commit.unwrap_or("unknown"),
        pid
    ))
}

/// The stage in the task's own record, for the stamp on a line.
pub fn recorded_stage(dir: &Path) -> String {
    record::read(&dir.join("task.json"))
        .map_or_else(|_| NO_STAGE.to_owned(), |r| r.stage.name().to_owned())
}

pub struct RunLog {
    base: std::path::PathBuf,
    header: String,
    /// The tasks the command names (`issue-5`), whether or not their folders exist yet.
    named: Vec<String>,
    /// Also log to every task this process records in its `task.json` (`task start` picks them).
    /// Both the pid and the start time must match (decision 163): a pid alone can be reused by an
    /// unrelated later process, which would otherwise make a stale task look like this one's.
    identity: Option<record::Process>,
    clock: Box<dyn Fn() -> u64 + Send>,
    stage_of: Box<dyn Fn(&Path) -> String + Send>,
    /// Tasks found to be this process's.
    found: BTreeSet<String>,
    /// Folders already looked at and found to belong to another process.
    foreign: BTreeSet<String>,
    /// Every line of this invocation, to catch a task up when its folder appears after them.
    history: Vec<(u64, String)>,
    /// How much of `history` each task's log already has.
    caught_up: BTreeMap<String, usize>,
    /// Ids already given this invocation's header, so a `run.log` left over from an earlier
    /// invocation still gets a fresh header on its first write, not just a file that didn't exist.
    headered: BTreeSet<String>,
    /// Told once, not once per line, when a log can't be appended to (an actual I/O failure, not
    /// a folder that merely doesn't exist yet): the exact text to show the user.
    warn: Box<dyn FnMut(&str) + Send>,
    /// Ids already warned about, so a failure that keeps happening says so only once.
    warned: BTreeSet<String>,
}

impl RunLog {
    pub fn new(
        base: &Path,
        header: String,
        named: Vec<String>,
        identity: Option<record::Process>,
        clock: Box<dyn Fn() -> u64 + Send>,
        stage_of: Box<dyn Fn(&Path) -> String + Send>,
        warn: Box<dyn FnMut(&str) + Send>,
    ) -> Self {
        Self {
            base: base.to_owned(),
            header,
            named,
            identity,
            clock,
            stage_of,
            found: BTreeSet::new(),
            foreign: BTreeSet::new(),
            history: Vec::new(),
            caught_up: BTreeMap::new(),
            headered: BTreeSet::new(),
            warn,
            warned: BTreeSet::new(),
        }
    }

    /// Adds one line to the log of every task this command is working on. A log that can't be
    /// written is skipped: the screen already has the line, and the command must not fail for it.
    /// (`write_to`'s own error is still reported, once per id, through `warn`.)
    pub fn line(&mut self, text: &str) {
        let secs = (self.clock)();
        self.history.push((secs, redact(text)));
        self.find_own_tasks();
        let ids: Vec<String> = self
            .named
            .iter()
            .chain(self.found.iter())
            .cloned()
            .collect();
        let newest = self.history.len() - 1;
        let line = self.history[newest].clone();
        for id in ids {
            if let Err(err) = self.write_to(&id, newest, &line)
                && self.warned.insert(id.clone())
            {
                let path = record::task_dir(&self.base, &id).join("run.log");
                (self.warn)(&format!("warning: cannot write {}: {err}", path.display()));
            }
        }
    }

    /// Adds one line to the log of the task `id` only (issue 114: an `sbx` line of its sandbox),
    /// after this invocation's header and the lines printed before, if they aren't there yet. The
    /// line is not kept for catching up another task's log. Returns false, writing nothing, for a
    /// task this command neither names nor owns.
    pub fn line_for(&mut self, id: &str, text: &str) -> bool {
        self.find_own_tasks();
        if !self.named.iter().any(|n| n == id) && !self.found.contains(id) {
            return false;
        }
        let line = ((self.clock)(), redact(text));
        let upto = self.history.len();
        if let Err(err) = self.write_to(id, upto, &line)
            && self.warned.insert(id.to_owned())
        {
            let path = record::task_dir(&self.base, id).join("run.log");
            (self.warn)(&format!("warning: cannot write {}: {err}", path.display()));
        }
        true
    }

    /// Whether `dir` is the folder of the task `id` under this log's base folder.
    fn is_task_dir(&self, id: &str, dir: &Path) -> bool {
        record::task_dir(&self.base, id) == dir
    }

    fn find_own_tasks(&mut self) {
        let Some(identity) = &self.identity else {
            return;
        };
        let Ok(entries) = fs::read_dir(record::tasks_root(&self.base)) else {
            return;
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if !is_valid_id(&id) || self.found.contains(&id) || self.foreign.contains(&id) {
                continue;
            }
            // A folder without a readable record is still being made: look again next line.
            if let Ok(rec) = record::read(&entry.path().join("task.json")) {
                if rec.process.as_ref() == Some(identity) {
                    self.found.insert(id);
                } else {
                    self.foreign.insert(id);
                }
            }
        }
    }

    /// Appends `line` to the task's log. A log made now starts with the header and the lines of
    /// `history` before `upto`, printed before it could be made; a folder that was removed is left removed. A
    /// `run.log` left over from an earlier invocation still gets this invocation's own header, on
    /// its first write here, so each invocation's errors stay traceable to their own header
    /// (issue 129 M-1) instead of trailing silently after a previous one's. `headered` is only
    /// marked once the write actually succeeds (issue 130): a failed first append must not make a
    /// later, successful one skip the header.
    fn write_to(&mut self, id: &str, upto: usize, line: &(u64, String)) -> std::io::Result<()> {
        let dir = record::task_dir(&self.base, id);
        if !dir.is_dir() {
            return Ok(());
        }
        let path = dir.join("run.log");
        let first_write_this_invocation = !self.headered.contains(id);
        let mut text = String::new();
        if !path.exists() {
            text.push_str(&self.header);
            text.push('\n');
            let from = self.caught_up.get(id).copied().unwrap_or(0);
            for (secs, line) in &self.history[from.min(upto)..upto] {
                text.push_str(&format!(
                    "{} [{NO_STAGE}] {line}\n",
                    format_timestamp(*secs)
                ));
            }
        } else if first_write_this_invocation {
            text.push_str(&self.header);
            text.push('\n');
        }
        let (secs, line) = line;
        text.push_str(&format!(
            "{} [{}] {line}\n",
            format_timestamp(*secs),
            (self.stage_of)(&dir)
        ));
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(text.as_bytes())?;
        self.headered.insert(id.to_owned());
        self.caught_up.insert(id.to_owned(), self.history.len());
        Ok(())
    }
}

/// The command logs that take the lines meant for one of their tasks only ([`route`]), each with
/// the number its [`Route`] removes it by.
static ROUTES: Mutex<Vec<(u64, Arc<Mutex<RunLog>>)>> = Mutex::new(Vec::new());

/// The number of the next [`Route`].
static NEXT_ROUTE: AtomicU64 = AtomicU64::new(0);

/// A command log that [`task_line`] goes through, until this is dropped.
pub struct Route(u64);

impl Drop for Route {
    fn drop(&mut self) {
        routes().retain(|(n, _)| *n != self.0);
    }
}

fn routes() -> std::sync::MutexGuard<'static, Vec<(u64, Arc<Mutex<RunLog>>)>> {
    ROUTES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes [`task_line`] go through `log` for the tasks it names or owns, until the returned
/// [`Route`] is dropped: their `run.log` then gets this invocation's header and earlier lines
/// first, as for any line the command prints.
pub fn route(log: &Arc<Mutex<RunLog>>) -> Route {
    let n = NEXT_ROUTE.fetch_add(1, Ordering::SeqCst);
    routes().push((n, Arc::clone(log)));
    Route(n)
}

/// Adds `text`, a line of an `sbx create` or `sbx rm` the screen already shows, to the `run.log`
/// in the task folder `dir` only (issue 114): through a routed command log that has the task
/// ([`route`]), else appended as is, stamped and masked like a command's line. A folder that
/// doesn't exist gets nothing, and a log that can't be written is skipped: the screen has the line.
pub fn task_line(dir: &Path, text: &str) {
    if let Some(id) = dir.file_name().and_then(|n| n.to_str()) {
        let logs: Vec<Arc<Mutex<RunLog>>> = routes().iter().map(|(_, l)| Arc::clone(l)).collect();
        for log in logs {
            let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
            if log.is_task_dir(id, dir) && log.line_for(id, text) {
                return;
            }
        }
    }
    if !dir.is_dir() {
        return;
    }
    let line = format!(
        "{} [{}] {}\n",
        format_timestamp(now()),
        recorded_stage(dir),
        redact(text)
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("run.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Where a [`Tee`] sends each whole line: a task's [`RunLog`], or a run folder's log
/// (`run::log::RunFolderLog`, decision 170, part 3).
pub trait LineSink {
    fn line(&mut self, text: &str);
}

impl LineSink for RunLog {
    fn line(&mut self, text: &str) {
        RunLog::line(self, text);
    }
}

/// A writer that passes everything to `inner` unchanged and hands each whole line to the log.
pub struct Tee<W: Write, L: LineSink = RunLog> {
    inner: W,
    log: Arc<Mutex<L>>,
    partial: Vec<u8>,
}

impl<W: Write, L: LineSink> Tee<W, L> {
    pub fn new(inner: W, log: Arc<Mutex<L>>) -> Self {
        Self {
            inner,
            log,
            partial: Vec::new(),
        }
    }

    fn ship(&mut self, all: bool) {
        while let Some(end) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.send(&line[..end]);
        }
        if all && !self.partial.is_empty() {
            let rest = std::mem::take(&mut self.partial);
            self.send(&rest);
        }
    }

    fn send(&self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        if let Ok(mut log) = self.log.lock() {
            log.line(text.trim_end_matches('\r'));
        }
    }
}

impl<W: Write, L: LineSink> Write for Tee<W, L> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.partial.extend_from_slice(&buf[..written]);
        self.ship(false);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write, L: LineSink> Drop for Tee<W, L> {
    fn drop(&mut self) {
        self.ship(true);
    }
}
