//! A task's own phases as child `sbxm` processes (issue 612, decisions 317(2), 321(2) and (4)):
//! the loop starts `task run` or `task resume` for an issue task as the loop's own executable,
//! in the loop's checkout and with its config dir, waits for it, then reads the `outcome.json`
//! it wrote. The child's output goes to the task's `run.log` (the child tees it there as any
//! `task` command does); the loop's window shows one line per event, `[<task> <slot>] <event>`.
//!
//! The child runs in a process group of its own, so the loop's first Ctrl-C doesn't reach it and
//! it finishes its phase; the second ends the loop and, through [`Spawner::stop`], the child. A
//! `task run` or `task resume` child waiting for the machine's sandbox slot polls the loop's
//! stop file ([`STOP_FILE_ENV`]), which the loop writes on its first Ctrl-C, so a stop then
//! ends the wait creating nothing (issue 570).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::disk;

/// The environment variable naming the loop's stop file to a child whose slot wait obeys it.
pub const STOP_FILE_ENV: &str = "SBXM_LOOP_STOP_FILE";

/// How often a running child is looked at.
const POLL: Duration = Duration::from_millis(200);

/// How many polls the end of a child's error output is waited for once it exited.
const READER_GRACE_POLLS: u32 = 10;

/// How much of a child's error output is kept, for a child that wrote no outcome.
const KEPT: usize = 16 * 1024;

/// One child `sbxm` command for a task's phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    /// `sbxm`'s arguments: `task run --issue 3 --local`.
    pub args: Vec<String>,
    /// Where it runs: the loop's checkout.
    pub dir: PathBuf,
    /// Its environment on top of the loop's: `SBXM_CONFIG_DIR`, and [`STOP_FILE_ENV`] for a phase
    /// whose slot wait obeys the loop's stop.
    pub env: Vec<(String, String)>,
}

/// How a child exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// Exit code 0.
    pub success: bool,
    /// How, in words: `exited with code 1`, `was killed`.
    pub how: String,
    /// The end of what it printed as errors, for one that wrote no outcome.
    pub errors: String,
}

/// Starts a task's phase as a child process, waits for it, and stops it.
pub trait Spawner {
    /// Runs `child` to its exit; `tick` is called while it runs.
    fn run(&self, child: &Child, tick: &dyn Fn()) -> Result<Exit>;
    /// Starts `child` and returns at once (issue 615): its exit is polled through the handle, so
    /// the loop supervises several at a time. A `Spawner` that only plays children runs it to its
    /// exit here, as [`Self::run`] does.
    fn start(&self, child: &Child) -> Result<Box<dyn Running + '_>> {
        Ok(Box::new(Exited(Some(self.run(child, &|| {})?))))
    }
    /// Ends every running child at once: the loop's second Ctrl-C.
    fn stop(&self);
}

/// A started child, polled for its exit.
pub trait Running {
    /// Its exit once it has exited, `None` while it runs.
    fn poll(&mut self) -> Result<Option<Exit>>;
}

/// A child that already exited: its exit, given once.
struct Exited(Option<Exit>);

impl Running for Exited {
    fn poll(&mut self) -> Result<Option<Exit>> {
        Ok(self.0.take())
    }
}

/// A process the spawner started, until it is reaped; `None` once it is, or once stopped.
type Slot = Arc<Mutex<Option<std::process::Child>>>;

/// The real children: `exe`, the loop's own executable.
pub struct ChildSpawner {
    exe: PathBuf,
    running: Mutex<Vec<Slot>>,
}

impl ChildSpawner {
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            running: Mutex::new(Vec::new()),
        }
    }

    /// Children of this executable (`std::env::current_exe`).
    pub fn current() -> Result<Self> {
        Ok(Self::new(
            std::env::current_exe().context("cannot find this executable")?,
        ))
    }
}

impl Spawner for ChildSpawner {
    fn run(&self, child: &Child, tick: &dyn Fn()) -> Result<Exit> {
        let mut running = self.start(child)?;
        loop {
            if let Some(exit) = running.poll()? {
                return Ok(exit);
            }
            tick();
            std::thread::sleep(POLL);
        }
    }

    fn start(&self, child: &Child) -> Result<Box<dyn Running + '_>> {
        let mut command = Command::new(&self.exe);
        command
            .args(&child.args)
            .current_dir(&child.dir)
            .envs(child.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        own_group(&mut command);
        let mut process = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.exe.display()))?;
        let stderr = process.stderr.take();
        let slot: Slot = Arc::new(Mutex::new(Some(process)));
        {
            let mut all = self.running.lock().unwrap_or_else(|e| e.into_inner());
            all.retain(|s| s.lock().unwrap_or_else(|e| e.into_inner()).is_some());
            all.push(Arc::clone(&slot));
        }
        // Read on a thread of its own: a process the child started may keep the pipe open after
        // the child exits, so the child's end is never waited on through it.
        let kept = Arc::new(Mutex::new(Vec::new()));
        let reader = std::thread::spawn({
            let kept = Arc::clone(&kept);
            move || keep_tail(stderr, &kept)
        });
        Ok(Box::new(Started {
            slot,
            reader: Some(reader),
            kept,
        }))
    }

    fn stop(&self) {
        let all = self.running.lock().unwrap_or_else(|e| e.into_inner());
        for slot in all.iter() {
            if let Some(mut running) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = running.kill();
                let _ = running.wait();
            }
        }
    }
}

