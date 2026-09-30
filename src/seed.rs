//! Copying a seed directory into a new workspace.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::git;

/// Copies the contents of `seed` into `dest`, which must not exist yet.
/// Refuses before writing anything if the seed contains a link, since
/// following it could copy files from outside the seed into the sandbox.
pub fn copy(seed: &Path, dest: &Path) -> Result<()> {
    reject_links(seed)?;
    copy_tree(seed, dest)
}

/// Seeds one contestant's workspace: copies `seed` into `workspace` (which
/// must not exist), drops the copy's own `.git` (its history, remotes and
/// config: decisions 14, 47), then starts a fresh repository with exactly one
/// baseline commit of everything the seed's ignore files allow. Returns the
/// baseline commit id.
///
/// The baseline is also committed into `git_dir`, a repository sbxm keeps
/// outside the workspace, because that is the only one the host will later
/// trust to diff against (the agent can rewrite the one in the workspace; see
/// `crate::git`). Both commits have the same id.
pub fn seed_contestant(seed: &Path, workspace: &Path, git_dir: &Path) -> Result<String> {
    if !seed.is_dir() {
        bail!(
            "seed {} is not a directory; point task.seed at a folder",
            seed.display()
        );
    }
    reject_links(seed)?;
    if workspace.exists() {
        bail!(
            "workspace {} already exists; not seeding over it",
            workspace.display()
        );
    }
    for dir in [git_dir.parent(), workspace.parent()].into_iter().flatten() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    copy_tree(seed, workspace)?;
    match prepare_repo(workspace, git_dir) {
        Ok(id) => Ok(id),
        Err(e) => {
            // Only what this call just made.
            let _ = fs::remove_dir_all(workspace);
            let _ = fs::remove_dir_all(git_dir);
            Err(e)
        }
    }
}

fn prepare_repo(workspace: &Path, git_dir: &Path) -> Result<String> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        fs::remove_dir_all(&dot_git)
            .with_context(|| format!("cannot remove the seed's {}", dot_git.display()))?;
    } else if dot_git.exists() {
        fs::remove_file(&dot_git)
            .with_context(|| format!("cannot remove the seed's {}", dot_git.display()))?;
    }
    git::run(
        workspace,
        None,
        None,
        &["init", "--quiet", "--bare", &git_dir.to_string_lossy()],
    )?;
    let id = git::baseline_commit(workspace, git_dir, workspace)?;
    // The repository the agent sees, with the same single commit.
    git::run(workspace, None, None, &["init", "--quiet"])?;
    let agent_id = git::baseline_commit(workspace, &dot_git, workspace)?;
    if agent_id != id {
        bail!("the workspace baseline ({agent_id}) differs from the host's ({id})");
    }
    Ok(id)
}

/// Fails naming the first symlink or junction under `dir`. `new` calls it
/// with the other seed checks, before anything is written (decision 47).
pub fn reject_links(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "seed entry {} is a symlink or junction; remove it or copy the seed without links",
                entry.path().display()
            );
        }
        if file_type.is_dir() {
            reject_links(&entry.path())?;
        }
    }
    Ok(())
}

fn copy_tree(seed: &Path, dest: &Path) -> Result<()> {
    fs::create_dir(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    for entry in fs::read_dir(seed).with_context(|| format!("cannot read {}", seed.display()))? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}
