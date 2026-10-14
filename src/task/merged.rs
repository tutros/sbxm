//! `task rm --merged` (issue 202, decision 195): finds every task whose work is merged, shows what
//! it would remove and why, and removes it after one confirmation. Never touches a task whose PR
//! is open or closed without merging, nor one whose result isn't safely elsewhere yet (decision
//! 178's protection).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::finish::{self, RemovalPlan};
use super::record::{self, Kind, ProcessProbe, Record};
use super::repo;
use super::review;
use crate::backend::SandboxBackend;
use crate::confirm::Confirm;
use crate::github::{GitHubBackend, PrState};

/// A task `task rm --merged` would remove, and why.
#[derive(Debug)]
pub struct Candidate {
    pub id: String,
    pub reason: String,
    pub plan: RemovalPlan,
    /// A spec task's own worktree and local branch (decision 195 point 3): `task rm --merged`
    /// never deletes either, since it has no record of where, or whether, the user checked the
    /// branch out; these are the two commands that do it by hand. `None` for an issue or
    /// PR-review task.
    pub local_cleanup: Option<[String; 2]>,
}

/// The two git commands that clean up a spec task's result in the user's own checkout (decision
/// 195 point 3), named instead of run: the branch's own worktree, if the user made one to look at
/// it, and the local branch itself.
fn local_cleanup_commands(branch: &str) -> [String; 2] {
    [
        format!(
            "git worktree remove <path>, naming the worktree you checked {branch} out in (skip \
             this if you never made one)"
        ),
        format!("git branch -D {branch}"),
    ]
}

/// A task considered but left alone, and why (decision 195 point 3: listed, not silently dropped,
/// when it is merged but protected, or when a spec task's merge can't be told from "not merged
/// yet").
#[derive(Debug)]
pub struct LeftAlone {
    pub id: String,
    pub reason: String,
}

/// What a `--merged` scan found. A task whose PR is open, or closed without merging, or that has
/// no PR at all yet, is excluded entirely: neither list names it.
#[derive(Debug, Default)]
pub struct MergedPlan {
    pub candidates: Vec<Candidate>,
    pub left_alone: Vec<LeftAlone>,
}

