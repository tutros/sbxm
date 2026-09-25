use std::path::Path;

use anyhow::{Result, bail};

use super::{HARNESS, new};
use crate::backend::SandboxBackend;
use crate::config::{self, GlobalConfig, Profile};
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
        (Some(entry), true) => check_unchanged(&config, name, &sandbox, &entry)?,
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

/// Refuses when the config hash differs from the one the sandbox was built
/// with, or when none was recorded (decisions 31, 55).
fn check_unchanged(
    config: &GlobalConfig,
    name: &str,
    sandbox: &str,
    entry: &state::SandboxState,
) -> Result<()> {
    let fix = format!(
        "run `sbxm open {name} --rebuild` to recreate it (its session history is lost;          the workspace is kept)"
    );
    let Some(stored) = &entry.config_hash else {
        bail!("{sandbox} was created before sbxm recorded config hashes; {fix}");
    };
    let profile_name = entry.profile.as_deref().unwrap_or(&config.default_profile);
    let profile = Profile::load(config.profiles_dir(), profile_name)?;
    if *stored != config::config_hash(profile_name, &profile, &config.resources) {
        bail!(
            "the config of {sandbox} (profile '{profile_name}') changed since it was created; {fix}"
        );
    }
    Ok(())
}
