//! `run.log` in a task's folder (decision 170, part 1): a copy of what a task command prints, one
//! stamped line per line with the task's stage, and a header for each invocation. Nothing is
//! logged that the screen did not show, and values that look like secrets are masked anyway.
//! The `sbx` child processes write straight to the terminal, so their output is not here (#114).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;

use super::record::{self, is_valid_id};
use crate::run::results::format_timestamp;

/// The stage shown for a line printed before the task has a folder (or when its record can't be read).
const NO_STAGE: &str = "-";

/// Values that look like an API key or token: `sk-…`, `ghp_…` (and the other `gh*_` kinds),
/// `github_pat_…` and Google `AIza…` keys.
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|AIza[A-Za-z0-9_-]{16,})")
        .expect("the secret pattern is valid")
});

fn redact(text: &str) -> String {
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
}

impl RunLog {
    pub fn new(
        base: &Path,
        header: String,
        named: Vec<String>,
        identity: Option<record::Process>,
        clock: Box<dyn Fn() -> u64 + Send>,
        stage_of: Box<dyn Fn(&Path) -> String + Send>,
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
        }
    }

    /// Adds one line to the log of every task this command is working on. A log that can't be
    /// written is skipped: the screen already has the line, and the command must not fail for it.
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
        for id in ids {
            let _ = self.write_to(&id);
        }
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

    /// Appends the newest line to the task's log. A log made now starts with the header and the
    /// lines printed before it could be made; a folder that was removed is left removed.
    fn write_to(&mut self, id: &str) -> std::io::Result<()> {
        let dir = record::task_dir(&self.base, id);
        if !dir.is_dir() {
            return Ok(());
        }
        let path = dir.join("run.log");
        let newest = self.history.len() - 1;
        let mut text = String::new();
        if !path.exists() {
            text.push_str(&self.header);
            text.push('\n');
            let from = self.caught_up.get(id).copied().unwrap_or(0);
            for (secs, line) in &self.history[from..newest] {
                text.push_str(&format!(
                    "{} [{NO_STAGE}] {line}\n",
                    format_timestamp(*secs)
                ));
            }
        }
        let (secs, line) = &self.history[newest];
        text.push_str(&format!(
            "{} [{}] {line}\n",
            format_timestamp(*secs),
            (self.stage_of)(&dir)
        ));
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(text.as_bytes())?;
        self.caught_up.insert(id.to_owned(), self.history.len());
        Ok(())
    }
}

/// A writer that passes everything to `inner` unchanged and hands each whole line to the log.
pub struct Tee<W: Write> {
    inner: W,
    log: Arc<Mutex<RunLog>>,
    partial: Vec<u8>,
}

impl<W: Write> Tee<W> {
    pub fn new(inner: W, log: Arc<Mutex<RunLog>>) -> Self {
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

impl<W: Write> Write for Tee<W> {
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

impl<W: Write> Drop for Tee<W> {
    fn drop(&mut self) {
        self.ship(true);
    }
}
