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
    /// Ends the running child at once, if one runs: the loop's second Ctrl-C.
    fn stop(&self);
}

/// The real children: `exe`, the loop's own executable.
pub struct ChildSpawner {
    exe: PathBuf,
    running: Mutex<Option<std::process::Child>>,
}

impl ChildSpawner {
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            running: Mutex::new(None),
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
        let mut command = Command::new(&self.exe);
        command
            .args(&child.args)
            .current_dir(&child.dir)
            .envs(child.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        own_group(&mut command);
        let mut running = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.exe.display()))?;
        let stderr = running.stderr.take();
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
        let reader = std::thread::spawn(move || kept_tail(stderr));
        let status = loop {
            let polled = {
                let mut guard = self.running.lock().unwrap_or_else(|e| e.into_inner());
                match guard.as_mut() {
                    Some(running) => running.try_wait(),
                    None => break None,
                }
            };
            match polled {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(e) => {
                    self.stop();
                    return Err(e).context("cannot wait for the task's child sbxm");
                }
            }
            tick();
            std::thread::sleep(POLL);
        };
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let errors = reader.join().unwrap_or_default();
        Ok(match status {
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
        })
    }

    fn stop(&self) {
        if let Some(running) = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = running.kill();
            let _ = running.wait();
        }
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

/// The last [`KEPT`] bytes read from `stream`, as text.
fn kept_tail(stream: Option<impl Read>) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut kept: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buf[..n]);
        if kept.len() > KEPT {
            kept.drain(..kept.len() - KEPT);
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
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
