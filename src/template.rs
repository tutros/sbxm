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
    /// Written to a temporary file and renamed over the old record, so a failed write never
    /// loses the tags the old one names (slice 4 review S-2).
    pub fn save(&self, base_dir: &Path, key: &str) -> Result<()> {
        let dir = dir(base_dir);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("{key}.json"));
        let tmp = dir.join(format!("{key}.json.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }
}

/// Who still records `tag` as their template (decision 205): `sandbox <name>` for each
/// `state.json` entry and `task <id>` for each task whose worker or reviewer has it.
pub fn users(base_dir: &Path, tag: &str) -> Result<Vec<String>> {
    let mut users = Vec::new();
    for (_, state) in crate::state::load_all(base_dir)? {
        for sandbox in state.sandboxes.values() {
            if sandbox.template.as_deref() == Some(tag) {
                users.push(format!("sandbox {}", sandbox.sandbox));
            }
        }
    }
    for task in crate::task::record::load_all(base_dir)? {
        let agents = [&task.worker, &task.reviewer];
        if agents
            .iter()
            .any(|a| a.as_ref().and_then(|a| a.template.as_deref()) == Some(tag))
        {
            users.push(format!("task {}", task.id));
        }
    }
    Ok(users)
}

impl Record {
    /// The record of `key`, or `None` if there is none.
    pub fn load(base_dir: &Path, key: &str) -> Result<Option<Self>> {
        let path = dir(base_dir).join(format!("{key}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).map(Some).with_context(|| {
            format!(
                "template record {} is not valid; delete it, then remove the sbxm- templates it \
                 named with `sbx template ls` and `sbx template rm`",
                path.display()
            )
        })
    }
}

/// The combined SHA-256 of `files` (paths relative to `repo`), in order (decision 207).
/// Each file's path and length go in before its contents, so moving bytes between files changes
/// the hash.
pub fn lock_hash(repo: &Path, files: &[String]) -> Result<String> {
    let mut bytes = Vec::new();
    for file in files {
        let path = repo.join(file);
        let contents =
            fs::read(&path).with_context(|| format!("cannot read lock file {}", path.display()))?;
        bytes.extend_from_slice(file.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&contents);
    }
    Ok(crate::config::sha256_hex(&bytes))
}
