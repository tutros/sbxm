//! `sbxm task rm (--issue N | --pr N) [--yes]` (spec §4, decision 153): removes a task's
//! sandboxes, clones and folder after showing exactly what, and asking.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::confirm::Confirm;
use crate::task::finish;
use crate::task::record::{Kind, ProcessProbe};

pub struct Options {
    pub kind: Kind,
    pub number: u32,
    /// Skip the question (needed without a terminal).
    pub yes: bool,
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
    let id = match opts.kind {
        Kind::Issue => format!("issue-{}", opts.number),
        Kind::Pr => format!("pr-{}", opts.number),
        // Issue 142 is start-only for spec tasks; remove its folders under the base dir by hand.
        Kind::Spec => bail!("sbxm task rm doesn't take a spec task yet"),
    };
    finish::discard(&base_dir, &id, opts.yes, backend, probe, confirm, out)
}
