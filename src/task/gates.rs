//! Gates (spec §7, decision 160): commands that decide whether a task's work may go on. They run
//! as a build step by sbxm, never left to the agent's prompt. Two tiers: the *sandbox* tier in the
//! worker's own sandbox, and an optional *host* tier on this machine, each stopping at the first
//! failing command.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::record::GateResult;
use crate::backend::SandboxBackend;
use crate::run::checks::run_checks;
use crate::run::config::Check;

/// How much of a command's output is kept in `gates.log`.
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// How one gate command went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub result: GateResult,
    /// The end of the command's output, for `gates.log`.
    pub output_tail: String,
    pub timed_out: bool,
}

/// Runs `commands` in `sandbox` under `workdir`, as `sh -c` inside the in-sandbox timeout (the
/// same wrapper as `sbxm run`'s checks, so a stuck command can't outlive its limit). Stops after
/// the first command that fails, times out or can't be run.
pub fn run_sandbox_tier(
    backend: &dyn SandboxBackend,
    sandbox: &str,
    workdir: &Path,
    phase: &str,
    commands: &[String],
    timeout: Duration,
) -> Vec<GateOutcome> {
    let mut outcomes = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        let check = Check {
            id: format!("gate-{i}"),
            command: command.clone(),
            timeout: Some(timeout),
        };
        let verdict = run_checks(backend, sandbox, workdir, &[check], timeout).remove(0);
        let output_tail = match &verdict.error {
            Some(error) => format!("could not run the command: {error}"),
            None => verdict.output_tail.clone(),
        };
        let passed = verdict.passed;
        outcomes.push(GateOutcome {
            result: GateResult {
                phase: phase.to_owned(),
                tier: "sandbox".to_owned(),
                command: command.clone(),
                exit: verdict.exit_code,
                passed,
            },
            output_tail,
            timed_out: verdict.timed_out,
        });
        if !passed {
            break;
        }
    }
    outcomes
}

/// What a command run on this machine left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOutput {
    pub stdout: String,
    pub stderr: String,
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Runs a gate command on this machine (spec §7). A trait so tests never run agent-shaped
/// commands on the host.
pub trait HostRunner: Send + Sync {
    /// `Err` means the command could not be started at all.
    fn run(&self, cwd: &Path, command: &str, timeout: Duration) -> Result<HostOutput>;
}

/// Runs `commands` on this machine in `cwd`, which must be a clean checkout (never the agent's
/// own folder). Stops after the first command that fails, times out or can't be started.
pub fn run_host_tier(
    runner: &dyn HostRunner,
    cwd: &Path,
    phase: &str,
    commands: &[String],
    timeout: Duration,
) -> Vec<GateOutcome> {
    let mut outcomes = Vec::new();
    for command in commands {
        let (exit, passed, timed_out, output_tail) = match runner.run(cwd, command, timeout) {
            Ok(out) => (
                out.exit_code,
                out.exit_code == Some(0) && !out.timed_out,
                out.timed_out,
                tail(&format!("{}{}", out.stdout, out.stderr), OUTPUT_TAIL_BYTES),
            ),
            Err(e) => (
                None,
                false,
                false,
                format!("could not run the command: {e:#}"),
            ),
        };
        outcomes.push(GateOutcome {
            result: GateResult {
                phase: phase.to_owned(),
                tier: "host".to_owned(),
                command: command.clone(),
                exit,
                passed,
            },
            output_tail,
            timed_out,
        });
        if !passed {
            break;
        }
    }
    outcomes
}

/// The last `max` bytes of `text` (rounded up to a character boundary).
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// The platform shell: `cmd` on Windows, `sh` elsewhere. Host gates inherit the user's
/// environment but not `GIT_*` variables, and a command that outlives its timeout is killed
/// together with what it started.
pub struct ShellHostRunner;

fn host_command(
    cwd: &Path,
    command: &str,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        // `/S` strips the outer quotes, so the command reaches cmd as written.
        cmd.raw_arg("/D /S /C").raw_arg(format!("\"{command}\""));
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.env_clear();
    cmd.envs(
        environment
            .into_iter()
            .filter(|(name, _)| !is_git_variable(name)),
    );
    cmd
}

fn is_git_variable(name: &OsStr) -> bool {
    name.to_string_lossy()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIT_"))
}

/// Collects a pipe on its own thread, keeping about the last `OUTPUT_TAIL_BYTES * 2` bytes.
fn drain(mut pipe: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<String> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut kept: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&chunk[..n]);
            if kept.len() > OUTPUT_TAIL_BYTES * 4 {
                kept.drain(..kept.len() - OUTPUT_TAIL_BYTES * 2);
            }
        }
        let _ = send.send(String::from_utf8_lossy(&kept).into_owned());
    });
    receive
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

