use std::path::Path;

use anyhow::{Result, bail};

use super::HARNESS;
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

pub fn run(config_dir: &Path, name: &str, backend: &dyn SandboxBackend) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let state = state::load(&project::metadata_dir(&config.base_dir, name))?;
    let Some(sandbox) = state.as_ref().and_then(|s| s.sandboxes.get(HARNESS)) else {
        bail!("no sbxm sandbox for project '{name}'; create one with `sbxm new {name}`");
    };
    backend.stop(&sandbox.sandbox)
}
