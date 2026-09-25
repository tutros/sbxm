//! All `sbx` interaction goes through [`SandboxBackend`] (decision 17).

mod fake;
mod sbx;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use fake::FakeBackend;
pub use sbx::SbxBackend;

pub trait SandboxBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()>;
    fn list(&self) -> Result<Vec<SandboxInfo>>;
    fn stop(&self, name: &str) -> Result<()>;
    /// Removes the sandbox without prompting (`sbx rm -f`).
    fn remove(&self, name: &str) -> Result<()>;
    /// Attaches the terminal to the sandbox's agent, starting it if stopped.
    fn attach(&self, name: &str) -> Result<()>;
    /// Checks a kit directory (`sbx kit validate --json`).
    fn validate_kit(&self, dir: &Path) -> Result<KitValidation>;
}

/// Result of `sbx kit validate --json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KitValidation {
    pub valid: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Shown to the user as-is; the shape isn't documented.
    #[serde(default)]
    pub warnings: Vec<serde_json::Value>,
}

/// One entry of `sbx ls --json`; other fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxInfo {
    pub name: String,
    pub agent: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSpec {
    pub name: String,
    pub agent: String,
    pub workspace: PathBuf,
    pub cpus: u32,
    pub memory: String,
    /// Always passed explicitly: without `--skills`, `sbx` falls back to its
    /// own `skills.defaultMode` setting, which could be `readwrite`.
    pub skills: SkillsStore,
    /// Mixin kit directories, passed as `--kit` in this order.
    pub kits: Vec<PathBuf>,
}

/// How `sbx`'s shared skills store is mounted (decision 46). sbxm never uses
/// `readwrite`: an agent could plant skills that every other sandbox loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillsStore {
    #[default]
    ReadOnly,
    Off,
}

impl SkillsStore {
    pub fn as_arg(self) -> &'static str {
        match self {
            SkillsStore::ReadOnly => "readonly",
            SkillsStore::Off => "off",
        }
    }
}
