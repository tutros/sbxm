use std::path::Path;

use anyhow::Result;

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::{project, state};

/// One row of `sbxm list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub project: String,
    pub harness: String,
    pub sandbox: String,
    pub status: String,
    pub problem: Option<Problem>,
}

/// An orphan: `sbx` and sbxm's state disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The sandbox exists but sbxm has no state for it.
    NoState,
    /// sbxm has state but the sandbox is gone.
    NoSandbox,
}

pub fn entries(config_dir: &Path, backend: &dyn SandboxBackend) -> Result<Vec<Entry>> {
    let config = GlobalConfig::load(config_dir)?;
    let mut known: Vec<(String, String, String)> = Vec::new();
    for (project, state) in state::load_all(&config.base_dir)? {
        for (harness, sandbox) in state.sandboxes {
            known.push((project.clone(), harness, sandbox.sandbox));
        }
    }

    let mut entries = Vec::new();
    for sandbox in backend.list()? {
        let Some((project, harness)) = project::parse_sandbox_name(&sandbox.name) else {
            continue;
        };
        let has_state = known.iter().any(|(_, _, name)| *name == sandbox.name);
        entries.push(Entry {
            project,
            harness,
            sandbox: sandbox.name,
            status: sandbox.status,
            problem: (!has_state).then_some(Problem::NoState),
        });
    }
    for (project, harness, sandbox) in known {
        if !entries.iter().any(|e| e.sandbox == sandbox) {
            entries.push(Entry {
                project,
                harness,
                sandbox,
                status: "missing".into(),
                problem: Some(Problem::NoSandbox),
            });
        }
    }
    Ok(entries)
}
