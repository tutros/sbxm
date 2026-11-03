//! The per-task mutation lock, `<base>/.sbxm/tasks/<id>/lock.json` (issue 58): every command that
//! changes a task's record holds it from before it reads the record until after its last write,
//! so two commands on one task can't both pass the record's own checks and then overwrite each
//! other. Read-only commands (`task status`, `task logs`) never take it.
//!
//! [`ProcessLock`] is the shared helper: a file made with `create_new` holding the process that
//! holds it ([`Process`], pid and start time) and a token of its own. A live holder refuses at
//! once; a holder that is gone (its pid isn't running, or runs with another start time) has its
//! lock taken over. Dropping the lock removes the file, but only while it is still this lock.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::record::{Process, ProcessProbe};

/// The lock file's name in its folder.
pub const FILE_NAME: &str = "lock.json";

/// How many times a lock that is gone, stale or being taken over is looked at again before the
/// command gives up, and how long it waits between two looks.
const ATTEMPTS: u32 = 50;
const PAUSE: Duration = Duration::from_millis(10);

/// What the lock file holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Holder {
    #[serde(flatten)]
    process: Process,
    /// Tells two locks of one process apart (two threads, or a lock taken again after its folder
    /// moved), so a lock is only ever removed by the value that made it.
    token: String,
}

/// Held while a command changes a task; dropping it (on return, an error or a panic) removes the
/// lock file if it still names this lock. A crash without unwinding leaves the file, naming a
/// process that is gone, and the next command takes it over.
#[derive(Debug)]
pub struct ProcessLock {
    path: PathBuf,
    /// The file's exact contents, as this lock wrote them.
    me: String,
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|text| text == self.me) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

impl ProcessLock {
    /// Takes `<dir>/lock.json` for this process; `what` names the locked thing in messages
    /// (`task issue-58`). A lock whose holder is alive refuses, naming it; one whose holder is
    /// gone is taken over. Only one of two commands that both find a stale lock takes it: the
    /// takeover itself is claimed with `create_new` (`lock.json.takeover`), and the other then
    /// finds the winner's live lock and refuses.
    pub fn acquire(dir: &Path, what: &str, probe: &dyn ProcessProbe) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let me = serde_json::to_string(&Holder {
            process: Process::current(probe),
            token: token(),
        })?;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(PAUSE);
            }
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let lock = Self {
                        path,
                        me: me.clone(),
                    };
                    file.write_all(me.as_bytes())
                        .with_context(|| format!("cannot write {}", lock.path.display()))?;
                    return Ok(lock);
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("cannot create {}", path.display()));
                }
            }
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("cannot read {}", path.display()));
                }
            };
            // An empty or half-written file is a lock still being written: looked at again.
            let Ok(held) = serde_json::from_str::<Holder>(&text) else {
                if attempt + 1 == ATTEMPTS {
                    bail!(
                        "cannot read the lock {} of {what}; if no sbxm command is working on it, \
                         delete the file and run the command again",
                        path.display()
                    );
                }
                continue;
            };
            if held.process.alive(probe) {
                bail!(
                    "{what} is in use by another sbxm command (process {}, started {}); wait for \
                     it to finish, then run this command again",
                    held.process.pid,
                    held.process.started_at
                );
            }
            take_over_stale(&path, &text)?;
        }
        let claim = takeover_path(&path);
        if claim.exists() {
            bail!(
                "cannot take the stale lock {} of {what}: {} says another sbxm command is taking \
                 it over; if none is running, delete {} and run this command again",
                path.display(),
                claim.display(),
                claim.display()
            );
        }
        bail!(
            "cannot take the lock {} of {what}; another sbxm command may be taking it, run this \
             command again",
            path.display()
        )
    }

    /// The lock's folder moved to `dir` with the lock file in it (a `--restart` setting the old
    /// task aside): the lock is now released from there.
    pub fn moved_to(&mut self, dir: &Path) {
        self.path = dir.join(FILE_NAME);
    }
}

/// The takeover claim next to the lock at `path`: `lock.json.takeover`.
fn takeover_path(path: &Path) -> PathBuf {
    path.with_extension("json.takeover")
}

