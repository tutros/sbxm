//! Host-side state per project, because `sbx ls --json` doesn't expose kits
//! or env.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// `<metadata_dir>/state.json`: one entry per harness (decision 41).
#[derive(Debug, Default, Serialize)]
pub struct State {
    pub sandboxes: BTreeMap<String, SandboxState>,
}

#[derive(Debug, Serialize)]
pub struct SandboxState {
    pub sandbox: String,
    pub workspace: PathBuf,
    /// Unix seconds.
    pub created_at: u64,
}

impl State {
    pub fn save(&self, metadata_dir: &Path) -> Result<()> {
        fs::create_dir_all(metadata_dir)
            .with_context(|| format!("cannot create {}", metadata_dir.display()))?;
        let path = metadata_dir.join("state.json");
        fs::write(&path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", path.display()))
    }
}
