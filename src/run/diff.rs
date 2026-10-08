//! What a contestant changed in its workspace, as a unified diff (decisions
//! 14, 113). Host `git` only ever sees a repository sbxm made outside the
//! workspace, never one the agent could have edited (see `crate::git`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::git;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// Unified diff; empty when nothing changed.
    pub patch: String,
    /// Workspace-relative paths left out of the diff because they are links.
    pub skipped: Vec<PathBuf>,
}

/// Seeded workspace: everything that differs from `baseline`, the commit made
/// by [`crate::seed::seed_contestant`] in `git_dir`. New files are included
/// even if the agent committed them, moved `HEAD` or deleted its own `.git`;
/// files the seed's ignore rules exclude stay excluded.
pub fn seeded(git_dir: &Path, workspace: &Path, baseline: &str) -> Result<Diff> {
    git::run(workspace, Some(git_dir), Some(workspace), &["add", "-A"])?;
    let patch = git::run(
        workspace,
        Some(git_dir),
        Some(workspace),
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            baseline,
        ],
    )?;
    Ok(Diff {
        patch,
        skipped: Vec::new(),
    })
}

/// Unseeded workspace (a plain folder, decision 113): every file, as added
/// against an empty folder. Any `.git` the agent made is left out, and links
/// are skipped, never followed, so nothing outside the workspace can end up
/// in the result.
pub fn unseeded(workspace: &Path) -> Result<Diff> {
    if !workspace.is_dir() {
        bail!(
            "workspace {} is not a folder; nothing to diff",
            workspace.display()
        );
    }
    let scratch = scratch_dir();
    let result = diff_copy(workspace, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    result
}

/// A scratch folder name no other call in this process gets: two pairs diffed at the same
/// moment would otherwise share one, and the first to finish deletes it under the other.
fn scratch_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("sbxm-diff-{}-{nanos}-{n}", std::process::id()))
}

/// The name the snapshot folder has inside `scratch`; stripped from the
/// paths in the diff headers again.
const SNAPSHOT: &str = "workspace";

fn diff_copy(workspace: &Path, scratch: &Path) -> Result<Diff> {
    let empty = scratch.join("empty");
    let snapshot = scratch.join(SNAPSHOT);
    fs::create_dir_all(&empty).with_context(|| format!("cannot create {}", empty.display()))?;
    let mut skipped = Vec::new();
    copy_filtered(workspace, &snapshot, Path::new(""), &mut skipped)?;

    let out = git::output(
        scratch,
        None,
        None,
        &[
            "diff",
            "--no-index",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            "empty",
            SNAPSHOT,
        ],
    )?;
    // Exit code 1 means "differences found", not failure (decision 113).
    match out.status.code() {
        Some(0 | 1) => {}
        _ => bail!(
            "`git diff --no-index` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
    let patch = strip_snapshot_prefix(&String::from_utf8_lossy(&out.stdout));
    skipped.sort();
    Ok(Diff { patch, skipped })
}

/// Copies `from` into `to`, leaving out every `.git` and every link.
fn copy_filtered(from: &Path, to: &Path, rel: &Path, skipped: &mut Vec<PathBuf>) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("cannot create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("cannot read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let rel = rel.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            skipped.push(rel);
        } else if file_type.is_dir() {
            copy_filtered(&entry.path(), &to.join(&name), &rel, skipped)?;
        } else {
            fs::copy(entry.path(), to.join(&name))
                .with_context(|| format!("cannot read {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// `a/workspace/x` and `b/workspace/x` in the header lines become `a/x` and `b/x`.
fn strip_snapshot_prefix(patch: &str) -> String {
    let (a, b) = (format!(" a/{SNAPSHOT}/"), format!(" b/{SNAPSHOT}/"));
    let mut out = String::with_capacity(patch.len());
    for line in patch.split_inclusive('\n') {
        let is_header = [
            "diff --git ",
            "--- ",
            "+++ ",
            "Binary files ",
            "rename ",
            "copy ",
            "similarity ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix));
        if is_header {
            let line = line
                .replacen(&a, " a/", 1)
                .replacen(&b, " b/", 1)
                .replacen(&format!("--- a/{SNAPSHOT}/"), "--- a/", 1)
                .replacen(&format!("+++ b/{SNAPSHOT}/"), "+++ b/", 1);
            out.push_str(&line);
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn scratch_dirs_made_at_the_same_time_never_share_a_name() {
        let names: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| (0..2_000).map(|_| scratch_dir()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect()
        });
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
    }
}
