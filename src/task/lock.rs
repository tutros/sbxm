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

/// How many times in all `create_new` of the lock or of its takeover claim, or the read of the
/// lock's holder, is tried while it fails with `PermissionDenied`, and how long it waits between
/// two tries (issues 407, 441): on Windows a file another command is still deleting refuses to be
/// made or opened with "Access is denied", so what a later try sees decides, and only a denial on
/// every try is reported as one.
const DENIED_ATTEMPTS: u32 = 5;
const DENIED_PAUSE: Duration = Duration::from_millis(50);

/// Making a file that must not exist yet, reading the holder's file, and waiting before trying
/// again; tests replace all three.
trait CreateFiles {
    fn create_new(&self, path: &Path) -> std::io::Result<fs::File>;
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    fn sleep(&self, pause: Duration);
}

struct RealFiles;

impl CreateFiles for RealFiles {
    fn create_new(&self, path: &Path) -> std::io::Result<fs::File> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        fs::read_to_string(path)
    }

    fn sleep(&self, pause: Duration) {
        std::thread::sleep(pause);
    }
}

/// `create_new` of `path`, tried again after a `PermissionDenied` up to [`DENIED_ATTEMPTS`] in
/// all; any other outcome, or the last try's denial, is returned as it is.
fn create_new(path: &Path, files: &dyn CreateFiles) -> std::io::Result<fs::File> {
    retry_denied(files, || files.create_new(path))
}

/// Reading the holder's file at `path`, retried like [`create_new`] (issue 441): on Windows a
/// file another command is replacing or deleting refuses to open with "Access is denied" too.
fn read_to_string(path: &Path, files: &dyn CreateFiles) -> std::io::Result<String> {
    retry_denied(files, || files.read_to_string(path))
}

/// `op`, tried again after a `PermissionDenied` up to [`DENIED_ATTEMPTS`] in all, with
/// [`DENIED_PAUSE`] between two tries.
fn retry_denied<T>(
    files: &dyn CreateFiles,
    op: impl Fn() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut attempt = 1;
    loop {
        match op() {
            Err(err) if err.kind() == ErrorKind::PermissionDenied && attempt < DENIED_ATTEMPTS => {
                files.sleep(DENIED_PAUSE);
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

/// What the lock file holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Holder {
    #[serde(flatten)]
    process: Process,
    /// Tells two locks of one process apart (two threads, or a lock taken again after its folder
    /// moved), so a lock is only ever removed by the value that made it.
    token: String,
}

/// The refusal of a lock whose holder is alive, typed so a caller that would rather skip the
/// locked thing than fail (`task loop`, issue 409) can tell it from any other error.
#[derive(Debug)]
pub struct InUse {
    what: String,
    holder: Process,
}

impl InUse {
    /// What is locked and who holds it, without the advice to run the command again.
    pub fn describe(&self) -> String {
        format!(
            "{} is in use by another sbxm command (process {}, started {})",
            self.what, self.holder.pid, self.holder.started_at
        )
    }
}

impl std::fmt::Display for InUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; wait for it to finish, then run this command again",
            self.describe()
        )
    }
}

impl std::error::Error for InUse {}

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

