//! `sbxm task rm (--issue N | --pr N | --spec FILE) [--yes] [--force]` (spec §4, decision 153):
//! removes a task's sandboxes, clones and folder after showing exactly what, and asking.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::task::finish;
use crate::task::record::{self, ProcessProbe};

/// Which task to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Issue(u32),
    Pr(u32),
    /// A spec task's file (decision 174(d)).
    Spec(PathBuf),
}

pub struct Options {
    pub target: Target,
    /// Skip the question (needed without a terminal).
    pub yes: bool,
    /// Delete a spec task's result even though it exists only in its `repo.git` (issue 143).
    pub force: bool,
    /// The checkout a spec task's result is fetched into.
    pub repo_root: PathBuf,
}

pub fn run(
    config_dir: &Path,
    opts: &Options,
    backend: &dyn SandboxBackend,
    probe: &dyn ProcessProbe,
    confirm: &dyn Confirm,
    out: &mut dyn Write,
) -> Result<()> {
    let base_dir = GlobalConfig::load(config_dir)?.base_dir;
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
    };
    finish::discard(&base_dir, &id, opts.yes, backend, probe, confirm, out)
}
