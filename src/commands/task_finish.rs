//! `sbxm task finish (--issue N | --spec FILE)` (spec §4): pushes the task's branch from its `repo.git` and opens
//! the PR, or, for a task that continues an open PR, pushes to that PR's branch and comments on it
//! (decision 169 (e)). The work is in `task::finish`.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::commands::task_start::{path_arg, spec_flag};
use crate::config::GlobalConfig;
use crate::github::GitHubBackend;
use crate::task::config::{Sink, TaskConfig};
use crate::task::finish;
use crate::task::record::{self, ProcessProbe};

/// What `finish` is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    /// A spec task's file (decision 174(d), issue 142): finished through the `[finish] sink` in
    /// `sbxm-task.toml` (decision 174(e), issue 143).
    Spec(PathBuf),
}

pub struct Options {
    pub target: Target,
    /// The checkout whose `sbxm-task.toml` names a spec task's `[finish] sink`.
    pub repo_root: PathBuf,
    /// Push a spec task that has `stopped` set or must-fix findings left (issue 144).
    pub push_unresolved: bool,
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    let (id, number) = match &opts.target {
        Target::Issue(0) => bail!("issue numbers start at 1; pass --issue <n>"),
        Target::Issue(n) => (format!("issue-{n}"), *n),
        Target::Spec(path) => {
            let id = record::spec_id(path)?.0;
            let base_dir = GlobalConfig::load(config_dir)?.base_dir;
            return finish_spec(&base_dir, &id, path, opts, probe, out);
        }
    };
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let done = finish::finish(&base_dir, &id, github, probe)?;
    for cut in &done.cuts {
        writeln!(out, "  note: {cut}")?;
    }
    if let Some(pr) = done.continued {
        let branch = &done.branch;
        writeln!(
            out,
            "{id}: pushed {branch} to PR #{pr} and commented on it, {}",
            done.url
        )?;
        for reason in &done.unresolved {
            writeln!(out, "  unresolved: {reason}")?;
        }
        writeln!(
            out,
            "  next: review PR #{pr} again with: sbxm task review --pr {pr} (if it was reviewed before, \
             first sbxm task rm --pr {pr}); after the merge, clean up with: sbxm task rm --issue {}",
            number
        )?;
        return Ok(());
    }
    if done.draft {
        writeln!(out, "{id}: pushed {id} and opened draft PR {}", done.url)?;
        for reason in &done.unresolved {
            writeln!(out, "  unresolved: {reason}")?;
        }
    } else {
        writeln!(out, "{id}: pushed {id} and opened {}", done.url)?;
    }
    writeln!(
        out,
        "  next: after it is merged, clean up with: sbxm task rm --issue {}",
        number
    )?;
    Ok(())
}

/// A spec task goes to the `[finish] sink` named in the checkout's `sbxm-task.toml` (decision
/// 174(e)): `local` keeps the branch in the task's `repo.git` and says how to fetch it; `push`
/// pushes it to `origin` as a new branch, opening no PR.
fn finish_spec(
    base_dir: &Path,
    id: &str,
    spec: &Path,
    opts: &Options,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    if TaskConfig::load(&opts.repo_root)?.sink == Sink::Push {
        let pushed = finish::finish_push(base_dir, id, opts.push_unresolved, probe)?;
        let branch = &pushed.branch;
        writeln!(out, "{id}: pushed {branch} to origin; no PR was opened")?;
        writeln!(
            out,
            "  next: fetch it with: git fetch origin {branch}; then clean up with: sbxm task rm {}",
            spec_flag(spec)
        )?;
        return Ok(());
    }
    let kept = finish::finish_local(base_dir, id, probe)?;
    let branch = &kept.branch;
    writeln!(
        out,
        "{id}: kept branch {branch} in {}; nothing was pushed",
        kept.repo_git.display()
    )?;
    writeln!(
        out,
        "  next: fetch it into your checkout with: git fetch {} {branch}:{branch}",
        path_arg(&kept.repo_git)
    )?;
    Ok(())
}
