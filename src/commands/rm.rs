use std::path::Path;

use anyhow::{Result, bail};

use super::HARNESS;
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

/// Removes the project's sandbox and its state. The workspace is kept.
pub fn run(config_dir: &Path, name: &str, backend: &dyn SandboxBackend) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let metadata_dir = project::metadata_dir(&config.base_dir, name);
    let state = state::load(&metadata_dir)?;
    let Some(sandbox) = state.as_ref().and_then(|s| s.sandboxes.get(HARNESS)) else {
        bail!("no sbxm sandbox for project '{name}'; `sbxm list` shows existing ones");
    };
    backend.remove(&sandbox.sandbox)?;
    state::remove_sandbox(&metadata_dir, HARNESS)
}
