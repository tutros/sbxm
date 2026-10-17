//! Prebuilt `sbx` templates (milestone 3): which harnesses support them and the `sbx` version
//! they need.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::harness::Harness;
use crate::run::results::format_timestamp;

/// The `sbx` version templates were tested with (spike S11); older is unsupported (decision 213).
/// The global `min_sbx_version` stays for everything else.
pub const MIN_SBX_VERSION: &str = "0.46.0";

/// `claude` and `codex` (decisions 204, 217) and `pi` (spike S15); the rest are refused.
pub fn check_supported(harness: Harness) -> Result<()> {
    if !matches!(harness, Harness::Claude | Harness::Codex | Harness::Pi) {
        bail!(
            "templates are not supported for the {} harness yet; build one for claude, codex or pi",
            harness.as_str()
        );
    }
    Ok(())
}

/// Refuses an `sbx` older than [`MIN_SBX_VERSION`].
pub fn check_sbx_version(version: &str) -> Result<()> {
    if crate::commands::doctor::older(version, MIN_SBX_VERSION) {
        bail!(
            "sbx {version} is older than {MIN_SBX_VERSION}, the version templates need; upgrade sbx"
        );
    }
    Ok(())
}

/// `<base>/.sbxm/templates`, where records, build locks and build leftovers live.
pub fn dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("templates")
}

/// `<profile>-<harness>`, the stem of a template's record, build sandbox and build folders.
pub fn key(profile: &str, harness: Harness) -> String {
    format!("{profile}-{}", harness.as_str())
}

/// `sbxm-<profile>-<harness>:<hash12>-<yyyymmdd>t<hhmm>` in UTC (decision 211).
pub fn tag(profile: &str, harness: Harness, template_hash: &str, now: u64) -> String {
    let stamp: String = format_timestamp(now)
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'T')
        .take(13)
        .collect();
    format!(
        "sbxm-{}:{}-{}",
        key(profile, harness),
        &template_hash[..12],
        stamp.to_ascii_lowercase()
    )
}

/// `<base>/.sbxm/templates/<profile>-<harness>.json` (decision 206).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub tag: String,
    pub template_hash: String,
    pub warm: bool,
    pub lock_files: Vec<String>,
    pub lock_hash: Option<String>,
    pub sbx_version: String,
    pub size_bytes: u64,
    pub created_at: String,
    /// Earlier tags kept because a sandbox still used them (decision 205a).
    pub previous: Vec<String>,
}

impl Record {
    pub fn save(&self, base_dir: &Path, key: &str) -> Result<()> {
        let dir = dir(base_dir);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("{key}.json"));
        fs::write(&path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", path.display()))
    }
}
