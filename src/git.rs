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
/// file git just created, and git exits 128 with "Permission denied" when it
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

fn permission_denied(out: &Output) -> bool {
    out.status.code() == Some(128)
        && String::from_utf8_lossy(&out.stderr).contains("Permission denied")
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
const USER_GIT_VARIABLES: [&str; 8] = [
    "GIT_ASKPASS",
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
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd).args(args);
        cmd.env_clear();
        cmd.envs(std::env::vars_os().filter(|(name, _)| {
            !is_git_variable(name)
                || USER_GIT_VARIABLES
                    .iter()
                    .any(|allowed| name.to_string_lossy().eq_ignore_ascii_case(allowed))
        }));
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        for (name, value) in extra_env {
            cmd.env(name, value);
        }
        cmd.output()
            .context("cannot run `git`; is Git installed and on PATH?")
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
}
