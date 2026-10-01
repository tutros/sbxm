//! The host-owned repo of a task (spec §6, decision 159): `repo.git`, a bare clone that only
//! sbxm and the user's git touch. The agent's workspace is a clone *of* it; no host git
//! command ever runs inside that workspace. Network operations (clone, fetch, push) use the
//! user's git config and credentials; local ones use the hardened runner.

use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::git;

/// A branch or ref name that is safe to hand to git as an argument: no leading `-`, no
/// spaces or shell-ish characters, none of git's forbidden sequences.
pub fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

fn check_ref(what: &str, name: &str) -> Result<()> {
    if !valid_ref_name(name) {
        bail!(
            "{what} {name:?} isn't a usable branch name; use letters, digits, `.`, `_`, `-` and `/`"
        );
    }
    Ok(())
}

fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} isn't valid UTF-8; use another folder", path.display()))
}

/// The `user.name` and `user.email` the agent's commits carry (the sandbox can't see the
/// host's git config, spec §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// From the user's global git config, or from `config` when given (tests).
    pub fn read_from(config: Option<&Path>) -> Result<Self> {
        let cwd = std::env::temp_dir();
        let get = |key: &str| -> Result<String> {
            let extra: Vec<(&str, &OsStr)> = config
                .map(|path| ("GIT_CONFIG_GLOBAL", path.as_os_str()))
                .into_iter()
                .collect();
            let out = git::user_output(&cwd, &["config", "--global", "--get", key], &extra)?;
            let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !out.status.success() || value.is_empty() {
                bail!("git has no {key}; set it with `git config --global {key} <value>`");
            }
            Ok(value)
        };
        Ok(Self {
            name: get("user.name")?,
            email: get("user.email")?,
        })
    }
}

/// `git clone --bare <source> <dest>`: `source` is a GitHub URL (or a local path in tests).
pub fn clone_bare(source: &str, dest: &Path) -> Result<()> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    git::user_run(parent, &["clone", "--bare", "--", source, text(dest)?]).with_context(|| {
        format!("cannot clone {source}; check the repo name and `gh auth status`")
    })?;
    Ok(())
}

/// Creates `branch` at the tip of `base` in `repo_git`.
pub fn create_branch(repo_git: &Path, branch: &str, base: &str) -> Result<()> {
    check_ref("branch", branch)?;
    check_ref("base", base)?;
    let base_ref = format!("refs/heads/{base}");
    let found = git::output(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base_ref}^{{commit}}"),
        ],
    )?;
    if !found.status.success() {
        bail!("base branch {base} not found in the repo; check --base");
    }
    git::run(
        repo_git,
        Some(repo_git),
        None,
        &["branch", "--", branch, &base_ref],
    )?;
    Ok(())
}

/// Clones `repo_git` into `workspace` on `branch`, for the agent to work in. Git objects are
/// copied (`--no-hardlinks`) so the agent cannot alter `repo.git`'s files through the mount,
/// and no hooks come along (`--template=`).
pub fn clone_workspace(
    repo_git: &Path,
    workspace: &Path,
    branch: &str,
    identity: &Identity,
) -> Result<()> {
    check_ref("branch", branch)?;
    let parent = workspace.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let name = format!("user.name={}", identity.name);
    let email = format!("user.email={}", identity.email);
    git::user_run(
        parent,
        &[
            "clone",
            "--no-hardlinks",
            "--template=",
            "-c",
            &name,
            "-c",
            &email,
            "--branch",
            branch,
            "--",
            text(repo_git)?,
            text(workspace)?,
        ],
    )
    .with_context(|| format!("cannot clone the task repo into {}", workspace.display()))?;
    Ok(())
}

/// Fetches the head of PR `number` from GitHub into the local branch `pr-<n>`; returns its name.
pub fn fetch_pr_head(repo_git: &Path, number: u32) -> Result<String> {
    let branch = format!("pr-{number}");
    let refspec = format!("+refs/pull/{number}/head:refs/heads/{branch}");
    git::user_run(
        repo_git,
        &[
            "--git-dir",
            text(repo_git)?,
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "origin",
            &refspec,
        ],
    )
    .with_context(|| format!("cannot fetch the head of PR #{number}; is it a PR of this repo?"))?;
    Ok(branch)
}

fn has_branch(repo_git: &Path, branch: &str) -> Result<bool> {
    let out = git::output(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )?;
    Ok(out.status.success())
}

/// Pushes `branch` (and only it) from `repo_git` to GitHub, never forcing.
pub fn push(repo_git: &Path, branch: &str) -> Result<()> {
    check_ref("branch", branch)?;
    if !has_branch(repo_git, branch)? {
        bail!("branch {branch} isn't in the task repo; nothing to push");
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    git::user_run(
        repo_git,
        &["--git-dir", text(repo_git)?, "push", "origin", &refspec],
    )
    .with_context(|| {
        format!("cannot push {branch}; check `gh auth status` and your access to the repo")
    })?;
    Ok(())
}

/// How many commits `branch` has that `base` doesn't.
pub fn commits_ahead(repo_git: &Path, base: &str, branch: &str) -> Result<u32> {
    check_ref("base", base)?;
    check_ref("branch", branch)?;
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-list",
            "--count",
            &format!("refs/heads/{base}..refs/heads/{branch}"),
        ],
    )?;
    out.trim()
        .parse()
        .with_context(|| format!("unexpected `git rev-list --count` output: {out}"))
}
