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

use anyhow::{Context, Result, bail};

/// Same author and date for every baseline commit, so the host's copy and the
/// agent-visible repo get the same commit id.
const BASELINE_DATE: &str = "2000-01-01T00:00:00+0000";

/// `git <args>` with a hardened environment. `git_dir` and `work_tree` are
/// passed as `--git-dir`/`--work-tree` when given; `cwd` is where it runs.
pub(crate) fn output(
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
