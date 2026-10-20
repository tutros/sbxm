use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use super::{new, open_harness_flag, rm};
use crate::backend::SandboxBackend;
use crate::config::{self, GlobalConfig, Profile};
use crate::disk;
use crate::harness::Harness;
use crate::{project, state, template};

#[derive(Debug, Default)]
pub struct Options {
    /// Recreate the sandbox from the current config.
    pub rebuild: bool,
    /// Which of the project's sandboxes to open.
    pub harness: Harness,
    /// The free-space check of the sandbox drive, when `open` creates or rebuilds (decision 194).
    pub disk: disk::Guard,
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
    // Checked here too, not only by `new`, so a refused rebuild doesn't first warn about it.
    if options.rebuild || !exists {
        disk::require(&config, config_dir, &options.disk)?;
    }

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
                disk: options.disk.clone(),
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend, warn)?;
            if let Some(old_hash) = &entry.config_hash {
                rm::delete_unused_kit(&metadata_dir, old_hash)?;
            }
        }
        (Some(entry), true) => {
            let (profile_name, base, profile) =
                check_unchanged(&config, name, harness, &sandbox, &entry)?;
            super::write_unsupported_warnings(harness, &profile, &sandbox, warn)?;
            let request = template::Request {
                base_dir: &config.base_dir,
                profile_name: &profile_name,
                profile: &base,
                merged: &profile,
                project_config: &metadata_dir.join("sandbox.toml"),
                harness,
                sandbox: &sandbox,
                locks: template::Locks::Dir(&entry.workspace),
            };
            if entry.template.is_none() && template::ready(&request, backend)? {
                writeln!(
                    warn,
                    "hint: the template of profile '{profile_name}' for {} is ready; `sbxm open \
                     {name}{} --rebuild` recreates {sandbox} from it (its session history is lost; \
                     the workspace is kept)",
                    harness.as_str(),
                    open_harness_flag(harness)
                )?;
            }
        }
        (None, true) => bail!(
            "sandbox {sandbox} exists but sbxm has no state for it; it may lack sbxm's config, \
             so remove it with `sbx rm {sandbox}` (this deletes its session history) and run \
             `sbxm open {name}{}` again",
            open_harness_flag(harness)
        ),
        // Recreate with the profile it was built from; state from before
        // slice 11 has none, so the default applies.
        (entry, false) => {
            let options = new::Options {
                recreate: entry.is_some(),
                profile: entry.and_then(|e| e.profile),
                harness,
                disk: options.disk.clone(),
                ..new::Options::default()
            };
            new::run(config_dir, name, &options, backend, warn)?
        }
    }
    backend.attach(&sandbox)
}

/// Refuses when the config hash differs from the one the sandbox was built
/// with, or when none was recorded (decisions 31, 55); a sandbox from a
/// template is compared with that variant's hash (decision 208). Otherwise
/// returns the profile's name and the profile it was built from, without and
/// with the project's `sandbox.toml`, so the caller can warn about anything
/// this harness can't apply before attaching (decisions 11, 78).
fn check_unchanged(
    config: &GlobalConfig,
    name: &str,
    harness: Harness,
    sandbox: &str,
    entry: &state::SandboxState,
) -> Result<(String, Profile, Profile)> {
    let fix = format!(
        "run `sbxm open {name}{} --rebuild` to recreate it (its session history is lost; \
         the workspace is kept)",
        open_harness_flag(harness)
    );
    let Some(stored) = &entry.config_hash else {
        bail!("{sandbox} was created before sbxm recorded config hashes; {fix}");
    };
    let profile_name = entry.profile.as_deref().unwrap_or(&config.default_profile);
    let base = Profile::load(config.profiles_dir(), profile_name)?;
    let profile = base
        .clone()
        .with_project(&project::metadata_dir(&config.base_dir, name))?;
    let hash = match entry.template {
        Some(_) => config::template_config_hash,
        None => config::config_hash,
    };
    if *stored != hash(profile_name, &profile, &config.resources, harness) {
        bail!(
            "the config of {sandbox} (profile '{profile_name}') changed since it was created; {fix}"
        );
    }
    Ok((profile_name.to_owned(), base, profile))
}
