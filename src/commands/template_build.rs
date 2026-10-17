//! `sbxm template build <profile>` (decisions 201, 206, 209-213).

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::{GlobalConfig, Profile};
use crate::disk;
use crate::harness::Harness;
use crate::template;

#[derive(Debug, Default)]
pub struct Options {
    pub profile: String,
    pub harness: Harness,
    /// The free-space check of the sandbox drive (decision 194).
    pub disk: disk::Guard,
}

pub fn run(
    config_dir: &Path,
    options: &Options,
    backend: &dyn SandboxBackend,
    _out: &mut dyn Write,
) -> Result<()> {
    template::check_supported(options.harness)?;
    let config = GlobalConfig::load(config_dir)?;
    let profile = Profile::load(config.profiles_dir(), &options.profile)?;
    if profile.template.is_none() {
        bail!(
            "profile '{}' has no [template] table; add one with its runtime allow list to {} to \
             opt in",
            options.profile,
            config
                .profiles_dir()
                .join(&options.profile)
                .join("profile.toml")
                .display()
        );
    }
    template::check_sbx_version(&backend.version()?)?;
    disk::require(&config, config_dir, &options.disk)?;
    bail!("template build is not implemented yet")
}
