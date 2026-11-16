//! Reading a file an agent wrote in its workspace, trusting nothing about it.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// What reading a file the agent wrote found.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentFile {
    Missing,
    /// Bigger than the cap; not read.
    TooLarge(u64),
    Text(String),
}

/// Reads `<workspace>/<folder>/<name>` as text (lossily), at most `cap` bytes; never follows a
/// link out of the workspace: e.g. a task's `.sbxm-task` files, or the Jev evaluator's
/// `response.json` (decision 241).
pub fn read_file_in(workspace: &Path, folder: &str, name: &str, cap: u64) -> Result<AgentFile> {
    use std::io::Read;

    let Some((file, len)) = open_file_in(workspace, folder, name)? else {
        return Ok(AgentFile::Missing);
    };
    if len > cap {
        return Ok(AgentFile::TooLarge(len));
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Ok(AgentFile::TooLarge(bytes.len() as u64));
    }
    Ok(AgentFile::Text(
        String::from_utf8_lossy(&bytes).into_owned(),
    ))
}

/// Opens `<workspace>/<folder>/<name>` for reading, `folder` being one plain name, trusting
/// nothing about it: the folder must resolve to a real folder inside the workspace (not a link out
/// of it) and the file must be a plain file, checked on the open handle. `Ok(None)` when the
/// folder or file doesn't exist.
pub(crate) fn open_file_in(
    workspace: &Path,
    folder_name: &str,
    name: &str,
) -> Result<Option<(std::fs::File, u64)>> {
    use std::io::ErrorKind::NotFound;

    let folder = workspace.join(folder_name);
    let real_workspace = std::fs::canonicalize(workspace)
        .with_context(|| format!("cannot read the workspace {}", workspace.display()))?;
    let real_folder = match std::fs::canonicalize(&folder) {
        Ok(path) => path,
        Err(e) if e.kind() == NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", folder.display())),
    };
    if real_folder != real_workspace.join(folder_name) {
        bail!(
            "the folder {} doesn't resolve to a folder inside the workspace; refusing to read it",
            folder.display()
        );
    }
    let path = real_folder.join(name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let not_plain = || {
        anyhow::anyhow!(
            "{} is not a plain file; refusing to read it",
            path.display()
        )
    };
    if !meta.file_type().is_file() {
        return Err(not_plain());
    }
    let file =
        std::fs::File::open(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(not_plain());
    }
    Ok(Some((file, opened.len())))
}
