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

    /// [`config_hash`] of `profile_name` merged with `project`'s
    /// `sandbox.toml`, as they are now on disk.
    pub fn current_hash(&self, project: &str, profile_name: &str) -> Result<String> {
        let profile = Profile::load(self.profiles_dir(), profile_name)?
            .with_project(&crate::project::metadata_dir(&self.base_dir, project))?;
        Ok(config_hash(profile_name, &profile, &self.resources))
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
    #[serde(default)]
    pub setup: Setup,
    #[serde(default, skip_serializing)]
    skills: Skills,
    /// `skills.store`, checked by [`Profile::load`]. Hashed instead of the raw
    /// value, so an unset store and `"readonly"` hash the same.
    #[serde(skip_deserializing)]
    pub skills_store: SkillsStore,
    #[serde(default, skip_serializing)]
    instructions: InstructionPaths,
    /// Contents of the `instructions.mandatory` file, read by
    /// [`Profile::load`]. Hashed instead of the path (decision 62).
    #[serde(skip_deserializing)]
    pub mandatory_instructions: Option<String>,
    /// Contents of the `instructions.reference` file, like
    /// `mandatory_instructions`.
    #[serde(skip_deserializing)]
    pub reference_instructions: Option<String>,
}

/// Paths relative to the file that names them (decision 62).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstructionPaths {
    #[serde(default)]
    mandatory: Option<PathBuf>,
    #[serde(default)]
    reference: Option<PathBuf>,
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

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default)]
    pub install: Vec<InstallStep>,
}

/// One `[[setup.install]]` step, run once at create (kit SPEC-v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallStep {
    pub command: String,
    /// `sbx` runs the step as root (`"0"`) when unset.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

impl Profile {
    /// Merges `<metadata_dir>/sandbox.toml` over this profile, if the project
    /// has one (decision 57).
    pub fn with_project(mut self, metadata_dir: &Path) -> Result<Self> {
        let path = metadata_dir.join("sandbox.toml");
        if !path.is_file() {
            return Ok(self);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let project: Self = toml::from_str(&text)
            .with_context(|| format!("invalid project config {}", path.display()))?;
        project.check_env("project config", &path)?;
        self.network.allow.extend(project.network.allow);
        self.network.deny.extend(project.network.deny);
        self.env.extend(project.env);
        self.secrets.services.extend(project.secrets.services);
        self.setup.install.extend(project.setup.install);
        if project.skills.store.is_some() {
            self.skills_store =
                parse_skills_store(project.skills.store.as_deref(), "project config", &path)?;
        }
        // Paths resolve in the project's own folder (decision 62).
        let mandatory = project.instructions.mandatory.as_deref();
        let reference = project.instructions.reference.as_deref();
        let read =
            |key, relative| read_instructions(key, relative, metadata_dir, "project config", &path);
        if let Some(text) = read("mandatory", mandatory)? {
            self.mandatory_instructions = Some(text);
        }
        if let Some(text) = read("reference", reference)? {
            self.reference_instructions = Some(text);
        }
        Ok(self)
    }

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
        let source = format!("profile '{name}'");
        profile.check_env(&source, &path)?;
        profile.skills_store = parse_skills_store(profile.skills.store.as_deref(), &source, &path)?;
        let dir = profiles_dir.join(name);
        profile.mandatory_instructions = read_instructions(
            "mandatory",
            profile.instructions.mandatory.as_deref(),
            &dir,
            &source,
            &path,
        )?;
        profile.reference_instructions = read_instructions(
            "reference",
            profile.instructions.reference.as_deref(),
            &dir,
            &source,
            &path,
        )?;
        Ok(profile)
    }

    /// `sbx` rejects these too, but with messages that blame "the kit's author";
    /// sbxm points at the profile instead.
    fn check_env(&self, source: &str, path: &Path) -> Result<()> {
        for (key, value) in &self.env {
            let mut chars = key.chars();
            let valid = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                bail!(
                    "invalid env name '{key}' in {source} ({}); use letters, digits and '_', \
                     not starting with a digit",
                    path.display()
                );
            }
            if key.starts_with("SBXM_") {
                bail!(
                    "env name '{key}' in {source} uses the reserved prefix SBXM_ \
                     (sbxm sets its own SBXM_ variables); rename it in {}",
                    path.display()
                );
            }
            if value.contains("${{") {
                bail!(
                    "env value for '{key}' in {source} contains '${{{{', which sbx reads as a \
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

/// The contents of `instructions.<key>`, `None` when unset. The path is
/// relative to `dir` (the folder of `file`, which names it) and must stay
/// inside it, so a profile is self-contained (decision 62).
fn read_instructions(
    key: &str,
    relative: Option<&Path>,
    dir: &Path,
    source: &str,
    file: &Path,
) -> Result<Option<String>> {
    use std::path::Component;
    let Some(relative) = relative else {
        return Ok(None);
    };
    let inside = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !inside {
        bail!(
            "instructions.{key} = \"{}\" in {source} must be a relative path inside {}; fix it in {}",
            relative.display(),
            dir.display(),
            file.display()
        );
    }
    let path = dir.join(relative);
    if !path.is_file() {
        bail!(
            "instructions.{key} file {} ({source}) is missing or not a file; create it or fix {}",
            path.display(),
            file.display()
        );
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Some(text))
}

/// `readonly` when unset (decision 46).
fn parse_skills_store(value: Option<&str>, source: &str, path: &Path) -> Result<SkillsStore> {
    match value {
        None | Some("readonly") => Ok(SkillsStore::ReadOnly),
        Some("off") => Ok(SkillsStore::Off),
        Some("readwrite") => bail!(
            "skills.store = \"readwrite\" in {source} is not allowed: an agent could plant \
             skills that every other sandbox loads (decision 46); use \"readonly\" or \"off\" in {}",
            path.display()
        ),
        Some(other) => bail!(
            "skills.store = \"{other}\" in {source} is not valid; use \"readonly\" or \"off\" in {}",
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
