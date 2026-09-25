use std::path::Path;

use anyhow::{Result, bail};

use super::{HARNESS, new};
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

/// Attaches to the project's sandbox, creating it first (via `new`) if it
/// doesn't exist (decision 31).
pub fn run(config_dir: &Path, name: &str, backend: &dyn SandboxBackend) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let sandbox = project::sandbox_name(name, HARNESS);
    let has_state = state::load(&project::metadata_dir(&config.base_dir, name))?
        .is_some_and(|s| s.sandboxes.contains_key(HARNESS));
    let exists = backend.list()?.iter().any(|s| s.name == sandbox);

    match (has_state, exists) {
        (true, true) => {}
        (false, true) => bail!(
            "sandbox {sandbox} exists but sbxm has no state for it; it may lack sbxm's config, \
             so remove it with `sbx rm {sandbox}` (this deletes its session history) and run \
             `sbxm open {name}` again"
        ),
        (_, false) => new::run(config_dir, name, &new::Options::default(), backend)?,
    }
    backend.attach(&sandbox)
}
