use std::path::PathBuf;

use anyhow::{Context, Result};

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
