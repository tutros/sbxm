use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{CreateSpec, SandboxBackend};

/// Shells out to the `sbx` CLI on PATH.
#[derive(Debug, Default)]
pub struct SbxBackend;

impl SandboxBackend for SbxBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        let status = Command::new("sbx")
            .args(create_args(spec))
            .status()
            .context("cannot run `sbx`; is Docker Sandboxes installed and on PATH?")?;
        if !status.success() {
            bail!("`sbx create` failed for sandbox {} ({status})", spec.name);
        }
        Ok(())
    }
}

fn create_args(spec: &CreateSpec) -> Vec<OsString> {
    vec![
        "create".into(),
        "--name".into(),
        spec.name.clone().into(),
        "--cpus".into(),
        spec.cpus.to_string().into(),
        "-m".into(),
        spec.memory.clone().into(),
        spec.agent.clone().into(),
        spec.workspace.clone().into(),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn create_args_match_sbx_cli() {
        let spec = CreateSpec {
            name: "sbxm-demo-claude".into(),
            agent: "claude".into(),
            workspace: PathBuf::from("/work/demo"),
            cpus: 4,
            memory: "8g".into(),
        };

        assert_eq!(
            create_args(&spec),
            [
                "create",
                "--name",
                "sbxm-demo-claude",
                "--cpus",
                "4",
                "-m",
                "8g",
                "claude",
                "/work/demo"
            ]
            .map(std::ffi::OsString::from)
        );
    }
}
