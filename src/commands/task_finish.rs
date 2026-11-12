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
use crate::task::pipeline;
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
    // Held until the finish's last write (issue 58): a second command on this task refuses.
    let _lock = pipeline::lock_task(&base_dir, &id, probe)?;
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
            return finish_spec(&base_dir, &id, path, opts, github, probe, out);
        }
    };
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let done = finish::finish(&base_dir, &id, github, probe)?;
    let cleanup = format!("--issue {number}");
    if done.summaries_only {
        let comment = crate::title::hint_comment_of(&base_dir, &id);
        return print_summaries_only(out, &id, &done, &cleanup, &comment);
    }
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
        for warning in &done.warnings {
            writeln!(out, "warning: {warning}")?;
        }
        writeln!(
            out,
            "  next: review PR #{pr} again with: {}; after the merge, clean up with: sbxm task rm \
             --issue {}{}",
            done.review_again,
            number,
            crate::title::hint_comment_of(&base_dir, &id)
        )?;
        return Ok(());
    }
    let comment = crate::title::hint_comment_of(&base_dir, &id);
    print_opened(out, &id, &done, &cleanup, &comment)
}

/// What a `finish` of a task already finished with its PR did (issue 430): only the summaries of
/// the review rounds on its issues, posted or edited; nothing was pushed, opened or reviewed.
fn print_summaries_only(
    out: &mut dyn Write,
    id: &str,
    done: &finish::Finished,
    cleanup: &str,
    comment: &str,
) -> Result<()> {
    writeln!(
        out,
        "{id}: already finished with {}; nothing was pushed and no PR was opened or reviewed again",
        done.url
    )?;
    if !done.summarized.is_empty() {
        let issues: Vec<String> = done.summarized.iter().map(|n| format!("#{n}")).collect();
        writeln!(
            out,
            "  posted or updated the summary of the review rounds on issue {}",
            issues.join(", ")
        )?;
    }
    for warning in &done.warnings {
        writeln!(out, "warning: {warning}")?;
    }
    writeln!(
        out,
        "  next: after it is merged, clean up with: sbxm task rm {cleanup}{comment}"
    )?;
    Ok(())
}

/// What a `finish` that opened (or adopted) a PR did, for an issue task or a spec task under the
/// `pr` sink; `cleanup` is the `task rm` flag that removes the task once the PR is merged.
/// `comment` ends the `next:` hint ([`crate::title::hint_comment`]).
fn print_opened(
    out: &mut dyn Write,
    id: &str,
    done: &finish::Finished,
    cleanup: &str,
    comment: &str,
) -> Result<()> {
    let branch = &done.branch;
    // Issue 598: a branch already on origin at the task's tip was not pushed again. After a
    // finish that pushed it and couldn't open its PR, it may be that push, so it isn't "yours".
    let pushed = match (&done.branch_adopted, done.retried) {
        (Some(tip), false) => {
            format!("adopted the branch you pushed: {branch} at {tip}, pushing nothing,")
        }
        (Some(tip), true) => {
            format!("adopted the branch already on origin: {branch} at {tip}, pushing nothing,")
        }
        (None, _) => format!("pushed {branch}"),
    };
    if done.draft {
        writeln!(out, "{id}: {pushed} and opened draft PR {}", done.url)?;
        for reason in &done.unresolved {
            writeln!(out, "  unresolved: {reason}")?;
        }
    } else if done.adopted {
        writeln!(
            out,
            "{id}: {pushed} and found {} already open for it; recorded it as the task's PR",
            done.url
        )?;
    } else {
        writeln!(out, "{id}: {pushed} and opened {}", done.url)?;
    }
    // An adopted PR's description is neither read nor changed (issue 521), so only a PR this
    // finish opened is said to refer to its issue with `Refs #N`.
    for number in &done.left_open {
        if done.adopted {
            writeln!(
                out,
                "  the PR's description was left unchanged (an existing PR was adopted): check \
                 that it refers to #{number} with Refs, not Fixes, or the merge will close it"
            )?;
        } else {
            writeln!(
                out,
                "  the PR leaves #{number} open (Closes on merge: no): its description refers \
                 to it with Refs #{number}"
            )?;
        }
    }
    if let (Some(commit), Some(pr)) = (&done.review_posted, finish::pr_number(&done.url)) {
        writeln!(
            out,
            "  the in-task review of {commit} is posted on PR #{pr} as its review; a later commit \
             on the PR gets: {}",
            done.review_again
        )?;
    } else if let Some(pr) = finish::pr_number(&done.url) {
        writeln!(
            out,
            "  no review covers PR #{pr}'s head, so it has no sbxm/review status; review it \
             with: {}",
            done.review_again
        )?;
    }
    for warning in &done.warnings {
        writeln!(out, "warning: {warning}")?;
    }
    writeln!(
        out,
        "  next: after it is merged, clean up with: sbxm task rm {cleanup}{comment}"
    )?;
    Ok(())
}

/// A spec task goes to the `[finish] sink` named in the checkout's `sbxm-task.toml` (decisions
/// 174(e), 301): `local` keeps the branch in the task's `repo.git` and says how to fetch it;
/// `push` pushes it to `origin` as a new branch, opening no PR; `pr` pushes it and opens a PR
/// with the in-task review, as for an issue task.
fn finish_spec(
    base_dir: &Path,
    id: &str,
    spec: &Path,
    opts: &Options,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    let sink = TaskConfig::load(&opts.repo_root)?.sink;
    if sink == Sink::Pr {
        let flag = spec_flag(spec);
        let done = finish::finish_spec_pr(base_dir, id, &flag, github, probe)?;
        if done.summaries_only {
            let comment = crate::title::hint_comment_of(base_dir, id);
            return print_summaries_only(out, id, &done, &flag, &comment);
        }
        for cut in &done.cuts {
            writeln!(out, "  note: {cut}")?;
        }
        let comment = crate::title::hint_comment_of(base_dir, id);
        return print_opened(out, id, &done, &flag, &comment);
    }
    if sink == Sink::Push {
        let pushed = finish::finish_push(base_dir, id, opts.push_unresolved, probe)?;
        let branch = &pushed.branch;
        writeln!(out, "{id}: pushed {branch} to origin; no PR was opened")?;
        writeln!(
            out,
            "  next: fetch it with: git fetch origin {branch}; then clean up with: sbxm task rm {}{}",
            spec_flag(spec),
            crate::title::hint_comment_of(base_dir, id)
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
        "  next: fetch it into your checkout with: git fetch {} {branch}:{branch}{}",
        path_arg(&kept.repo_git),
        crate::title::hint_comment_of(base_dir, id)
    )?;
    Ok(())
}
