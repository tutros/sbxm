//! Copying a seed directory into a new workspace.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Copies the contents of `seed` into `dest`, which must not exist yet.
pub fn copy(seed: &Path, dest: &Path) -> Result<()> {
    fs::create_dir(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    for entry in fs::read_dir(seed).with_context(|| format!("cannot read {}", seed.display()))? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}
