//! All `sbx` interaction goes through [`SandboxBackend`] (decision 17).

mod fake;
mod sbx;

use std::path::PathBuf;

use anyhow::Result;

pub use fake::FakeBackend;
pub use sbx::SbxBackend;

pub trait SandboxBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSpec {
    pub name: String,
    pub agent: String,
    pub workspace: PathBuf,
    pub cpus: u32,
    pub memory: String,
}
