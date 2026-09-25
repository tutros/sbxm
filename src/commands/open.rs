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
    let entry = state::load(&project::metadata_dir(&config.base_dir, name))?
        .and_then(|mut s| s.sandboxes.remove(HARNESS));
    let exists = backend.list()?.iter().any(|s| s.name == sandbox);

    match (entry, exists) {
        (Some(_), true) => {}
        (None, true) => bail!(
            "sandbox {sandbox} exists but sbxm has no state for it; it may lack sbxm's config, \
             so remove it with `sbx rm {sandbox}` (this deletes its session history) and run \
             `sbxm open {name}` again"
        ),
        // Recreate with the profile it was built from; state from before
        // slice 11 has none, so the default applies.
        (entry, false) => {
            let options = new::Options {
                profile: entry.and_then(|e| e.profile),
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend)?
        }
    }
    backend.attach(&sandbox)
}
