use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use super::HARNESS;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::{GlobalConfig, Profile};
use crate::project;
use crate::seed;
use crate::state::{SandboxState, State};

#[derive(Debug, Default)]
pub struct Options {
    /// Directory whose contents are copied into a new workspace.
    pub seed: Option<PathBuf>,
    /// Profile to apply; `default_profile` from the global config if `None`.
    pub profile: Option<String>,
}

pub fn run(
    config_dir: &Path,
    name: &str,
    options: &Options,
    backend: &dyn SandboxBackend,
) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let profile_name = options
        .profile
        .as_deref()
        .unwrap_or(&config.default_profile);
    let _profile = Profile::load(config.profiles_dir(), profile_name)?;

    if !config.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            config.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }
    let workspace = config.base_dir.join(name);
    if let Some(seed_dir) = &options.seed {
        if workspace.exists() {
            bail!(
                "project {} already exists; run without --seed to reuse it",
                workspace.display()
            );
        }
        if !seed_dir.is_dir() {
            bail!("seed {} is not a directory", seed_dir.display());
        }
        let seed_abs = fs::canonicalize(seed_dir)?;
        if fs::canonicalize(&config.base_dir)?.starts_with(&seed_abs) {
            bail!(
                "seed {} contains the base dir {}; copying it would recurse into itself",
                seed_dir.display(),
                config.base_dir.display()
            );
        }
    }
    match &options.seed {
        Some(seed_dir) => seed::copy(seed_dir, &workspace)?,
        None => fs::create_dir_all(&workspace)
            .with_context(|| format!("cannot create {}", workspace.display()))?,
    }

    let sandbox = project::sandbox_name(name, HARNESS);
    backend.create(&CreateSpec {
        name: sandbox.clone(),
        agent: HARNESS.into(),
        workspace: workspace.clone(),
        cpus: config.resources.cpus,
        memory: config.resources.memory,
    })?;

    let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut state = State::default();
    state.sandboxes.insert(
        HARNESS.into(),
        SandboxState {
            sandbox,
            workspace,
            created_at,
        },
    );
    state.save(&project::metadata_dir(&config.base_dir, name))
}
