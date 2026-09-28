use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use super::{harness_flag, new, rm};
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::harness::Harness;
use crate::{project, state};

#[derive(Debug, Default)]
pub struct Options {
    /// Recreate the sandbox from the current config.
    pub rebuild: bool,
    /// Which of the project's sandboxes to open.
    pub harness: Harness,
}

/// Attaches to the project's sandbox, creating it first (via `new`) if it
/// doesn't exist (decision 31). Warnings go to `warn`.
pub fn run(
    config_dir: &Path,
    name: &str,
    options: &Options,
    backend: &dyn SandboxBackend,
    warn: &mut dyn Write,
) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let harness = options.harness;
    let sandbox = project::sandbox_name(name, harness.as_str());
    let metadata_dir = project::metadata_dir(&config.base_dir, name);
    let mut state = state::load(&metadata_dir)?.unwrap_or_default();
    let entry = state.sandboxes.remove(harness.as_str());
    let exists = backend.list()?.iter().any(|s| s.name == sandbox);

    match (entry, exists) {
        (Some(entry), true) if options.rebuild => {
            writeln!(
                warn,
                "rebuilding {sandbox}: its session history will be lost; the workspace {} is kept",
                entry.workspace.display()
            )?;
            // `new` removes the old sandbox only once the new kit validates.
            let options = new::Options {
                profile: entry.profile,
                replace: true,
                harness,
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend, warn)?;
            if let Some(old_hash) = &entry.config_hash {
                rm::delete_unused_kit(&metadata_dir, old_hash)?;
            }
        }
        (Some(entry), true) => check_unchanged(&config, name, harness, &sandbox, &entry)?,
        (None, true) => bail!(
            "sandbox {sandbox} exists but sbxm has no state for it; it may lack sbxm's config, \
             so remove it with `sbx rm {sandbox}` (this deletes its session history) and run \
             `sbxm open {name}{}` again",
            harness_flag(harness)
        ),
        // Recreate with the profile it was built from; state from before
        // slice 11 has none, so the default applies.
        (entry, false) => {
            let options = new::Options {
                recreate: entry.is_some(),
                profile: entry.and_then(|e| e.profile),
                harness,
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend, warn)?
        }
    }
    backend.attach(&sandbox)
}

/// Refuses when the config hash differs from the one the sandbox was built
/// with, or when none was recorded (decisions 31, 55).
fn check_unchanged(
    config: &GlobalConfig,
    name: &str,
    harness: Harness,
    sandbox: &str,
    entry: &state::SandboxState,
) -> Result<()> {
    let fix = format!(
        "run `sbxm open {name}{} --rebuild` to recreate it (its session history is lost; \
         the workspace is kept)",
        harness_flag(harness)
    );
    let Some(stored) = &entry.config_hash else {
        bail!("{sandbox} was created before sbxm recorded config hashes; {fix}");
    };
    let profile_name = entry.profile.as_deref().unwrap_or(&config.default_profile);
    if *stored != config.current_hash(name, profile_name, harness)? {
        bail!(
            "the config of {sandbox} (profile '{profile_name}') changed since it was created; {fix}"
        );
    }
    Ok(())
}
