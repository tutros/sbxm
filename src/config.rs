use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// `SBXM_CONFIG_DIR` if set, otherwise `~/.config/sbxm` on every platform.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("SBXM_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(home_dir()?.join(".config").join("sbxm"))
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("cannot determine the home directory")
}

/// The global `config.toml`. Only the keys used so far are read.
#[derive(Debug, Deserialize)]
pub struct GlobalConfig {
    pub base_dir: PathBuf,
    pub resources: Resources,
}

#[derive(Debug, Deserialize)]
pub struct Resources {
    pub cpus: u32,
    pub memory: String,
}

impl GlobalConfig {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join("config.toml");
        if !path.exists() {
            bail!(
                "no config at {}; run `sbxm config init` first",
                path.display()
            );
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))
    }
}
