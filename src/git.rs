//! Host-side `git` calls on workspaces that an agent controls (M2a slice 7).
//!
//! A contestant can write anything into its workspace, including a `.git/config`
//! with `core.fsmonitor`, `diff.external`, filter or textconv commands, and
//! hooks. So sbxm never lets git read a repository inside a workspace: the
//! repository (`--git-dir`) is always a folder sbxm made outside it, the
//! workspace is only ever a `--work-tree`, and the user's global and system
//! git config and inherited `GIT_*` variables are switched off so results
//! don't depend on the machine either.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Same author and date for every baseline commit, so the host's copy and the
/// agent-visible repo get the same commit id.
const BASELINE_DATE: &str = "2000-01-01T00:00:00+0000";

/// How often a git call that Windows refused with "Permission denied" is tried,
/// and how long to wait before the second try (it doubles after each failure).
const TRIES: u32 = 5;
const FIRST_DELAY: Duration = Duration::from_millis(50);

/// Runs `attempt` until it succeeds, fails for any reason but git being refused
/// access to a file, or `tries` is used up; then returns the last result.
///
/// On Windows another process (antivirus, the file indexer) can briefly hold a
/// file git just created, and git fails with "Permission denied" (exit 128, or 1 from `git fetch`) when it
/// replaces that file. It goes away on its own, so waiting and trying again is
/// right; a seeded contestant would otherwise fail with "cannot seed workspace"
/// (issue #39). Every call sbxm makes is safe to repeat after that failure,
/// because git changed nothing when it couldn't write.
fn retry(
    tries: u32,
    first_delay: Duration,
    mut attempt: impl FnMut() -> Result<Output>,
) -> Result<Output> {
    let mut delay = first_delay;
    for _ in 1..tries {
        let out = attempt()?;
        if out.status.success() || !permission_denied(&out) {
            return Ok(out);
        }
        std::thread::sleep(delay);
        delay *= 2;
    }
    attempt()
}

/// Git failed because it was refused access to a file. The exit code varies: 128 when git dies
/// itself, 1 when `git fetch` reports that `index-pack` could not move a pack into place.
fn permission_denied(out: &Output) -> bool {
    !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("Permission denied")
}

/// `git <args>` with a hardened environment. `git_dir` and `work_tree` are
/// passed as `--git-dir`/`--work-tree` when given; `cwd` is where it runs.
/// Retries when git is refused access to a file (see [`retry`]).
pub(crate) fn output(
    cwd: &Path,
    git_dir: Option<&Path>,
    work_tree: Option<&Path>,
    args: &[&str],
) -> Result<Output> {
    retry(TRIES, FIRST_DELAY, || {
        output_once(cwd, git_dir, work_tree, args)
    })
}

fn output_once(
    cwd: &Path,
    git_dir: Option<&Path>,
    work_tree: Option<&Path>,
    args: &[&str],
) -> Result<Output> {
    command(cwd, git_dir, work_tree, args, std::env::vars_os())
        .output()
        .context("cannot run `git`; is Git installed and on PATH?")
}

fn command(
    cwd: &Path,
    git_dir: Option<&Path>,
    work_tree: Option<&Path>,
    args: &[&str],
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    if let Some(dir) = git_dir {
        cmd.arg("--git-dir").arg(dir);
    }
    if let Some(tree) = work_tree {
        cmd.arg("--work-tree").arg(tree);
    }
    cmd.args([
        "-c",
        "core.autocrlf=false",
        "-c",
        "core.safecrlf=false",
        "-c",
        "core.quotepath=false",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "protocol.allow=never",
    ]);
    cmd.args(args);
    // Nothing inherited may point git somewhere else or make it run code.
    cmd.env_clear();
    cmd.envs(
        environment
            .into_iter()
            .filter(|(name, _)| !is_git_variable(name)),
    );
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_AUTHOR_NAME", "sbxm")
        .env("GIT_AUTHOR_EMAIL", "sbxm@localhost")
        .env("GIT_AUTHOR_DATE", BASELINE_DATE)
        .env("GIT_COMMITTER_NAME", "sbxm")
        .env("GIT_COMMITTER_EMAIL", "sbxm@localhost")
        .env("GIT_COMMITTER_DATE", BASELINE_DATE);
    cmd
}

fn is_git_variable(name: &OsStr) -> bool {
    name.to_string_lossy()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIT_"))
}

