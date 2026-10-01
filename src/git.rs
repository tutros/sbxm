//! Host-side `git` calls on workspaces that an agent controls (M2a slice 7).
//!
//! A contestant can write anything into its workspace, including a `.git/config`
//! with `core.fsmonitor`, `diff.external`, filter or textconv commands, and
//! hooks. So sbxm never lets git read a repository inside a workspace: the
//! repository (`--git-dir`) is always a folder sbxm made outside it, the
//! workspace is only ever a `--work-tree`, and the user's global and system
//! git config are switched off so results don't depend on the machine either.

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
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_EXTERNAL_DIFF",
        "GIT_PAGER",
        "GIT_EDITOR",
        "GIT_ASKPASS",
        "GIT_SSH_COMMAND",
    ] {
        cmd.env_remove(var);
    }
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
    cmd.output()
        .context("cannot run `git`; is Git installed and on PATH?")
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
}
