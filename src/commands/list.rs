use std::path::Path;

use anyhow::Result;

use crate::backend::SandboxBackend;
use crate::project;

/// One row of `sbxm list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub project: String,
    pub harness: String,
    pub sandbox: String,
    pub status: String,
    pub problem: Option<Problem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {}

pub fn entries(_config_dir: &Path, backend: &dyn SandboxBackend) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for sandbox in backend.list()? {
        let Some((project, harness)) = project::parse_sandbox_name(&sandbox.name) else {
            continue;
        };
        entries.push(Entry {
            project,
            harness,
            sandbox: sandbox.name,
            status: sandbox.status,
            problem: None,
        });
    }
    Ok(entries)
}