pub(crate) fn token() -> String {
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
        Self::acquire_with(dir, what, probe, &RealFiles)
    }

    fn acquire_with(
        dir: &Path,
        what: &str,
        probe: &dyn ProcessProbe,
        files: &dyn CreateFiles,
    ) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let me = serde_json::to_string(&Holder {
            process: Process::current(probe),
            token: token(),
        })?;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(PAUSE);
            }
            match create_new(&path, files) {
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
            let text = match read_to_string(&path, files) {
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
                return Err(InUse {
                    what: what.to_owned(),
                    holder: held.process,
                }
                .into());
            }
            take_over_stale(&path, &text, files)?;
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

/// Whether `<dir>/lock.json` names a process that is still running (issue 579): a resume takes
/// the lock before it records its own process in `task.json`, so a task whose recorded process
/// is gone is still being worked on while this is true. A missing or unreadable lock is `false`.
pub fn held_alive(dir: &Path, probe: &dyn ProcessProbe) -> bool {
    fs::read_to_string(dir.join(FILE_NAME))
        .ok()
        .and_then(|text| serde_json::from_str::<Holder>(&text).ok())
        .is_some_and(|held| held.process.alive(probe))
}

/// [`create_new`] of the real file system, for the sandbox slots (decision 304).
pub(crate) fn create_new_file(path: &Path) -> std::io::Result<fs::File> {
    create_new(path, &RealFiles)
}

/// [`read_to_string`] of the real file system, for the sandbox slots (decision 304).
pub(crate) fn read_file(path: &Path) -> std::io::Result<String> {
    read_to_string(path, &RealFiles)
}

/// [`take_over_stale`] of the real file system, for the sandbox slots (decision 304).
pub(crate) fn take_over_stale_file(path: &Path, stale: &str) -> Result<()> {
    take_over_stale(path, stale, &RealFiles)
}

/// The takeover claim next to the lock at `path`: `lock.json.takeover`.
fn takeover_path(path: &Path) -> PathBuf {
    path.with_extension("json.takeover")
}

/// Removes the stale lock at `path` if it still reads `stale`, under the takeover claim; a claim
/// held by another command leaves it to that command.
fn take_over_stale(path: &Path, stale: &str, files: &dyn CreateFiles) -> Result<()> {
    let claim = takeover_path(path);
    match create_new(&claim, files) {
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
    fn a_live_holders_refusal_is_typed_so_a_caller_can_skip_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let other = 4_000_007;
        write_holder(dir.path(), other, 2000);
        let err = ProcessLock::acquire(dir.path(), "task issue-1", &Probe { other: Some(other) })
            .unwrap_err();
        assert!(err.is::<InUse>(), "{err:#}");
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

    /// Real files, except that the first `denials` creations of the file called `name` fail with
    /// `PermissionDenied`, each after running `on_deny` (what a racing command did meanwhile);
    /// pauses are recorded instead of slept.
    struct Denying<'a> {
        name: &'static str,
        denials: u32,
        on_deny: &'a dyn Fn(),
        calls: std::cell::Cell<u32>,
        pauses: std::cell::RefCell<Vec<Duration>>,
    }

    impl<'a> Denying<'a> {
        fn new(name: &'static str, denials: u32, on_deny: &'a dyn Fn()) -> Self {
            Self {
                name,
                denials,
                on_deny,
                calls: std::cell::Cell::new(0),
                pauses: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl CreateFiles for Denying<'_> {
        fn create_new(&self, path: &Path) -> std::io::Result<fs::File> {
            if path.file_name().is_some_and(|n| n == self.name) {
                let call = self.calls.get() + 1;
                self.calls.set(call);
                if call <= self.denials {
                    (self.on_deny)();
                    return Err(ErrorKind::PermissionDenied.into());
                }
            }
            RealFiles.create_new(path)
        }

        fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            RealFiles.read_to_string(path)
        }

        fn sleep(&self, pause: Duration) {
            self.pauses.borrow_mut().push(pause);
        }
    }

    const CLAIM: &str = "lock.json.takeover";

    fn holder_pid(dir: &Path) -> u32 {
        let held: Holder =
            serde_json::from_str(&fs::read_to_string(dir.join(FILE_NAME)).unwrap()).unwrap();
        held.process.pid
    }

    #[test]
    fn a_denied_lock_that_then_exists_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let other = 4_000_010;
        let race = || {
            write_holder(dir.path(), other, 2000);
        };
        let files = Denying::new(FILE_NAME, 1, &race);
        let err = ProcessLock::acquire_with(
            dir.path(),
            "task issue-1",
            &Probe { other: Some(other) },
            &files,
        )
        .unwrap_err();
        assert!(err.is::<InUse>(), "{err:#}");
        assert_eq!(files.calls.get(), 2);
        assert_eq!(*files.pauses.borrow(), vec![DENIED_PAUSE]);
    }

    #[test]
    fn a_denied_takeover_claim_that_then_exists_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_011, 2000);
        let other = 4_000_012;
        // The other taker claimed the takeover and took the lock meanwhile.
        let race = || {
            fs::write(dir.path().join(CLAIM), "").unwrap();
            write_holder(dir.path(), other, 2000);
        };
        let files = Denying::new(CLAIM, 1, &race);
        let err = ProcessLock::acquire_with(
            dir.path(),
            "task issue-1",
            &Probe { other: Some(other) },
            &files,
        )
        .unwrap_err();
        assert!(err.is::<InUse>(), "{err:#}");
        assert_eq!(files.calls.get(), 2);
    }

    #[test]
    fn a_lock_denied_once_then_created_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let files = Denying::new(FILE_NAME, 1, &|| {});
        let lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.calls.get(), 2);
        drop(lock);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_takeover_claim_denied_once_then_created_takes_the_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_013, 2000);
        let files = Denying::new(CLAIM, 1, &|| {});
        let _lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.calls.get(), 2);
        assert!(!dir.path().join(CLAIM).exists());
    }

    #[test]
    fn a_lock_denied_four_times_then_created_on_the_last_attempt_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let files = Denying::new(FILE_NAME, DENIED_ATTEMPTS - 1, &|| {});
        let _lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.calls.get(), DENIED_ATTEMPTS);
        assert_eq!(
            *files.pauses.borrow(),
            vec![DENIED_PAUSE; DENIED_ATTEMPTS as usize - 1]
        );
    }

    #[test]
    fn a_takeover_claim_denied_four_times_then_created_takes_the_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_014, 2000);
        let files = Denying::new(CLAIM, DENIED_ATTEMPTS - 1, &|| {});
        let _lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.calls.get(), DENIED_ATTEMPTS);
    }

    #[test]
    fn a_lock_denied_on_every_attempt_reports_the_real_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let files = Denying::new(FILE_NAME, u32::MAX, &|| {});
        let err = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap_err();
        assert!(!err.is::<InUse>(), "{err:#}");
        assert_eq!(
            err.to_string(),
            format!("cannot create {}", dir.path().join(FILE_NAME).display())
        );
        assert_eq!(
            err.root_cause().to_string(),
            std::io::Error::from(ErrorKind::PermissionDenied).to_string()
        );
        assert_eq!(files.calls.get(), DENIED_ATTEMPTS);
        assert_eq!(
            *files.pauses.borrow(),
            vec![DENIED_PAUSE; DENIED_ATTEMPTS as usize - 1]
        );
    }

    #[test]
    fn a_takeover_claim_denied_on_every_attempt_reports_the_real_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let stale = write_holder(dir.path(), 4_000_015, 2000);
        let files = Denying::new(CLAIM, u32::MAX, &|| {});
        let err = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap_err();
        assert!(!err.is::<InUse>(), "{err:#}");
        assert_eq!(
            err.to_string(),
            format!("cannot create {}", dir.path().join(CLAIM).display())
        );
        assert_eq!(files.calls.get(), DENIED_ATTEMPTS);
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            stale
        );
    }

    // Issue 441: the read of the holder's file, retried after `PermissionDenied` like the create.

    /// Real files, except that the first `denials` reads of the lock file fail with `kind`, each
    /// after running `on_deny` (what a racing command did meanwhile); pauses are recorded instead
    /// of slept.
    struct DenyingReads<'a> {
        kind: ErrorKind,
        denials: u32,
        on_deny: &'a dyn Fn(),
        reads: std::cell::Cell<u32>,
        pauses: std::cell::RefCell<Vec<Duration>>,
    }

    impl<'a> DenyingReads<'a> {
        fn new(kind: ErrorKind, denials: u32, on_deny: &'a dyn Fn()) -> Self {
            Self {
                kind,
                denials,
                on_deny,
                reads: std::cell::Cell::new(0),
                pauses: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl CreateFiles for DenyingReads<'_> {
        fn create_new(&self, path: &Path) -> std::io::Result<fs::File> {
            RealFiles.create_new(path)
        }

        fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            let read = self.reads.get() + 1;
            self.reads.set(read);
            if read <= self.denials {
                (self.on_deny)();
                return Err(self.kind.into());
            }
            RealFiles.read_to_string(path)
        }

        fn sleep(&self, pause: Duration) {
            self.pauses.borrow_mut().push(pause);
        }
    }

    #[test]
    fn a_read_denied_four_times_then_naming_a_live_holder_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let other = 4_000_020;
        let text = write_holder(dir.path(), other, 2000);
        let files = DenyingReads::new(ErrorKind::PermissionDenied, DENIED_ATTEMPTS - 1, &|| {});
        let err = ProcessLock::acquire_with(
            dir.path(),
            "task issue-1",
            &Probe { other: Some(other) },
            &files,
        )
        .unwrap_err();
        assert!(err.is::<InUse>(), "{err:#}");
        assert_eq!(files.reads.get(), DENIED_ATTEMPTS);
        assert_eq!(
            *files.pauses.borrow(),
            vec![DENIED_PAUSE; DENIED_ATTEMPTS as usize - 1]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            text
        );
    }

    #[test]
    fn a_read_denied_four_times_then_naming_a_stale_holder_takes_it_over() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_021, 2000);
        let files = DenyingReads::new(ErrorKind::PermissionDenied, DENIED_ATTEMPTS - 1, &|| {});
        let _lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.reads.get(), DENIED_ATTEMPTS);
        assert_eq!(
            *files.pauses.borrow(),
            vec![DENIED_PAUSE; DENIED_ATTEMPTS as usize - 1]
        );
        assert!(!dir.path().join(CLAIM).exists());
    }

    #[test]
    fn a_read_denied_once_then_gone_takes_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        write_holder(dir.path(), 4_000_022, 2000);
        // The other command released the lock meanwhile.
        let race = || fs::remove_file(dir.path().join(FILE_NAME)).unwrap();
        let files = DenyingReads::new(ErrorKind::PermissionDenied, 1, &race);
        let lock = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap();
        assert_eq!(holder_pid(dir.path()), std::process::id());
        assert_eq!(files.reads.get(), 2);
        assert_eq!(*files.pauses.borrow(), vec![DENIED_PAUSE]);
        drop(lock);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_read_denied_on_every_attempt_reports_the_real_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let stale = write_holder(dir.path(), 4_000_023, 2000);
        let files = DenyingReads::new(ErrorKind::PermissionDenied, u32::MAX, &|| {});
        let err = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap_err();
        assert!(!err.is::<InUse>(), "{err:#}");
        assert_eq!(
            err.to_string(),
            format!("cannot read {}", dir.path().join(FILE_NAME).display())
        );
        assert_eq!(
            err.root_cause().to_string(),
            std::io::Error::from(ErrorKind::PermissionDenied).to_string()
        );
        assert_eq!(files.reads.get(), DENIED_ATTEMPTS);
        assert_eq!(
            *files.pauses.borrow(),
            vec![DENIED_PAUSE; DENIED_ATTEMPTS as usize - 1]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            stale
        );
    }

    #[test]
    fn a_read_failing_otherwise_is_reported_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let stale = write_holder(dir.path(), 4_000_024, 2000);
        let files = DenyingReads::new(ErrorKind::InvalidData, 1, &|| {});
        let err = ProcessLock::acquire_with(dir.path(), "task issue-1", &ME, &files).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("cannot read {}", dir.path().join(FILE_NAME).display())
        );
        assert_eq!(
            err.root_cause().to_string(),
            std::io::Error::from(ErrorKind::InvalidData).to_string()
        );
        assert_eq!(files.reads.get(), 1);
        assert!(files.pauses.borrow().is_empty());
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            stale
        );
    }
}
