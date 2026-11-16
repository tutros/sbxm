//! The per-task mutation lock, `<base>/.sbxm/tasks/<id>/lock.json` (issue 58): every command that
//! changes a task's record holds it from before it reads the record until after its last write,
//! so two commands on one task can't both pass the record's own checks and then overwrite each
//! other. Read-only commands (`task status`, `task logs`) never take it.
//!
//! [`ProcessLock`] is the shared helper (`shared::lock`, re-exported here, issue 539): a file made
//! with `create_new` holding the process that holds it ([`Process`], pid and start time) and a
//! token of its own. A live holder refuses at once; a holder that is gone (its pid isn't running,
//! or runs with another start time) has its lock taken over. Dropping the lock removes the file,
//! but only while it is still this lock.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub use crate::shared::lock::*;
use crate::shared::process::{Process, ProcessProbe};

/// A task's [`ProcessLock`], taken in its folder: letting it go ends this process's run of the
/// task (issue 597). Whatever the outcome (failed, parked, ready, finished, a refusal after the
/// record was written), `task.json` then stops naming this process, before the lock file goes,
/// so `process` names a live owner only while a command really works on the task. Only a record
/// this hold made name this process is changed: one naming another process, or none, or this
/// process already when the lock was taken (a command that refuses before it writes changes
/// nothing), is left alone; so is a folder that is gone (`task rm`). A process killed mid-run
/// never gets here, and its record keeps naming it.
#[derive(Debug)]
pub struct TaskLock {
    lock: ProcessLock,
    dir: PathBuf,
    me: Process,
    /// Whether `task.json` named this process when the lock was taken.
    named_me: bool,
}

impl TaskLock {
    /// Takes `<dir>/lock.json` for this process, as [`ProcessLock::acquire`] does.
    pub fn acquire(dir: &Path, what: &str, probe: &dyn ProcessProbe) -> Result<Self> {
        let lock = ProcessLock::acquire(dir, what, probe)?;
        let me = Process::current(probe);
        Ok(Self {
            lock,
            dir: dir.to_owned(),
            named_me: names(dir, &me),
            me,
        })
    }

    /// The task's folder moved to `dir` with the lock file in it: see [`ProcessLock::moved_to`].
    pub fn moved_to(&mut self, dir: &Path) {
        self.lock.moved_to(dir);
        self.dir = dir.to_owned();
    }
}

impl Drop for TaskLock {
    fn drop(&mut self) {
        if self.named_me {
            return;
        }
        let Ok(mut task) = super::record::read(&self.dir.join("task.json")) else {
            return;
        };
        if task.process.as_ref() == Some(&self.me) {
            task.process = None;
            let _ = super::record::write(&self.dir, &task);
        }
    }
}

/// Whether `<dir>/task.json` can be read and names `process`.
fn names(dir: &Path, process: &Process) -> bool {
    super::record::read(&dir.join("task.json"))
        .is_ok_and(|task| task.process.as_ref() == Some(process))
}