impl HostRunner for ShellHostRunner {
    fn run(&self, cwd: &Path, command: &str, timeout: Duration) -> Result<HostOutput> {
        let mut child = host_command(cwd, command, std::env::vars_os())
            .spawn()
            .with_context(|| format!("cannot start the shell in {}", cwd.display()))?;
        let stdout = child.stdout.take().map(drain);
        let stderr = child.stderr.take().map(drain);
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                kill_tree(&mut child);
                break child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let collect = |pipe: Option<std::sync::mpsc::Receiver<String>>| {
            pipe.and_then(|rx| rx.recv_timeout(Duration::from_secs(3)).ok())
                .unwrap_or_default()
        };
        Ok(HostOutput {
            stdout: collect(stdout),
            stderr: collect(stderr),
            exit_code: status.code(),
            timed_out,
        })
    }
}

/// One call a [`FakeHostRunner`] received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCall {
    pub cwd: PathBuf,
    pub command: String,
    pub timeout: Duration,
}

#[derive(Clone)]
enum Scripted {
    Exit(i32, String, String),
    Timeout,
    StartError(String),
}

type HostHook = dyn Fn(&Path, &str) + Send + Sync;

/// Records every command and answers from what a test scripted (exit 0 otherwise): no command
/// ever runs.
#[derive(Default)]
pub struct FakeHostRunner {
    calls: Mutex<Vec<HostCall>>,
    rules: Vec<(String, Scripted)>,
    hook: Option<Arc<HostHook>>,
}

impl FakeHostRunner {
    /// Commands containing `needle` exit with `code`.
    pub fn with_exit(mut self, needle: &str, code: i32, stdout: &str, stderr: &str) -> Self {
        self.rules.push((
            needle.to_owned(),
            Scripted::Exit(code, stdout.to_owned(), stderr.to_owned()),
        ));
        self
    }

    /// Commands containing `needle` hit their timeout.
    pub fn with_timeout(mut self, needle: &str) -> Self {
        self.rules.push((needle.to_owned(), Scripted::Timeout));
        self
    }

    /// Commands containing `needle` cannot be started.
    pub fn with_start_error(mut self, needle: &str, message: &str) -> Self {
        self.rules
            .push((needle.to_owned(), Scripted::StartError(message.to_owned())));
        self
    }

    /// Calls `hook(cwd, command)` on every run, e.g. to look at the checkout while a gate "runs".
    pub fn with_hook(mut self, hook: impl Fn(&Path, &str) + Send + Sync + 'static) -> Self {
        self.hook = Some(Arc::new(hook));
        self
    }

    pub fn calls(&self) -> Vec<HostCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl HostRunner for FakeHostRunner {
    fn run(&self, cwd: &Path, command: &str, timeout: Duration) -> Result<HostOutput> {
        self.calls.lock().unwrap().push(HostCall {
            cwd: cwd.to_path_buf(),
            command: command.to_owned(),
            timeout,
        });
        if let Some(hook) = &self.hook {
            hook(cwd, command);
        }
        let rule = self
            .rules
            .iter()
            .find(|(needle, _)| command.contains(needle.as_str()));
        match rule.map(|(_, scripted)| scripted) {
            Some(Scripted::Exit(code, stdout, stderr)) => Ok(HostOutput {
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                exit_code: Some(*code),
                timed_out: false,
            }),
            Some(Scripted::Timeout) => Ok(HostOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                timed_out: true,
            }),
            Some(Scripted::StartError(message)) => Err(anyhow::anyhow!("{message}")),
            None => Ok(HostOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                timed_out: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_commands_never_inherit_git_variables() {
        let inherited = [
            ("PATH", "host-path"),
            ("GIT_DIR", "/elsewhere"),
            ("GIT_CONFIG_COUNT", "1"),
            ("git_lower", "x"),
            ("GITHUB_TOKEN_NAME", "kept: only GIT_ is dropped"),
        ]
        .map(|(n, v)| (OsString::from(n), OsString::from(v)));

        let cmd = host_command(Path::new("."), "echo", inherited);

        let names: Vec<String> = cmd
            .get_envs()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"PATH".to_owned()));
        assert!(names.contains(&"GITHUB_TOKEN_NAME".to_owned()));
        for dropped in ["GIT_DIR", "GIT_CONFIG_COUNT", "git_lower"] {
            assert!(
                !names.contains(&dropped.to_owned()),
                "{dropped} leaked: {names:?}"
            );
        }
    }
}
