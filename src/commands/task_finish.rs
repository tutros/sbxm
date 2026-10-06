//! `sbxm task finish --issue N` (spec §4): pushes the task's branch from its `repo.git` and opens
//! the PR, or, for a task that continues an open PR, pushes to that PR's branch and comments on it
//! (decision 169 (e)). The work is in `task::finish`.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::config::GlobalConfig;
use crate::github::GitHubBackend;
use crate::task::finish;
use crate::task::record::ProcessProbe;

pub struct Options {
    pub issue: u32,
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
) -> Result<()> {
    if opts.issue == 0 {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
    let id = format!("issue-{}", opts.issue);
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
        writeln!(
            out,
            "  next: review PR #{pr} again with: sbxm task review --pr {pr} (if it was reviewed before, \
             first sbxm task rm --pr {pr}); after the merge, clean up with: sbxm task rm --issue {}",
            opts.issue
        )?;
        return Ok(());
    }
    writeln!(out, "{id}: pushed {id} and opened {}", done.url)?;
    writeln!(
        out,
        "  next: after it is merged, clean up with: sbxm task rm --issue {}",
        opts.issue
    )?;
    Ok(())
}
