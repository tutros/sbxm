use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{HARNESS, new, rm};
use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

#[derive(Debug, Default)]
pub struct Options {
    /// Recreate the sandbox from the current config.
    pub rebuild: bool,
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
    let sandbox = project::sandbox_name(name, HARNESS);
    let metadata_dir = project::metadata_dir(&config.base_dir, name);
    let mut state = state::load(&metadata_dir)?.unwrap_or_default();
    let entry = state.sandboxes.remove(HARNESS);
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
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend)?;
            if let Some(old_hash) = &entry.config_hash {
                let in_use = state
                    .sandboxes
                    .values()
                    .any(|other| other.config_hash.as_ref() == Some(old_hash));
                if !in_use {
                    delete_old_kit(&metadata_dir, old_hash)?;
                }
            }
        }
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
        "run `sbxm open {name} --rebuild` to recreate it (its session history is lost; \
         the workspace is kept)"
    );
    let Some(stored) = &entry.config_hash else {
        bail!("{sandbox} was created before sbxm recorded config hashes; {fix}");
    };
    let profile_name = entry.profile.as_deref().unwrap_or(&config.default_profile);
    if *stored != config.current_hash(profile_name)? {
        bail!(
            "the config of {sandbox} (profile '{profile_name}') changed since it was created; {fix}"
        );
    }
    Ok(())
}

/// Deletes `kits/<prefix>` of the hash a rebuilt sandbox was built with,
/// unless the rebuild produced the same prefix (decision 55).
fn delete_old_kit(metadata_dir: &Path, old_hash: &str) -> Result<()> {
    // Read from state.json, so check it before building a path from it.
    let Some(prefix) = old_hash.get(..12) else {
        return Ok(());
    };
    if !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    let current =
        state::load(metadata_dir)?.and_then(|s| s.sandboxes.get(HARNESS)?.config_hash.clone());
    if current.as_deref().and_then(|h| h.get(..12)) == Some(prefix) {
        return Ok(());
    }
    let dir = metadata_dir.join("kits").join(prefix);
    if dir.symlink_metadata().is_err() {
        return Ok(());
    }
    rm::check_deletable(&dir)?;
    fs::remove_dir_all(&dir).with_context(|| format!("cannot delete {}", dir.display()))
}