impl MergedPlan {
    /// Every task it would remove and why, then every task left alone and why: for the
    /// confirmation prompt and the dry run.
    pub fn listing(&self) -> String {
        let mut lines = Vec::new();
        for c in &self.candidates {
            lines.push(format!("task {}: {}", c.id, c.reason));
            for line in c.plan.listing().lines() {
                lines.push(format!("  {line}"));
            }
            if let Some(cleanup) = &c.local_cleanup {
                lines.push(
                    "  your own checkout isn't touched; clean it up by hand with:".to_owned(),
                );
                for cmd in cleanup {
                    lines.push(format!("    {cmd}"));
                }
            }
        }
        for l in &self.left_alone {
            lines.push(format!("left alone, task {}: {}", l.id, l.reason));
        }
        lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// [`finish::plan_removal`] for a task already known to be merged: a task that can't be planned
/// (most likely because it is running) is left alone instead of stopping the whole scan, with the
/// reason [`finish::plan_removal`] gave.
fn add_merged(
    base_dir: &Path,
    id: &str,
    reason: String,
    local_cleanup: Option<[String; 2]>,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) {
    match finish::plan_removal(base_dir, id, probe) {
        Ok(plan) => out.candidates.push(Candidate {
            id: id.to_owned(),
            reason,
            plan,
            local_cleanup,
        }),
        Err(e) => out.left_alone.push(LeftAlone {
            id: id.to_owned(),
            reason: format!("{e:#}"),
        }),
    }
}

/// A task whose PR is `number` is merged: a candidate; anything else (open, or closed without
/// merging) is left alone silently, like `task rm` leaves it.
fn check_pr_number(
    base_dir: &Path,
    task: &Record,
    number: u32,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) -> Result<()> {
    let info = github.pr(&task.repo, number).with_context(|| {
        format!(
            "task {}: cannot read PR #{number} of {}",
            task.id, task.repo
        )
    })?;
    if info.state != PrState::Merged {
        return Ok(());
    }
    add_merged(
        base_dir,
        &task.id,
        format!("PR #{number} is merged"),
        None,
        probe,
        out,
    );
    Ok(())
}

/// An issue task (decision 195 point 1): no PR yet isn't a candidate at all.
fn check_issue(
    base_dir: &Path,
    task: &Record,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) -> Result<()> {
    let Some(url) = &task.pr else {
        return Ok(());
    };
    let Some(number) = finish::pr_number(url) else {
        out.left_alone.push(LeftAlone {
            id: task.id.clone(),
            reason: format!("its recorded PR {url:?} doesn't end in a PR number"),
        });
        return Ok(());
    };
    check_pr_number(base_dir, task, number, github, probe, out)
}

/// A PR-review task (decision 195 point 1): its own number is the PR's.
fn check_pr_review(
    base_dir: &Path,
    task: &Record,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) -> Result<()> {
    check_pr_number(base_dir, task, task.number, github, probe, out)
}

/// A spec task (decision 195 point 1): merged is "the branch's tip is reachable from the base
/// branch as `origin/<base>` stands after a fetch", checked for any non-running spec task that
/// has commits, whether or not `task finish --spec` has run yet; a task still working, gating,
/// reviewing or fixing is left alone by the removal-state check inside [`add_merged`], the same
/// one that protects a running issue or PR-review task, so this doesn't duplicate that check. A
/// squash or rebase merge isn't detected, so a task that looks not-merged by this test is listed,
/// not dropped silently, naming the plain command to remove it by hand. A result that counts as
/// merged but was never fetched into `repo_root` is left alone too (decision 178's protection,
/// reused as-is).
fn check_spec(
    base_dir: &Path,
    task: &Record,
    repo_root: &Path,
    probe: &dyn ProcessProbe,
    out: &mut MergedPlan,
) -> Result<()> {
    let meta = record::task_dir(base_dir, &task.id);
    let repo_git = meta.join("repo.git");
    if !repo_git.is_dir() {
        return Ok(());
    }
    if !repo::valid_ref_name(&task.branch) || !repo::valid_ref_name(&task.base) {
        out.left_alone.push(LeftAlone {
            id: task.id.clone(),
            reason: "records a branch or base name that isn't usable; see task.json".to_owned(),
        });
        return Ok(());
    }
    if task.commits_ahead(&repo_git)? == 0 {
        return Ok(());
    }
    repo::fetch_base(&repo_git, &task.base).with_context(|| {
        format!(
            "task {}: cannot check whether its branch is merged into {}",
            task.id, task.base
        )
    })?;
    let tip = repo::branch_tip(&repo_git, &task.branch)?;
    if !repo::is_ancestor_of_remote_base(&repo_git, &tip, &task.base)? {
        out.left_alone.push(LeftAlone {
            id: task.id.clone(),
            reason: format!(
                "not merged into {} (a squash or rebase merge isn't detected); remove it with \
                 `sbxm task rm --spec` naming its file if you're sure it's done",
                task.base
            ),
        });
        return Ok(());
    }
    if let Err(e) = finish::check_spec_result_fetched(base_dir, &task.id, repo_root) {
        out.left_alone.push(LeftAlone {
            id: task.id.clone(),
            reason: format!("{e:#}"),
        });
        return Ok(());
    }
    add_merged(
        base_dir,
        &task.id,
        format!("branch {} is merged into {}", task.branch, task.base),
        Some(local_cleanup_commands(&task.branch)),
        probe,
        out,
    );
    Ok(())
}

/// Scans every task under `base_dir` and works out which are merged (decision 195). Nothing is
/// removed; the caller shows [`MergedPlan::listing`], confirms, then calls [`remove`].
pub fn plan(
    base_dir: &Path,
    repo_root: &Path,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
) -> Result<MergedPlan> {
    let mut out = MergedPlan::default();
    for task in record::load_all(base_dir)? {
        match task.kind {
            Kind::Issue => check_issue(base_dir, &task, github, probe, &mut out)?,
            Kind::Pr => check_pr_review(base_dir, &task, github, probe, &mut out)?,
            Kind::Spec => check_spec(base_dir, &task, repo_root, probe, &mut out)?,
        }
    }
    Ok(out)
}

/// Where a merged task's text records go before removal (decision 195 point 2): under
/// `.sbxm/task-archive/`, so it is on the same drive as the base dir and outside `tasks/` and
/// `.sbxm/tasks/`, which `task status` and `task rm` scan.
pub fn archive_dir(base_dir: &Path, id: &str) -> PathBuf {
    base_dir.join(".sbxm").join("task-archive").join(id)
}

/// The names of a task's text records worth archiving (decision 195 point 2): `review.md`, every
/// `review-<n>.md`, `result.md`, `run.log`, `gates.log` and `task.json`. Never transcripts, clones
/// or `repo.git`: those stay with whatever else `task rm` deletes.
fn archive_names(meta: &Path) -> Vec<String> {
    let mut names = vec![
        "review.md".to_owned(),
        "result.md".to_owned(),
        "run.log".to_owned(),
        "gates.log".to_owned(),
        "task.json".to_owned(),
    ];
    names.extend(
        review::saved_rounds(meta)
            .into_iter()
            .map(|r| format!("review-{r}.md")),
    );
    names
}

/// Copies `id`'s text records into [`archive_dir`] before its folder is removed (decision 195
/// point 2). Only files that exist are copied; a name [`archive_names`] lists but that isn't
/// there is silently skipped, never an error. Refuses, without writing anything, if the archive
/// path already exists in any form (a directory from an earlier archive of the same id, a plain
/// file, or a symlink or junction, checked without following it): "nothing deletes an archive"
/// means nothing overwrites one either. Returns the archive folder.
pub fn archive_task(base_dir: &Path, id: &str) -> Result<PathBuf> {
    let meta = record::task_dir(base_dir, id);
    let archive = archive_dir(base_dir, id);
    if fs::symlink_metadata(&archive).is_ok() {
        bail!(
            "{} already exists; move or delete it by hand, then run `sbxm task rm --merged` again",
            archive.display()
        );
    }
    fs::create_dir_all(&archive).with_context(|| format!("cannot create {}", archive.display()))?;
    for name in archive_names(&meta) {
        let src = meta.join(&name);
        if !src.is_file() {
            continue;
        }
        let dest = archive.join(&name);
        fs::copy(&src, &dest)
            .with_context(|| format!("cannot archive {} to {}", src.display(), dest.display()))?;
    }
    Ok(archive)
}

/// Archives, then removes, every candidate in `merged` (decision 195): one confirmation for the
/// whole batch (`--dry-run` never reaches here; the caller only lists). A task whose archive
/// fails is left entirely alone (its folders are not deleted), counted as stayed like a folder
/// `finish::remove` could not delete. Reports the disk freed on the base dir's drive when
/// `free_space` can read it both before and after (decision 195's "if it can be read").
pub fn remove(
    base_dir: &Path,
    merged: &MergedPlan,
    yes: bool,
    backend: &dyn SandboxBackend,
    confirm: &dyn Confirm,
    free_space: fn(&Path) -> std::io::Result<u64>,
    out: &mut dyn Write,
) -> Result<()> {
    if merged.candidates.is_empty() {
        writeln!(out, "no merged tasks to remove")?;
        return Ok(());
    }
    let listing = merged.listing();
    if !yes {
        if !confirm.is_interactive() {
            bail!(
                "refusing to remove merged tasks without a terminal; pass --yes to delete:\n{listing}"
            );
        }
        if !confirm.confirm(&format!(
            "Permanently delete {} merged task(s):\n{listing}\n",
            merged.candidates.len()
        ))? {
            bail!("removing merged tasks cancelled; nothing was deleted");
        }
    }
    let before = free_space(base_dir).ok();
    let mut stayed = 0;
    for candidate in &merged.candidates {
        match archive_task(base_dir, &candidate.id) {
            Ok(archive) => writeln!(
                out,
                "archived task {} to {}",
                candidate.id,
                archive.display()
            )?,
            Err(e) => {
                writeln!(out, "could not archive task {}: {e:#}", candidate.id)?;
                stayed += 1;
                continue;
            }
        }
        let report = finish::remove(&candidate.plan, backend);
        for line in &report.removed {
            writeln!(out, "removed {line}")?;
        }
        for line in &report.stayed {
            writeln!(out, "could not remove {line}")?;
        }
        stayed += report.stayed.len();
        if let Some(cleanup) = &candidate.local_cleanup {
            writeln!(
                out,
                "task {}: your own checkout isn't touched; clean it up by hand with:",
                candidate.id
            )?;
            for cmd in cleanup {
                writeln!(out, "  {cmd}")?;
            }
        }
    }
    if let (Some(before), Some(after)) = (before, free_space(base_dir).ok())
        && after > before
    {
        writeln!(
            out,
            "freed about {} on {}",
            crate::disk::gib(after - before),
            crate::disk::describe(base_dir)
        )?;
    }
    if stayed > 0 {
        bail!("{stayed} thing(s) stayed; fix that, then run `sbxm task rm --merged` again");
    }
    Ok(())
}