/// Removes the stale lock at `path` if it still reads `stale`, under the takeover claim; a claim
/// held by another command leaves it to that command.
fn take_over_stale(path: &Path, stale: &str) -> Result<()> {
    let claim = takeover_path(path);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&claim)
    {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("cannot create {}", claim.display())),
    }
    let removed = match fs::read_to_string(path) {
        Ok(text) if text == stale => fs::remove_file(path)
            .or_else(|e| match e.kind() {
                ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            })
            .with_context(|| format!("cannot remove the stale lock {}", path.display())),
        _ => Ok(()),
    };
    let _ = fs::remove_file(&claim);
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::results::format_timestamp;

    /// This process started at 1000; every other pid is gone, except `other`, started at 2000.
    struct Probe {
        other: Option<u32>,
    }

    impl ProcessProbe for Probe {
        fn start_time(&self, pid: u32) -> Option<u64> {
            if pid == std::process::id() {
                Some(1000)
            } else if Some(pid) == self.other {
                Some(2000)
            } else {
                None
            }
        }
    }

    const ME: Probe = Probe { other: None };

    fn write_holder(dir: &Path, pid: u32, started: u64) -> String {
        let text = serde_json::to_string(&Holder {
            process: Process {
                pid,
                started_at: format_timestamp(started),
            },
            token: "t".into(),
        })
        .unwrap();
        fs::write(dir.join(FILE_NAME), &text).unwrap();
        text
    }

    #[test]
    fn taking_a_free_lock_writes_it_and_dropping_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let lock = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
        let held: Holder =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(held.process.pid, std::process::id());
        assert_eq!(held.process.started_at, format_timestamp(1000));
        drop(lock);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_live_holder_refuses_naming_its_pid_and_start() {
        let dir = tempfile::tempdir().unwrap();
        let other = 4_000_000;
        let text = write_holder(dir.path(), other, 2000);
        let err = ProcessLock::acquire(dir.path(), "task issue-1", &Probe { other: Some(other) })
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!(
                "task issue-1 is in use by another sbxm command (process {other}, started {}); \
                 wait for it to finish, then run this command again",
                format_timestamp(2000)
            )
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            text
        );
    }

    #[test]
    fn a_lock_of_this_same_process_refuses_too() {
        let dir = tempfile::tempdir().unwrap();
        let _held = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
        let err = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap_err();
        assert!(
            err.to_string()
                .contains("is in use by another sbxm command")
        );
    }

    #[test]
    fn a_lock_of_a_gone_pid_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_001, 2000);
        let lock = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
        let held: Holder =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(held.process.pid, std::process::id());
        assert!(!dir.path().join("lock.json.takeover").exists());
        drop(lock);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_lock_of_a_reused_pid_with_another_start_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let other = 4_000_002;
        write_holder(dir.path(), other, 1500);
        let _lock = ProcessLock::acquire(dir.path(), "task issue-1", &Probe { other: Some(other) })
            .unwrap();
        let held: Holder =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(held.process.pid, std::process::id());
    }

    #[test]
    fn a_panic_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let panicked = std::panic::catch_unwind(move || {
            let _lock = ProcessLock::acquire(&path, "task issue-1", &ME).unwrap();
            panic!("the command failed");
        });
        assert!(panicked.is_err());
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_crash_without_unwinding_leaves_a_lock_the_next_run_takes_over() {
        let dir = tempfile::tempdir().unwrap();
        // The crashed run: its lock stays, and its process (another pid) is gone now.
        let crashed = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
        std::mem::forget(crashed);
        let text = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        let mut held: Holder = serde_json::from_str(&text).unwrap();
        held.process.pid = 4_000_003;
        fs::write(
            dir.path().join(FILE_NAME),
            serde_json::to_string(&held).unwrap(),
        )
        .unwrap();
        let _next = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
    }

    #[test]
    fn dropping_leaves_a_lock_that_is_no_longer_this_one() {
        let dir = tempfile::tempdir().unwrap();
        let lock = ProcessLock::acquire(dir.path(), "task issue-1", &ME).unwrap();
        let other = write_holder(dir.path(), 4_000_004, 2000);
        drop(lock);
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            other
        );
    }

    #[test]
    fn a_moved_lock_is_released_from_its_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("a"), root.path().join("b"));
        fs::create_dir(&from).unwrap();
        let mut lock = ProcessLock::acquire(&from, "task issue-1", &ME).unwrap();
        fs::rename(&from, &to).unwrap();
        lock.moved_to(&to);
        drop(lock);
        assert!(!to.join(FILE_NAME).exists());
    }

    #[test]
    fn a_takeover_left_by_a_crash_is_named_with_how_to_clear_it() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_006, 2000);
        let claim = dir.path().join("lock.json.takeover");
        fs::write(&claim, "").unwrap();

        let err = ProcessLock::acquire(dir.path(), "task issue-1", &ME)
            .unwrap_err()
            .to_string();

        assert_eq!(
            err,
            format!(
                "cannot take the stale lock {} of task issue-1: {} says another sbxm command is \
                 taking it over; if none is running, delete {} and run this command again",
                dir.path().join(FILE_NAME).display(),
                claim.display(),
                claim.display()
            )
        );
    }

    #[test]
    fn two_takers_of_one_stale_lock_never_both_win() {
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            write_holder(dir.path(), 4_000_005, 2000);
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<Result<ProcessLock>> = std::thread::scope(|s| {
                let take = || {
                    barrier.wait();
                    ProcessLock::acquire(dir.path(), "task issue-1", &ME)
                };
                let a = s.spawn(take);
                let b = s.spawn(take);
                vec![a.join().unwrap(), b.join().unwrap()]
            });
            let won = results.iter().filter(|r| r.is_ok()).count();
            assert_eq!(won, 1, "exactly one taker wins");
            let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
            assert!(
                refused
                    .to_string()
                    .contains("is in use by another sbxm command"),
                "{refused:#}"
            );
        }
    }
}
