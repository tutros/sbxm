use std::collections::BTreeMap;
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
    /// Defaults to `<config_dir>/profiles`; see [`GlobalConfig::profiles_dir`].
    #[serde(default)]
    profiles_dir: Option<PathBuf>,
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
    pub resources: Resources,
}

fn default_profile_name() -> String {
    "default".into()
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
        let mut config: Self =
            toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))?;
        config
            .profiles_dir
            .get_or_insert_with(|| config_dir.join("profiles"));
        Ok(config)
    }

    pub fn profiles_dir(&self) -> &Path {
        self.profiles_dir
            .as_deref()
            .expect("profiles_dir is set by GlobalConfig::load")
    }
}

/// `<profiles_dir>/<name>/profile.toml`. Unknown keys are errors, so a typo
/// (or a section a later slice will support) is never silently ignored
/// (decision 11).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: Secrets,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    #[serde(default)]
    pub services: Vec<String>,
}

impl Profile {
    pub fn load(profiles_dir: &Path, name: &str) -> Result<Self> {
        validate_profile_name(name)?;
        let path = profiles_dir.join(name).join("profile.toml");
        if !path.is_file() {
            bail!(
                "profile '{name}' not found at {}; create it or pick another with --profile",
                path.display()
            );
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid profile {}", path.display()))
    }
}

/// Profile names become directory names, so they use the project-name
/// characters (`default` is allowed here).
fn validate_profile_name(name: &str) -> Result<()> {
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if name.is_empty() || name.len() > 40 || name.starts_with('-') || !name.chars().all(allowed) {
        bail!(
            "invalid profile name '{name}': use 1-40 lowercase letters, digits and '-', \
             starting with a letter or digit"
        );
    }
    Ok(())
}
