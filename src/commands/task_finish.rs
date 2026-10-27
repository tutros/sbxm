//! `sbxm task finish (--issue N | --spec FILE)` (spec §4): pushes the task's branch from its `repo.git` and opens
//! the PR, or, for a task that continues an open PR, pushes to that PR's branch and comments on it
//! (decision 169 (e)). The work is in `task::finish`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::backend::{Location, SandboxBackend};
use crate::commands::task_start::{path_arg, spec_flag};
use crate::config::GlobalConfig;
use crate::github::GitHubBackend;
use crate::task::config::{Sink, TaskConfig};
use crate::task::finish;
use crate::task::lifetime::{self, Released};
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
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    run_with_sleep(
        config_dir,
        opts,
        backend,
        github,
        probe,
        out,
        &std::thread::sleep,
    )
}

/// [`run`], with `sleep` called for each wait between a cloud sandbox's removal retries (P6 (a))
/// instead of really sleeping. The finish itself is host-side; once it is done, any cloud sandbox
/// the task's record still holds open (its removal at the task's stop failed) is removed, then
/// its scoped secrets (milestone 4 slice 6). What that says is printed; a sandbox that still
/// can't be removed doesn't fail the finish.
#[allow(clippy::too_many_arguments)]
pub fn run_with_sleep(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    sleep: &dyn Fn(Duration),
) -> Result<()> {
    let id = match &opts.target {
        Target::Issue(0) => bail!("issue numbers start at 1; pass --issue <n>"),
        Target::Issue(n) => format!("issue-{n}"),
        Target::Spec(path) => record::spec_id(path)?.0,
    };
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    // Read and checked before anything is published (check before acting).
    let recorded = match record::is_valid_id(&id) {
        true => record::read(&record::task_dir(&base_dir, &id).join("task.json")).ok(),
        false => None,
    };
    let open = recorded
        .as_ref()
        .map(finish::open_cloud_sandboxes)
        .transpose()?
        .unwrap_or_default();
    // An ambiguous create (decision 268) isn't the task's to remove: the finish leaves it, and
    // its record keeps it for `task rm` to refuse over.
    let ambiguous = recorded
        .as_ref()
        .and_then(|task| task.ambiguous_create().map(str::to_owned));
    publish(config_dir, opts, github, probe, out)?;
    if let Some(name) = &ambiguous {
        writeln!(
            out,
            "warning: {}",
            lifetime::ambiguous_create(name, lifetime::FORGET_BY_RM)
        )?;
    }
    if open.is_empty() {
        return Ok(());
    }
    let meta = record::task_dir(&base_dir, &id);
    let mut task = match record::read(&meta.join("task.json")) {
        Ok(task) => task,
        Err(e) => {
            for name in &open {
                writeln!(
                    out,
                    "warning: {}",
                    lifetime::not_removed(name, &format!("task.json couldn't be read: {e:#}"))
                )?;
            }
            return Ok(());
        }
    };
    let cloud = backend.located(Location::Cloud);
    for said in lifetime::release_open(cloud, &meta, &mut task, &open, sleep) {
        match said {
            Released::Line(line) => writeln!(out, "{line}")?,
            Released::Warning(warning) => writeln!(out, "warning: {warning}")?,
        }
    }
    Ok(())
}

/// The finish itself: host-side only (`repo.git`, GitHub).
fn publish(
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
            "  next: review PR #{pr} again with: {}; after the merge, clean up with: sbxm task rm \
             --issue {}",
            done.review_again, number
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
    if let (Some(commit), Some(pr)) = (&done.review_posted, finish::pr_number(&done.url)) {
        writeln!(
            out,
            "  the in-task review of {commit} is posted on PR #{pr} as its review; a later commit \
             on the PR gets: {}",
            done.review_again
        )?;
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
