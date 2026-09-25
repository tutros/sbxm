//! Host-side state per project, because `sbx ls --json` doesn't expose kits
//! or env.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `<metadata_dir>/state.json`: one entry per harness (decision 41).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub sandboxes: BTreeMap<String, SandboxState>,
}

#[derive(Debug, Serialize, Deserialize)]
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

/// Every project's state under `<base>/.sbxm/`, as `(project, state)`.
/// A missing `.sbxm` dir means no state yet.
pub fn load_all(base_dir: &Path) -> Result<Vec<(String, State)>> {
    let root = base_dir.join(".sbxm");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut states = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("cannot read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path().join("state.json");
        if !path.is_file() {
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let state = serde_json::from_str(&text)
            .with_context(|| format!("invalid state file {}", path.display()))?;
        states.push((entry.file_name().to_string_lossy().into_owned(), state));
    }
    Ok(states)
}
