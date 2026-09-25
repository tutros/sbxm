use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context, Result, bail};

use serde::Deserialize;

use super::{CreateSpec, SandboxBackend, SandboxInfo};

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

    fn list(&self) -> Result<Vec<SandboxInfo>> {
        let output = Command::new("sbx")
            .args(["ls", "--json"])
            .output()
            .context("cannot run `sbx`; is Docker Sandboxes installed and on PATH?")?;
        if !output.status.success() {
            bail!(
                "`sbx ls --json` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        parse_ls(&String::from_utf8_lossy(&output.stdout))
    }
}

fn parse_ls(json: &str) -> Result<Vec<SandboxInfo>> {
    #[derive(Deserialize)]
    struct Ls {
        sandboxes: Vec<SandboxInfo>,
    }
    let ls: Ls = serde_json::from_str(json).context("unexpected `sbx ls --json` output")?;
    Ok(ls.sandboxes)
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

#[cfg(test)]
mod ls_tests {
    use super::*;

    /// First entry captured from `sbx ls --json` (v0.43.0); second added in the same shape.
    const SBX_LS: &str = include_str!("fixtures/sbx-ls.json");

    #[test]
    fn parses_sbx_ls_json() {
        assert_eq!(
            parse_ls(SBX_LS).unwrap(),
            vec![
                SandboxInfo {
                    name: "spike-s6-codex".into(),
                    agent: "codex".into(),
                    status: "stopped".into(),
                },
                SandboxInfo {
                    name: "sbxm-demo-claude".into(),
                    agent: "claude".into(),
                    status: "running".into(),
                },
            ]
        );
    }
}
