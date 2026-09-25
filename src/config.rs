use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backend::SkillsStore;

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

#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Left out of the hash: it never reaches the sandbox (decision 55).
    #[serde(default, skip_serializing)]
    pub description: String,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: Secrets,
    #[serde(default, skip_serializing)]
    skills: Skills,
    /// `skills.store`, checked by [`Profile::load`]. Hashed instead of the raw
    /// value, so an unset store and `"readonly"` hash the same.
    #[serde(skip_deserializing)]
    pub skills_store: SkillsStore,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Skills {
    #[serde(default)]
    store: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
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
        let mut profile: Self =
            toml::from_str(&text).with_context(|| format!("invalid profile {}", path.display()))?;
        profile.check_env(name, &path)?;
        profile.skills_store = parse_skills_store(profile.skills.store.as_deref(), name, &path)?;
        Ok(profile)
    }

    /// `sbx` rejects these too, but with messages that blame "the kit's author";
    /// sbxm points at the profile instead.
    fn check_env(&self, name: &str, path: &Path) -> Result<()> {
        for (key, value) in &self.env {
            let mut chars = key.chars();
            let valid = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                bail!(
                    "invalid env name '{key}' in profile '{name}' ({}); use letters, digits and '_', \
                     not starting with a digit",
                    path.display()
                );
            }
            if key.starts_with("SBXM_") {
                bail!(
                    "env name '{key}' in profile '{name}' uses the reserved prefix SBXM_ \
                     (sbxm sets its own SBXM_ variables); rename it in {}",
                    path.display()
                );
            }
            if value.contains("${{") {
                bail!(
                    "env value for '{key}' in profile '{name}' contains '${{{{', which sbx reads as a \
                     kit expression; remove it from {}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

/// SHA-256 (lowercase hex) over canonical JSON of everything that shapes the
/// sandbox: profile name and settings, resources and sbxm's version
/// (decision 55). Maps are `BTreeMap`s, so the JSON is deterministic.
pub fn config_hash(profile_name: &str, profile: &Profile, resources: &Resources) -> String {
    #[derive(Serialize)]
    struct Input<'a> {
        sbxm_version: &'a str,
        profile_name: &'a str,
        profile: &'a Profile,
        resources: &'a Resources,
    }
    let json = serde_json::to_vec(&Input {
        sbxm_version: env!("CARGO_PKG_VERSION"),
        profile_name,
        profile,
        resources,
    })
    .expect("config serializes");
    Sha256::digest(json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `readonly` when unset (decision 46).
fn parse_skills_store(value: Option<&str>, name: &str, path: &Path) -> Result<SkillsStore> {
    match value {
        None | Some("readonly") => Ok(SkillsStore::ReadOnly),
        Some("off") => Ok(SkillsStore::Off),
        Some("readwrite") => bail!(
            "skills.store = \"readwrite\" in profile '{name}' is not allowed: an agent could plant \
             skills that every other sandbox loads (decision 46); use \"readonly\" or \"off\" in {}",
            path.display()
        ),
        Some(other) => bail!(
            "skills.store = \"{other}\" in profile '{name}' is not valid; use \"readonly\" or \"off\" in {}",
            path.display()
        ),
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
