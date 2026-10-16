//! `sbxm task rm (--issue N | --pr N | --spec FILE | --merged) [--yes] [--force] [--dry-run]`
//! (spec §4, decision 153): removes a task's sandboxes, clones and folder after showing exactly
//! what, and asking. `--merged` (decision 195, issue 202) removes every task whose work is merged
//! instead of naming one.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::disk;
use crate::github::GitHubBackend;
use crate::task::finish;
use crate::task::merged;
use crate::task::record::{self, ProcessProbe};

/// Which task to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    Pr(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
    /// Every task whose work is merged (decision 195, issue 202).
    Merged,
}

pub struct Options {
    pub target: Target,
    /// Skip the question (needed without a terminal).
    pub yes: bool,
    /// Delete a spec task's result even though it exists only in its `repo.git` (issue 143).
    /// Refused together with `Target::Merged` (decision 195 point 3).
    pub force: bool,
    /// The checkout a spec task's result is fetched into.
    pub repo_root: PathBuf,
    /// With `Target::Merged`: list what would be removed and why, and remove nothing.
    pub dry_run: bool,
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<()> {
    if opts.dry_run && opts.target != Target::Merged {
        bail!("--dry-run only makes sense with --merged");
    }
    let config = GlobalConfig::load(config_dir)?;
    let base_dir = config.base_dir.clone();
    let id = match &opts.target {
        Target::Issue(n) => format!("issue-{n}"),
        Target::Pr(n) => format!("pr-{n}"),
        Target::Spec(path) => {
            let id = record::spec_id(path)?.0;
            if !finish::exists(&base_dir, &id) {
                bail!("no task {id}; run `sbxm task status` to list the tasks");
            }
            if !opts.force {
                finish::check_spec_result_fetched(&base_dir, &id, &opts.repo_root)?;
            }
            id
        }
        Target::Merged => {
            let found = merged::plan(&base_dir, &opts.repo_root, github, probe)?;
            if opts.dry_run {
                if found.is_empty() && found.left_alone.is_empty() {
                    writeln!(out, "no merged tasks to remove")?;
                } else {
                    writeln!(out, "{}", found.listing())?;
                }
                return Ok(());
            }
            let state_dir = disk::state_dir(&config, disk::platform_state_dir().as_deref());
            return merged::remove(
                &base_dir,
                &found,
                opts.yes,
                backend,
                confirm,
                |p| fs4::available_space(p),
                state_dir.as_deref(),
                out,
            );
        }
    };
    finish::discard(&base_dir, &id, opts.yes, backend, probe, confirm, out)
}
