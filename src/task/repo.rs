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
    let cloned = git::user_run(parent, &["clone", "--bare", "--", source, text(dest)?]);
    if cloned.is_err() {
        // A failed clone can leave a half-made folder (slowly, on Windows); nothing of it is wanted.
        let _ = std::fs::remove_dir_all(dest);
    }
    cloned.with_context(|| {
        format!("cannot clone {source}; check the repo name, `gh auth status` and that git can log in without a prompt (`gh auth setup-git`)")
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
    if has_branch(repo_git, branch)? {
        bail!(
            "branch {branch} already exists in the repo (a clone has every branch of origin, so it \
             was probably pushed by an earlier `sbxm task finish`); delete it on GitHub, or merge \
             it, before starting this task again"
        );
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
            // The user's `core.autocrlf` (true on Windows) would write CRLF files that the
            // sandbox's Linux git then sees as modified; no conversion, in either direction.
            "-c",
            "core.autocrlf=false",
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

/// Checks out `branch` of `repo_git` into the new folder `dest`: only committed files, no hooks
/// and no line-ending conversion, made by the hardened runner (no user config, so no filters or
/// helpers run). This is where host gates and a reviewer's clone come from: no file the agent
/// merely left lying around reaches it. `dest` must not exist.
pub fn clean_checkout(repo_git: &Path, branch: &str, dest: &Path) -> Result<()> {
    check_ref("branch", branch)?;
    if dest.exists() {
        bail!(
            "{} already exists; remove it or let the previous run finish",
            dest.display()
        );
    }
    if !has_branch(repo_git, branch)? {
        bail!("branch {branch} isn't in the task repo; collect the worker's commits first");
    }
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let made = git::run(
        parent,
        None,
        None,
        &[
            // The hardened runner denies every protocol; a local path is the one this needs.
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--no-hardlinks",
            "--template=",
            "-c",
            "core.autocrlf=false",
            "--branch",
            branch,
            "--",
            text(repo_git)?,
            text(dest)?,
        ],
    );
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(dest);
        return Err(e)
            .with_context(|| format!("cannot check out {branch} into {}", dest.display()));
    }
    Ok(())
}

/// Fetches the head of PR `number` from GitHub into the local branch `pr-<n>`; returns its name.
pub fn fetch_pr_head(repo_git: &Path, number: u32) -> Result<String> {
    let branch = format!("pr-{number}");
    fetch_pr_into(repo_git, number, &branch)?;
    Ok(branch)
}

/// Fetches the head of PR `number` from GitHub into the local branch `branch` (the PR's own
/// branch name, for a task that continues the PR), replacing what the clone had there; returns
/// the commit it is at.
pub fn fetch_pr_branch(repo_git: &Path, number: u32, branch: &str) -> Result<String> {
    check_ref("branch", branch)?;
    fetch_pr_into(repo_git, number, branch)?;
    branch_tip(repo_git, branch)
}

fn fetch_pr_into(repo_git: &Path, number: u32, branch: &str) -> Result<()> {
    let refspec = format!("+refs/pull/{number}/head:refs/heads/{branch}");
    git::user_run(
        repo_git,
        &[
            "--git-dir",
            text(repo_git)?,
            // A PR author's objects are as untrusted as an agent's bundle: have git check them.
            "-c",
            "fetch.fsckObjects=true",
            "-c",
            "transfer.fsckObjects=true",
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "origin",
            &refspec,
        ],
    )
    .with_context(|| format!("cannot fetch the head of PR #{number}; is it a PR of this repo?"))?;
    Ok(())
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

/// The commit `branch` points at in `repo_git`.
pub fn branch_tip(repo_git: &Path, branch: &str) -> Result<String> {
    check_ref("branch", branch)?;
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )?;
    Ok(out.trim().to_owned())
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

/// Pushes `branch` from `repo_git` to `origin` only as a new branch: the lease's empty expected
/// value (`--force-with-lease=refs/heads/<branch>:`) makes the push fail if the branch exists on
/// the remote when it lands, so it never moves or overwrites anyone's branch.
pub fn push_new(repo_git: &Path, branch: &str) -> Result<()> {
    check_ref("branch", branch)?;
    if !has_branch(repo_git, branch)? {
        bail!("branch {branch} isn't in the task repo; nothing to push");
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let lease = format!("--force-with-lease=refs/heads/{branch}:");
    let out = git::user_output(
        repo_git,
        &[
            "--git-dir",
            text(repo_git)?,
            "push",
            &lease,
            "origin",
            &refspec,
        ],
        &[],
    )?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("stale info") {
        bail!("{branch} appeared on the remote after sbxm checked it; nothing was pushed");
    }
    bail!(
        "cannot push {branch}; check `gh auth status` and your access to the repo: `git push` failed ({}): {}",
        out.status,
        stderr.trim()
    )
}

/// Moves `branch` on GitHub (`origin`) from `from` (a full commit id) to its tip in `repo_git`,
/// as a fast-forward and only if the remote branch is still at `from` when the push lands: the
/// tip must descend from `from`, and the push carries `from` as the expected old value
/// (`--force-with-lease`), so a branch that moved meanwhile, even to an ancestor of the tip, is
/// left alone and the push is refused.
pub fn push_from(repo_git: &Path, branch: &str, from: &str) -> Result<()> {
    check_ref("branch", branch)?;
    if !is_commit_id(from) {
        bail!("{from:?} isn't a full commit id; the task record may be damaged");
    }
    let out = git::output(
        repo_git,
        Some(repo_git),
        None,
        &[
            "merge-base",
            "--is-ancestor",
            from,
            &format!("refs/heads/{branch}"),
        ],
    )?;
    if !out.status.success() {
        bail!(
            "{branch} in the task repo doesn't descend from {from}, so pushing it wouldn't be a fast-forward"
        );
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let lease = format!("--force-with-lease=refs/heads/{branch}:{from}");
    let out = git::user_output(
        repo_git,
        &[
            "--git-dir",
            text(repo_git)?,
            "push",
            &lease,
            "origin",
            &refspec,
        ],
        &[],
    )?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("stale info") {
        bail!(
            "{branch} moved on the remote after sbxm checked it (it is no longer at {from}); nothing was pushed"
        );
    }
    bail!(
        "cannot push {branch}; check `gh auth status` and your access to the repo: `git push` failed ({}): {}",
        out.status,
        stderr.trim()
    )
}

/// The commit `branch` is at on GitHub (`origin`), or `None` when the remote has no such branch.
pub fn remote_branch_head(repo_git: &Path, branch: &str) -> Result<Option<String>> {
    check_ref("branch", branch)?;
    let wanted = format!("refs/heads/{branch}");
    let out = git::user_run(
        repo_git,
        &["--git-dir", text(repo_git)?, "ls-remote", "origin", &wanted],
    )
    .with_context(|| {
        format!(
            "cannot read {branch} on the remote; check `gh auth status` and your access to the repo"
        )
    })?;
    Ok(out.lines().find_map(|line| {
        let (id, name) = line.split_once('\t')?;
        (name == wanted).then(|| id.to_owned())
    }))
}

/// The commits `branch` has past `commit` (a full commit id), oldest first, each as
/// `<short id> <subject>`.
pub fn commit_lines(repo_git: &Path, commit: &str, branch: &str) -> Result<Vec<String>> {
    if !is_commit_id(commit) {
        bail!("{commit:?} isn't a full commit id; the task record may be damaged");
    }
    check_ref("branch", branch)?;
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "log",
            "--reverse",
            "--format=%h %s",
            &format!("{commit}..refs/heads/{branch}"),
        ],
    )?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// Where the sandbox writes the bundle (spec §6), relative to the workspace.
const BUNDLE_DIR: &str = ".sbxm-task";
const BUNDLE_FILE: &str = ".sbxm-task/branch.bundle";

/// The default size cap for a bundle collected from a sandbox: 500 MB.
pub const BUNDLE_CAP: u64 = 500 * 1024 * 1024;

/// Opens `<workspace>/.sbxm-task/<name>` for reading, trusting nothing about it: the folder must
/// resolve to a real folder inside the workspace (not a link out of it) and the file must be a
/// plain file, checked on the open handle. `Ok(None)` when the folder or file doesn't exist.
fn open_agent_file(workspace: &Path, name: &str) -> Result<Option<(std::fs::File, u64)>> {
    use std::io::ErrorKind::NotFound;

    let folder = workspace.join(BUNDLE_DIR);
    let real_workspace = std::fs::canonicalize(workspace)
        .with_context(|| format!("cannot read the workspace {}", workspace.display()))?;
    let real_folder = match std::fs::canonicalize(&folder) {
        Ok(path) => path,
        Err(e) if e.kind() == NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", folder.display())),
    };
    if real_folder != real_workspace.join(BUNDLE_DIR) {
        bail!(
            "the folder {} doesn't resolve to a folder inside the workspace; refusing to read it",
            folder.display()
        );
    }
    let path = real_folder.join(name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let not_plain = || {
        anyhow::anyhow!(
            "{} is not a plain file; refusing to read it",
            path.display()
        )
    };
    if !meta.file_type().is_file() {
        return Err(not_plain());
    }
    let file =
        std::fs::File::open(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(not_plain());
    }
    Ok(Some((file, opened.len())))
}

/// What `write_agent_files` does when `.sbxm-task` is not a plain folder inside the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Refuse: the agent's own clone, where a link can only be the agent's doing.
    Refuse,
    /// Remove it and make a real folder: a fresh checkout, where the branch's content (what the
    /// worker committed) decides what is there.
    Replace,
}

/// Removes whatever is at `path` without following it: a link goes, a folder goes with its content.
fn remove_entry(path: &Path) -> Result<()> {
    let kind = std::fs::symlink_metadata(path)?.file_type();
    if kind.is_symlink() {
        // A Windows junction or directory symlink is removed as a directory, a file link as a file.
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))?;
    } else if kind.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Writes sbxm's own files into `<workspace>/.sbxm-task`, trusting nothing about what is already
/// there: the folder must be a real folder inside the workspace and no file is written through a
/// link (a file that exists is removed and created again, so a hard link isn't written through
/// either). Without this, a worker that replaces `.sbxm-task` or one of its files with a link makes
/// the host overwrite whatever the link points at (PR 57 review, M-1).
pub fn write_agent_files(
    workspace: &Path,
    files: &[(&str, &[u8])],
    existing: Existing,
) -> Result<()> {
    use std::io::Write;

    let folder = workspace.join(BUNDLE_DIR);
    let real_workspace = std::fs::canonicalize(workspace)
        .with_context(|| format!("cannot read the workspace {}", workspace.display()))?;
    let expected = real_workspace.join(BUNDLE_DIR);
    let is_real_folder = |folder: &Path| -> bool {
        std::fs::symlink_metadata(folder).is_ok_and(|m| m.file_type().is_dir())
            && std::fs::canonicalize(folder).is_ok_and(|real| real == expected)
    };
    if std::fs::symlink_metadata(&folder).is_ok() && !is_real_folder(&folder) {
        match existing {
            Existing::Refuse => bail!(
                "{} is a link or not a folder inside the workspace; refusing to write into it",
                folder.display()
            ),
            Existing::Replace => remove_entry(&folder)
                .with_context(|| format!("cannot replace {}", folder.display()))?,
        }
    }
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    if !is_real_folder(&folder) {
        bail!(
            "{} doesn't resolve to a folder inside the workspace; refusing to write into it",
            folder.display()
        );
    }
    for (name, bytes) in files {
        let path = folder.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
            if !plain && existing == Existing::Refuse {
                bail!(
                    "{} is a link or not a file; refusing to write it",
                    path.display()
                );
            }
            remove_entry(&path).with_context(|| format!("cannot replace {}", path.display()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// What reading a file the agent wrote found.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentFile {
    Missing,
    /// Bigger than the cap; not read.
    TooLarge(u64),
    Text(String),
}

/// Reads `<workspace>/.sbxm-task/<name>` as text (lossily), at most `cap` bytes; never follows
/// a link out of the workspace.
pub fn read_agent_file(workspace: &Path, name: &str, cap: u64) -> Result<AgentFile> {
    use std::io::Read;

    let Some((file, len)) = open_agent_file(workspace, name)? else {
        return Ok(AgentFile::Missing);
    };
    if len > cap {
        return Ok(AgentFile::TooLarge(len));
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Ok(AgentFile::TooLarge(bytes.len() as u64));
    }
    Ok(AgentFile::Text(
        String::from_utf8_lossy(&bytes).into_owned(),
    ))
}

/// Brings the agent's commits on `branch` into `repo_git` from the bundle the sandbox wrote at
/// `<workspace>/.sbxm-task/branch.bundle` (inside the agent's workspace, so nothing about it is
/// trusted).
///
/// The bundle's folder must resolve to a real folder inside the workspace (not a link out of it),
/// and the bundle must be a plain file of at most `cap` bytes: it is opened once, its size is
/// read from the open file, and at most `cap + 1` bytes are copied, so a file that grows or is
/// swapped during the copy cannot fill the disk. The copy goes to a host-owned folder, is checked
/// with `git bundle verify` (including that `repo_git` has everything it builds on), and only
/// `refs/heads/<branch>` is fetched: fast-forward only, hooks switched off, objects fsck-checked,
/// no submodules, no other refs. Nothing runs `git` inside the workspace.
pub fn fetch_bundle(repo_git: &Path, workspace: &Path, branch: &str, cap: u64) -> Result<()> {
    use std::io::Read;

    check_ref("branch", branch)?;
    let Some((mut source, len)) = open_agent_file(workspace, "branch.bundle")? else {
        bail!("cannot find {BUNDLE_FILE} in the workspace; did the agent's sandbox write it?");
    };
    if len > cap {
        bail!(
            "the bundle is too large ({len} bytes, the limit is {cap}); ask the agent for smaller changes"
        );
    }

    let task_dir = repo_git.parent().unwrap_or(Path::new("."));
    let work = task_dir.join("bundle");
    std::fs::create_dir_all(&work).with_context(|| format!("cannot create {}", work.display()))?;
    let copy = work.join("branch.bundle");
    let copied = {
        let mut target = std::fs::File::create(&copy)
            .with_context(|| format!("cannot write {}", copy.display()))?;
        std::io::copy(&mut (&mut source).take(cap.saturating_add(1)), &mut target)
            .with_context(|| format!("cannot copy the bundle to {}", copy.display()))?
    };
    if copied > cap {
        let _ = std::fs::remove_file(&copy);
        bail!(
            "the bundle is too large (it grew past the {cap}-byte limit while it was being copied)"
        );
    }
    let copy_text = text(&copy)?;

    git::run(
        repo_git,
        Some(repo_git),
        None,
        &["bundle", "verify", copy_text],
    )
    .context("the bundle failed verification (damaged, or built on commits the task repo lacks)")?;
    let heads = git::run(
        repo_git,
        Some(repo_git),
        None,
        &["bundle", "list-heads", copy_text],
    )?;
    let wanted = format!("refs/heads/{branch}");
    if !heads
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(wanted.as_str()))
    {
        bail!("the bundle has no branch {branch}; the agent must commit on {branch}");
    }

    let no_hooks = task_dir.join("no-hooks");
    std::fs::create_dir_all(&no_hooks)
        .with_context(|| format!("cannot create {}", no_hooks.display()))?;
    let refspec = format!("{wanted}:{wanted}");
    let out = git::output(
        repo_git,
        Some(repo_git),
        None,
        &[
            "-c",
            &format!("core.hooksPath={}", text(&no_hooks)?),
            // The hardened runner denies every protocol; a local file is the one this needs
            // (the verified copy above), so `ext::` and the network transports stay denied.
            "-c",
            "protocol.file.allow=always",
            "-c",
            "transfer.fsckObjects=true",
            "-c",
            "fetch.fsckObjects=true",
            "-c",
            "gc.auto=0",
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            copy_text,
            &refspec,
        ],
    )?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("non-fast-forward") || stderr.contains("rejected") {
            bail!(
                "the agent rewrote the history of {branch}; it must add commits on top of the ones already collected"
            );
        }
        bail!("`git fetch` from the bundle failed: {}", stderr.trim());
    }
    Ok(())
}

/// The paths `branch` changes relative to where it left `base` (added, changed or deleted; a rename
/// counts as both its old and its new path), as git prints them.
pub fn changed_paths(repo_git: &Path, base: &str, branch: &str) -> Result<Vec<String>> {
    check_ref("base", base)?;
    diff_names(repo_git, &format!("refs/heads/{base}"), branch)
}

/// [`changed_paths`] from `commit` (a full commit id, e.g. where a continued PR's branch was when
/// the task started).
pub fn changed_paths_since(repo_git: &Path, commit: &str, branch: &str) -> Result<Vec<String>> {
    if !is_commit_id(commit) {
        bail!("{commit:?} isn't a full commit id; the task record may be damaged");
    }
    diff_names(repo_git, commit, branch)
}

fn diff_names(repo_git: &Path, from: &str, branch: &str) -> Result<Vec<String>> {
    check_ref("branch", branch)?;
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            &format!("{from}...refs/heads/{branch}"),
        ],
    )?;
    Ok(out
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

/// A full commit id (SHA-1 or SHA-256 hex), the only form [`commits_since`] accepts.
pub fn is_commit_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How many commits `branch` has past `commit` (a full commit id, e.g. where a continued PR's
/// branch was when the task started).
pub fn commits_since(repo_git: &Path, commit: &str, branch: &str) -> Result<u32> {
    if !is_commit_id(commit) {
        bail!("{commit:?} isn't a full commit id; the task record may be damaged");
    }
    check_ref("branch", branch)?;
    let out = git::run(
        repo_git,
        Some(repo_git),
        None,
        &[
            "rev-list",
            "--count",
            &format!("{commit}..refs/heads/{branch}"),
        ],
    )?;
    out.trim()
        .parse()
        .with_context(|| format!("unexpected `git rev-list --count` output: {out}"))
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