/// A child [`ChildSpawner`] started.
struct Started {
    slot: Slot,
    reader: Option<std::thread::JoinHandle<()>>,
    kept: Arc<Mutex<Vec<u8>>>,
}

impl Running for Started {
    fn poll(&mut self) -> Result<Option<Exit>> {
        let status = {
            let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            let polled = guard.as_mut().map(|running| running.try_wait());
            match polled {
                None => None,
                Some(Ok(None)) => return Ok(None),
                Some(Ok(Some(status))) => Some(status),
                Some(Err(e)) => {
                    if let Some(mut running) = guard.take() {
                        let _ = running.kill();
                        let _ = running.wait();
                    }
                    return Err(e).context("cannot wait for the task's child sbxm");
                }
            }
        };
        // Reaped, or stopped: no longer the spawner's to stop.
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(reader) = self.reader.take() {
            for _ in 0..READER_GRACE_POLLS {
                if reader.is_finished() {
                    break;
                }
                std::thread::sleep(POLL);
            }
        }
        let errors = String::from_utf8_lossy(&self.kept.lock().unwrap_or_else(|e| e.into_inner()))
            .into_owned();
        Ok(Some(match status {
            Some(status) => Exit {
                success: status.success(),
                how: match status.code() {
                    Some(code) => format!("exited with code {code}"),
                    None => "was killed".to_owned(),
                },
                errors,
            },
            None => Exit {
                success: false,
                how: "was stopped".to_owned(),
                errors,
            },
        }))
    }
}

/// The child in a process group of its own: the loop's first Ctrl-C doesn't reach it.
#[cfg(unix)]
fn own_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// The child in a process group of its own (`CREATE_NEW_PROCESS_GROUP`), which also turns its
/// Ctrl-C off: the loop's first Ctrl-C doesn't reach it.
#[cfg(windows)]
fn own_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn own_group(_command: &mut Command) {}

/// Keeps the last [`KEPT`] bytes read from `stream` in `kept`, as they come.
fn keep_tail(stream: Option<impl Read>, kept: &Mutex<Vec<u8>>) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buf = [0u8; 4096];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.extend_from_slice(&buf[..n]);
        if kept.len() > KEPT {
            let over = kept.len() - KEPT;
            kept.drain(..over);
        }
    }
}

/// The last error a child printed (`Error: …` to the end), on one line, for a child that wrote no
/// outcome. Only quoted, never read for what it says (decision 321(2)).
pub fn last_error(errors: &str) -> Option<String> {
    let at = errors.rfind("Error: ")?;
    let text = errors[at + "Error: ".len()..]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

/// `guard`, whose slot wait also ends once `stop_file` exists: a child's, when the loop that
/// started it names its stop file in [`STOP_FILE_ENV`].
pub fn obeying_loop_stop(mut guard: disk::Guard, stop_file: Option<PathBuf>) -> disk::Guard {
    if let Some(file) = stop_file.filter(|f| !f.as_os_str().is_empty()) {
        let before = guard.slot.stop.take();
        guard.slot.stop = Some(Arc::new(move || {
            before.as_ref().is_some_and(|stop| stop()) || exists(&file)
        }));
    }
    guard
}

/// The stop file a loop named to this process, if any.
pub fn loop_stop_file() -> Option<PathBuf> {
    std::env::var_os(STOP_FILE_ENV).map(PathBuf::from)
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_error_is_quoted_on_one_line() {
        let errors = "warning: x\nError: no such issue\n\nCaused by:\n    gone\n";
        assert_eq!(
            last_error(errors).as_deref(),
            Some("no such issue Caused by: gone")
        );
        assert_eq!(last_error("warning: only\n"), None);
    }

    #[test]
    fn a_guard_obeys_the_stop_file_once_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("task-loop.stop");
        let guard = obeying_loop_stop(disk::Guard::default(), Some(file.clone()));
        let stop = guard.slot.stop.clone().unwrap();
        assert!(!stop());
        std::fs::write(&file, "").unwrap();
        assert!(stop());
        assert!(
            obeying_loop_stop(disk::Guard::default(), None)
                .slot
                .stop
                .is_none()
        );
    }
}
