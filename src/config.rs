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
    /// Checked by `doctor`; the version sbxm was verified against when unset.
    #[serde(default = "default_min_sbx_version")]
    pub min_sbx_version: String,
    pub resources: Resources,
}

fn default_profile_name() -> String {
    "default".into()
}

fn default_min_sbx_version() -> String {
    "0.43.0".into()
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
    #[serde(default, skip_serializing)]
    harness: HarnessPaths,
    /// Files under `harness.claude.home_files`, read by [`Profile::load`] and
    /// keyed by `/`-separated relative path. Hashed as path → SHA-256 of the
    /// contents (decision 63).
    #[serde(skip_deserializing, serialize_with = "serialize_file_hashes")]
    pub claude_home_files: BTreeMap<String, Vec<u8>>,
    /// The folder `claude_home_files` came from, for error messages.
    #[serde(skip)]
    claude_home_dir: Option<PathBuf>,
    /// The `harness.claude.managed_settings` JSON object, read by
    /// [`Profile::load`] and re-serialized compactly, so reformatting the file
    /// doesn't change the hash (decision 64).
    #[serde(skip_deserializing)]
    pub claude_managed_settings: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessPaths {
    #[serde(default)]
    claude: ClaudePaths,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudePaths {
    #[serde(default)]
    home_files: Option<PathBuf>,
    #[serde(default)]
    managed_settings: Option<PathBuf>,
}

fn serialize_file_hashes<S: serde::Serializer>(
    files: &BTreeMap<String, Vec<u8>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_map(files.iter().map(|(path, bytes)| (path, sha256_hex(bytes))))
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
        if let Some(relative) = project.harness.claude.home_files.as_deref() {
            let (dir, files) = read_home_files(relative, metadata_dir, "project config", &path)?;
            self.claude_home_dir = Some(dir);
            self.claude_home_files = files;
        }
        if let Some(relative) = project.harness.claude.managed_settings.as_deref() {
            self.claude_managed_settings = Some(read_managed_settings(
                relative,
                metadata_dir,
                "project config",
                &path,
            )?);
        }
        self.check_home_files()?;
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
        if let Some(relative) = profile.harness.claude.home_files.clone() {
            let (home, files) = read_home_files(&relative, &dir, &source, &path)?;
            profile.claude_home_dir = Some(home);
            profile.claude_home_files = files;
        }
        if let Some(relative) = profile.harness.claude.managed_settings.clone() {
            profile.claude_managed_settings =
                Some(read_managed_settings(&relative, &dir, &source, &path)?);
        }
        profile.check_home_files()?;
        Ok(profile)
    }

    /// Home files the Claude kit would replace, or that would replace the
    /// mandatory instructions, are errors: either way something is silently
    /// dropped (decisions 11, 61, 63).
    fn check_home_files(&self) -> Result<()> {
        let Some(dir) = &self.claude_home_dir else {
            return Ok(());
        };
        if self.claude_home_files.contains_key(".claude/settings.json") {
            bail!(
                ".claude/settings.json in harness.claude.home_files ({}) would be replaced by \
                 the Claude kit; remove it, and configure hooks through \
                 harness.claude.managed_settings instead",
                dir.display()
            );
        }
        if self.claude_home_files.contains_key(".claude/CLAUDE.md")
            && self.mandatory_instructions.is_some()
        {
            bail!(
                ".claude/CLAUDE.md in harness.claude.home_files ({}) would replace the \
                 instructions.mandatory file; remove one of them",
                dir.display()
            );
        }
        Ok(())
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
    let input = HashInput::new(profile_name, profile, resources);
    sha256_hex(&serde_json::to_vec(&input).expect("config serializes"))
}

/// What [`config_hash`] hashes, as TOML, so `config show` explains drift
/// (decision 65).
pub fn hash_input_toml(
    profile_name: &str,
    profile: &Profile,
    resources: &Resources,
) -> Result<String> {
    let input = HashInput::new(profile_name, profile, resources);
    toml::to_string(&input).context("cannot render the config as TOML")
}

#[derive(Serialize)]
struct HashInput<'a> {
    sbxm_version: &'a str,
    profile_name: &'a str,
    profile: &'a Profile,
    resources: &'a Resources,
}

impl<'a> HashInput<'a> {
    fn new(profile_name: &'a str, profile: &'a Profile, resources: &'a Resources) -> Self {
        HashInput {
            sbxm_version: env!("CARGO_PKG_VERSION"),
            profile_name,
            profile,
            resources,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
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
    let Some(relative) = relative else {
        return Ok(None);
    };
    let path = resolve_inside(&format!("instructions.{key}"), relative, dir, source, file)?;
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

/// `dir.join(relative)`, where `relative` must stay inside `dir`, so a
/// profile is self-contained (decisions 62, 63).
fn resolve_inside(
    key: &str,
    relative: &Path,
    dir: &Path,
    source: &str,
    file: &Path,
) -> Result<PathBuf> {
    use std::path::Component;
    let inside = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !inside {
        bail!(
            "{key} = \"{}\" in {source} must be a relative path inside {}; fix it in {}",
            relative.display(),
            dir.display(),
            file.display()
        );
    }
    Ok(dir.join(relative))
}

/// The folder named by `harness.claude.home_files` and every file in it,
/// keyed by `/`-separated relative path. Links are refused, since following
/// one could copy files from outside the profile into the sandbox.
fn read_home_files(
    relative: &Path,
    dir: &Path,
    source: &str,
    file: &Path,
) -> Result<(PathBuf, BTreeMap<String, Vec<u8>>)> {
    let root = resolve_inside("harness.claude.home_files", relative, dir, source, file)?;
    if !root.is_dir() {
        bail!(
            "harness.claude.home_files folder {} ({source}) is missing or not a folder; \
             create it or fix {}",
            root.display(),
            file.display()
        );
    }
    let mut files = BTreeMap::new();
    collect_files(&root, "", &mut files)?;
    Ok((root, files))
}

fn collect_files(dir: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let key = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if file_type.is_symlink() {
            bail!(
                "home file {} is a symlink or junction; replace it with a regular file or folder",
                path.display()
            );
        }
        if file_type.is_dir() {
            collect_files(&path, &format!("{key}/"), files)?;
        } else {
            let bytes =
                std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            files.insert(key, bytes);
        }
    }
    Ok(())
}

/// The JSON object in the file named by `harness.claude.managed_settings`,
/// as compact JSON (decision 64).
fn read_managed_settings(relative: &Path, dir: &Path, source: &str, file: &Path) -> Result<String> {
    let key = "harness.claude.managed_settings";
    let path = resolve_inside(key, relative, dir, source, file)?;
    if !path.is_file() {
        bail!(
            "{key} file {} ({source}) is missing or not a file; create it or fix {}",
            path.display(),
            file.display()
        );
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(value.to_string()),
        _ => bail!(
            "{key} file {} ({source}) is not a JSON object; fix it to hold Claude \
             managed settings, e.g. {{\"hooks\": {{…}}}}",
            path.display()
        ),
    }
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
