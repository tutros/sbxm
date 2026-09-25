use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::GlobalConfig;
use crate::project;

/// Only Claude until `--harness` arrives (milestone 1, slice 18).
const HARNESS: &str = "claude";

pub fn run(config_dir: &Path, name: &str, backend: &dyn SandboxBackend) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;

    if !config.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            config.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }
    let workspace = config.base_dir.join(name);
    fs::create_dir_all(&workspace)
        .with_context(|| format!("cannot create {}", workspace.display()))?;

    backend.create(&CreateSpec {
        name: project::sandbox_name(name, HARNESS),
        agent: HARNESS.into(),
        workspace,
        cpus: config.resources.cpus,
        memory: config.resources.memory,
    })
}