/// Like [`output`], but a non-zero exit is an error carrying git's message,
/// and the result is stdout as text.
pub(crate) fn run(
    cwd: &Path,
    git_dir: Option<&Path>,
    work_tree: Option<&Path>,
    args: &[&str],
) -> Result<String> {
    let out = output(cwd, git_dir, work_tree, args)?;
    if !out.status.success() {
        bail!(
            "`git {}` failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The `GIT_*` variables that keep working for [`user_output`]: where the user's own
/// config and credentials come from. Every other one (`GIT_DIR`, `GIT_CONFIG_COUNT`, ...)
/// could point git elsewhere or make it run code, so it is dropped.
const USER_GIT_VARIABLES: [&str; 7] = [
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_SSL_CAINFO",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_EXEC_PATH",
];

/// `git <args>` for the repo sbxm owns (`repo.git`) and for talking to GitHub, never for a
/// repository an agent controls: the user's git config and credentials apply, because
/// cloning, fetching and pushing need them (spec §6). `extra_env` is added last.
pub(crate) fn user_output(
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &OsStr)],
) -> Result<Output> {
    retry(TRIES, FIRST_DELAY, || {
        let mut cmd = user_command(cwd, args, extra_env, std::env::vars_os());
        output_with_timeout(&mut cmd, USER_GIT_TIMEOUT)
    })
}

/// How long one network git call may take before sbxm gives up on it.
const USER_GIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Variables that make a credential prompt appear (an editor's askpass helper, say). sbxm runs
/// unattended, so a prompt would hang it: they are dropped and the user's credential helper
/// answers or fails (a VS Code askpass held `task start` for 10+ minutes).
fn is_askpass_variable(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    name == "GIT_ASKPASS"
        || name == "SSH_ASKPASS"
        || name == "SSH_ASKPASS_REQUIRE"
        || name.starts_with("VSCODE_GIT_")
}

fn user_command(
    cwd: &Path,
    args: &[&str],
    extra_env: &[(&str, &OsStr)],
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    cmd.env_clear();
    cmd.envs(environment.into_iter().filter(|(name, _)| {
        !is_askpass_variable(name)
            && (!is_git_variable(name)
                || USER_GIT_VARIABLES
                    .iter()
                    .any(|allowed| name.to_string_lossy().eq_ignore_ascii_case(allowed)))
    }));
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    for (name, value) in extra_env {
        cmd.env(name, value);
    }
    cmd
}

/// Runs `cmd` to the end like `Command::output`, but kills it and fails after `limit`.
fn output_with_timeout(cmd: &mut Command, limit: Duration) -> Result<Output> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    }

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run `git`; is Git installed and on PATH?")?;
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("cannot wait for the command")? {
            break status;
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "the command did not finish within {} s and was stopped; if git was waiting for \
                 a login, run `gh auth setup-git` and try again",
                limit.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Like [`user_output`], but a non-zero exit is an error carrying git's message.
pub(crate) fn user_run(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = user_output(cwd, args, &[])?;
    if !out.status.success() {
        bail!(
            "`git {}` failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Stages everything in `work_tree` (respecting its ignore files) and commits
/// it as the single baseline; returns the commit id.
pub(crate) fn baseline_commit(cwd: &Path, git_dir: &Path, work_tree: &Path) -> Result<String> {
    run(cwd, Some(git_dir), Some(work_tree), &["add", "-A"])?;
    run(
        cwd,
        Some(git_dir),
        Some(work_tree),
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-verify",
            "--no-gpg-sign",
            "-m",
            "baseline",
        ],
    )?;
    let id = run(cwd, Some(git_dir), Some(work_tree), &["rev-parse", "HEAD"])?;
    Ok(id.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::time::Duration;

    fn exit(code: i32) -> std::process::ExitStatus {
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        }
    }

    fn failed(code: i32, stderr: &str) -> Output {
        Output {
            status: exit(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    const DENIED: &str = "error: could not write config file .git/config: Permission denied\nfatal: could not set 'core.bare' to 'false'";

    #[test]
    fn a_permission_denied_failure_is_retried_until_it_works() {
        let calls = Cell::new(0);

        let out = retry(3, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Ok(if calls.get() < 3 {
                failed(128, DENIED)
            } else {
                failed(0, "")
            })
        })
        .unwrap();

        assert!(out.status.success());
        assert_eq!(calls.get(), 3);
    }

    /// `git fetch` reports a pack it could not move into place with exit code 1, not 128 (seen
    /// in `task start` tests under load, issue #104).
    const PACK_DENIED: &str = "error: unable to write file repo.git/objects/pack/pack-1.pack: Permission denied\nfatal: unable to rename temporary '*.pack' file to 'repo.git/objects/pack/pack-1.pack'\nerror: index-pack died";

    #[test]
    fn a_permission_denied_failure_is_retried_whatever_gits_exit_code() {
        let calls = Cell::new(0);

        let out = retry(3, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Ok(if calls.get() < 3 {
                failed(1, PACK_DENIED)
            } else {
                failed(0, "")
            })
        })
        .unwrap();

        assert!(out.status.success());
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn it_gives_up_after_the_last_try_and_returns_gits_own_failure() {
        let calls = Cell::new(0);

        let out = retry(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Ok(failed(128, DENIED))
        })
        .unwrap();

        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("Permission denied"));
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn other_failures_are_not_retried() {
        let calls = Cell::new(0);

        let out = retry(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Ok(failed(128, "fatal: not a git repository"))
        })
        .unwrap();

        assert!(!out.status.success());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_failure_to_run_git_at_all_is_not_retried() {
        let calls = Cell::new(0);

        let err = retry(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err(anyhow::anyhow!("cannot run `git`"))
        })
        .unwrap_err();

        assert!(err.to_string().contains("cannot run"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn success_is_returned_at_once() {
        let calls = Cell::new(0);

        retry(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Ok(failed(0, ""))
        })
        .unwrap();

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn built_command_drops_all_inherited_git_variables() {
        let inherited = [
            ("PATH", "host-path"),
            ("SystemRoot", "host-system-root"),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "filter.probe.clean"),
            ("GIT_CONFIG_VALUE_0", "host-command"),
            ("GIT_CONFIG_PARAMETERS", "'filter.probe.clean=host-command'"),
            ("GIT_ANOTHER_FUTURE_VARIABLE", "unsafe"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));

        let cmd = command(Path::new("."), None, None, &["version"], inherited);
        let environment: Vec<_> = cmd.get_envs().collect();
        let git_names: BTreeSet<_> = environment
            .iter()
            .filter(|(name, _)| is_git_variable(name))
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();

        assert!(environment.contains(&(OsStr::new("PATH"), Some(OsStr::new("host-path")))));
        assert!(environment.contains(&(
            OsStr::new("SystemRoot"),
            Some(OsStr::new("host-system-root"))
        )));
        assert_eq!(
            git_names,
            [
                "GIT_AUTHOR_DATE",
                "GIT_AUTHOR_EMAIL",
                "GIT_AUTHOR_NAME",
                "GIT_COMMITTER_DATE",
                "GIT_COMMITTER_EMAIL",
                "GIT_COMMITTER_NAME",
                "GIT_CONFIG_GLOBAL",
                "GIT_CONFIG_NOSYSTEM",
                "GIT_OPTIONAL_LOCKS",
                "GIT_TERMINAL_PROMPT",
            ]
            .map(String::from)
            .into_iter()
            .collect()
        );
    }
    #[test]
    fn user_commands_never_inherit_an_askpass_helper_and_never_prompt() {
        let inherited = [
            ("PATH", "host-path"),
            ("GIT_ASKPASS", "code-askpass.sh"),
            ("SSH_ASKPASS", "code-askpass.sh"),
            ("VSCODE_GIT_ASKPASS_NODE", "code.exe"),
            ("VSCODE_GIT_ASKPASS_MAIN", "askpass-main.js"),
            ("VSCODE_GIT_IPC_HANDLE", "pipe"),
            ("GIT_SSH_COMMAND", "ssh -i key"),
            ("GIT_CONFIG_COUNT", "1"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));

        let cmd = user_command(Path::new("."), &["version"], &[], inherited);
        let env: Vec<_> = cmd.get_envs().collect();
        let has = |name: &str| {
            env.iter()
                .any(|(n, v)| *n == OsStr::new(name) && v.is_some())
        };

        assert!(has("PATH"));
        assert!(has("GIT_SSH_COMMAND"));
        assert!(!has("GIT_ASKPASS"));
        assert!(!has("SSH_ASKPASS"));
        assert!(!has("VSCODE_GIT_ASKPASS_NODE"));
        assert!(!has("VSCODE_GIT_ASKPASS_MAIN"));
        assert!(!has("VSCODE_GIT_IPC_HANDLE"));
        assert!(!has("GIT_CONFIG_COUNT"));
        assert!(env.contains(&(OsStr::new("GIT_TERMINAL_PROMPT"), Some(OsStr::new("0")))));
        assert!(env.contains(&(OsStr::new("GCM_INTERACTIVE"), Some(OsStr::new("never")))));
    }

    #[test]
    fn a_command_that_outlives_its_limit_is_killed_and_reported() {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        let started = std::time::Instant::now();
        let err = output_with_timeout(&mut cmd, Duration::from_millis(300)).unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(format!("{err:#}").contains("did not finish"), "{err:#}");
    }

    #[test]
    fn a_command_that_finishes_in_time_returns_its_output() {
        let out = output_with_timeout(
            Command::new("git").arg("--version"),
            Duration::from_secs(30),
        )
        .unwrap();

        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("git version"));
    }
}
