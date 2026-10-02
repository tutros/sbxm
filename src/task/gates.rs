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

/// Put in front of every sandbox gate command: `sbx exec` reads no login profile, so a toolchain
/// the profile installs under `~/.cargo` (Rust, `just`) isn't on the PATH without it.
const CARGO_ENV: &str = "[ -f \"$HOME/.cargo/env\" ] && . \"$HOME/.cargo/env\"; ";

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
            command: format!("{CARGO_ENV}{command}"),
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
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        // Its own process group, so a timeout can stop the shell and everything it started.
        cmd.process_group(0);
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
    // cmd.exe looks in the current directory before `PATH`; the checkout holds agent files, so a
    // committed `cargo.cmd` would otherwise stand in for the real tool.
    #[cfg(windows)]
    cmd.env("NoDefaultCurrentDirectoryInExePath", "1");
    cmd
}

fn is_git_variable(name: &OsStr) -> bool {
    name.to_string_lossy()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIT_"))
}

/// A pipe collected on its own thread, keeping about the last `OUTPUT_TAIL_BYTES * 2` bytes.
/// What has arrived stays readable even if something that outlived the command keeps the pipe
/// open, so a timeout doesn't lose the output that explains it.
struct Drained {
    kept: Arc<Mutex<Vec<u8>>>,
    eof: std::sync::mpsc::Receiver<()>,
}

impl Drained {
    /// Waits up to `wait` for the pipe to close, then returns what was read so far.
    fn collect(self, wait: Duration) -> String {
        let _ = self.eof.recv_timeout(wait);
        let kept = self.kept.lock().map(|k| k.clone()).unwrap_or_default();
        String::from_utf8_lossy(&kept).into_owned()
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> Drained {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (send, eof) = std::sync::mpsc::channel();
    let shared = Arc::clone(&kept);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut kept) = shared.lock() {
                kept.extend_from_slice(&chunk[..n]);
                if kept.len() > OUTPUT_TAIL_BYTES * 4 {
                    let excess = kept.len() - OUTPUT_TAIL_BYTES * 2;
                    kept.drain(..excess);
                }
            }
        }
        let _ = send.send(());
    });
    Drained { kept, eof }
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
    #[cfg(unix)]
    {
        // The shell leads its own group (see `host_command`): `-pid` is the whole group.
        // The external `kill`: the shell builtin of `dash` rejects `--` before a negative id.
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
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
        let collect = |pipe: Option<Drained>| {
            pipe.map(|drained| drained.collect(Duration::from_secs(3)))
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

    /// cmd.exe searches the current directory before `PATH` unless told not to, so a committed
    /// `hello.cmd` would run when a gate says just `hello` (or `cargo`, with a `cargo.cmd`).
    /// The environment given here lacks the variable, whatever this machine has set.
    #[cfg(windows)]
    #[test]
    fn a_program_in_the_checkout_does_not_take_the_place_of_a_tool_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.cmd"), "@echo HIJACKED\r\n").unwrap();
        let environment = std::env::vars_os()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("NoDefaultCurrentDirectoryInExePath"));

        let out = host_command(dir.path(), "hello", environment)
            .output()
            .unwrap();

        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("HIJACKED"),
            "{out:?}"
        );
        assert!(!out.status.success(), "{out:?}");
    }

    /// Whether process `pid` is alive; kills it if so, so a failing test leaves nothing behind.
    #[cfg(unix)]
    fn alive_then_killed(pid: &str) -> bool {
        let alive = Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if alive {
            let _ = Command::new("kill").args(["-KILL", pid]).status();
        }
        alive
    }

    #[cfg(unix)]
    #[test]
    fn a_timed_out_command_takes_its_children_with_it() {
        let dir = tempfile::tempdir().unwrap();

        let out = ShellHostRunner
            .run(
                dir.path(),
                "sleep 30 & echo $! > child.pid; echo before; wait",
                Duration::from_millis(700),
            )
            .unwrap();

        assert!(out.timed_out);
        let pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !alive_then_killed(pid.trim()),
            "the child outlived the timeout; agent code kept running on the host"
        );
        assert!(out.stdout.contains("before"), "{out:?}");
    }

    #[cfg(unix)]
    #[test]
    fn output_before_a_timeout_is_kept_when_a_descendant_escapes_and_holds_the_pipe() {
        let dir = tempfile::tempdir().unwrap();

        // `setsid` puts the sleeper in a session of its own, out of reach of a group kill, and it
        // still holds this command's stdout.
        let out = ShellHostRunner
            .run(
                dir.path(),
                "echo before; setsid sleep 30 & echo $! > child.pid; wait",
                Duration::from_millis(700),
            )
            .unwrap();

        let pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap();
        alive_then_killed(pid.trim());
        assert!(out.timed_out);
        assert!(
            out.stdout.contains("before"),
            "output seen before the timeout must not be lost: {out:?}"
        );
    }

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
