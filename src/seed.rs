//! Copying a seed directory into a new workspace.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Copies the contents of `seed` into `dest`, which must not exist yet.
/// Refuses before writing anything if the seed contains a link, since
/// following it could copy files from outside the seed into the sandbox.
pub fn copy(seed: &Path, dest: &Path) -> Result<()> {
    reject_links(seed)?;
    copy_tree(seed, dest)
}

/// Fails naming the first symlink or junction under `dir`. `new` calls it
/// with the other seed checks, before anything is written (decision 47).
pub fn reject_links(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "seed entry {} is a symlink or junction; remove it or copy the seed without links",
                entry.path().display()
            );
        }
        if file_type.is_dir() {
            reject_links(&entry.path())?;
        }
    }
    Ok(())
}

fn copy_tree(seed: &Path, dest: &Path) -> Result<()> {
    fs::create_dir(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    for entry in fs::read_dir(seed).with_context(|| format!("cannot read {}", seed.display()))? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}
