use std::path::Path;

use anyhow::Result;

use crate::config::{self, GlobalConfig, Profile};
use crate::harness::Harness;
use crate::{kit, project, state};

#[derive(Debug, Default)]
pub struct Options {
    /// Profile to show; otherwise the project's recorded one, then
    /// `default_profile`.
    pub profile: Option<String>,
    /// Also print the generated kits.
    pub kits: bool,
    /// Whose hash input and kits to show (decision 69).
    pub harness: Harness,
}

/// The merged config as the hash sees it, plus the hash and optionally the
/// kits `new` would generate (decision 65). Writes nothing and never calls
/// `sbx`.
pub fn render(config_dir: &Path, project: Option<&str>, options: &Options) -> Result<String> {
    let config = GlobalConfig::load(config_dir)?;
    let mut profile_name = options.profile.clone();
    let mut metadata_dir = None;
    if let Some(name) = project {
        project::validate_name(name)?;
        let dir = project::metadata_dir(&config.base_dir, name);
        if profile_name.is_none() {
            profile_name = state::load(&dir)?
                .and_then(|mut s| s.sandboxes.remove(options.harness.as_str()))
                .and_then(|entry| entry.profile);
        }
        metadata_dir = Some(dir);
    }
    let profile_name = profile_name.unwrap_or_else(|| config.default_profile.clone());
    let mut profile = Profile::load(config.profiles_dir(), &profile_name)?;
    if let Some(dir) = &metadata_dir {
        profile = profile.with_project(dir)?;
    }
    let hash = config::config_hash(&profile_name, &profile, &config.resources, options.harness);

    let mut out = format!("# profile: {profile_name}\n");
    if let Some(name) = project {
        out += &format!("# project: {name}\n");
    }
    out += &format!("# config hash: {hash}\n\n");
    out += &config::hash_input_toml(&profile_name, &profile, &config.resources, options.harness)?;
    if options.kits {
        for (name, spec) in kit::all(&profile_name, &profile, &hash, options.harness) {
            out += &format!("\n# kit: {name}\n");
            out += &spec.yaml()?;
            for path in spec.home_paths() {
                out += &format!("# files/home/{path}\n");
            }
        }
    }
    Ok(out)
}
