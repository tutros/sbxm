use std::ffi::OsString;
use std::process::Command;

use anyhow::{Context, Result, bail};

use serde::Deserialize;

use std::path::Path;

use super::{CreateSpec, KitValidation, SandboxBackend, SandboxInfo};

/// Shells out to the `sbx` CLI on PATH.
#[derive(Debug, Default)]
pub struct SbxBackend;

impl SandboxBackend for SbxBackend {
    fn create(&self, spec: &CreateSpec) -> Result<()> {
        run_sbx(create_args(spec), &spec.name)
    }

    fn list(&self) -> Result<Vec<SandboxInfo>> {
        let output = Command::new("sbx")
            .args(["ls", "--json"])
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx ls --json` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        parse_ls(&String::from_utf8_lossy(&output.stdout))
    }

    fn stop(&self, name: &str) -> Result<()> {
        run_sbx(stop_args(name), name)
    }

    fn remove(&self, name: &str) -> Result<()> {
        run_sbx(remove_args(name), name)
    }

    fn attach(&self, name: &str) -> Result<()> {
        run_sbx(attach_args(name), name)
    }

    fn validate_kit(&self, dir: &Path) -> Result<KitValidation> {
        // Exits non-zero for an invalid kit but still prints the JSON result.
        let output = Command::new("sbx")
            .args(validate_args(dir))
            .output()
            .context(SBX_MISSING)?;
        parse_validate(&String::from_utf8_lossy(&output.stdout)).with_context(|| {
            format!(
                "`sbx kit validate` failed for {} ({}): {}",
                dir.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
    }
}

fn validate_args(dir: &Path) -> Vec<OsString> {
    vec!["kit".into(), "validate".into(), "--json".into(), dir.into()]
}

fn parse_validate(json: &str) -> Result<KitValidation> {
    serde_json::from_str(json).context("unexpected `sbx kit validate --json` output")
}

const SBX_MISSING: &str = "cannot run `sbx`; is Docker Sandboxes installed and on PATH?";

/// Runs `sbx <args>` with the terminal attached, so `sbx`'s own progress
/// and errors reach the user.
fn run_sbx<I, S>(args: I, sandbox: &str) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let verb = args[0].to_string_lossy().into_owned();
    let status = Command::new("sbx")
        .args(&args)
        .status()
        .context(SBX_MISSING)?;
    if !status.success() {
        bail!("`sbx {verb}` failed for sandbox {sandbox} ({status})");
    }
    Ok(())
}

fn stop_args(name: &str) -> [&str; 2] {
    ["stop", name]
}

fn remove_args(name: &str) -> [&str; 3] {
    ["rm", "-f", name]
}

/// `--name` only: `sbx run` would otherwise create a sandbox without sbxm's config.
fn attach_args(name: &str) -> [&str; 3] {
    ["run", "--name", name]
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

#[cfg(test)]
mod stop_tests {
    use super::*;

    #[test]
    fn stop_args_match_sbx_cli() {
        assert_eq!(stop_args("sbxm-demo-claude"), ["stop", "sbxm-demo-claude"]);
    }

    #[test]
    fn attach_args_match_sbx_cli() {
        assert_eq!(
            attach_args("sbxm-demo-claude"),
            ["run", "--name", "sbxm-demo-claude"]
        );
    }

    #[test]
    fn remove_args_match_sbx_cli() {
        assert_eq!(
            remove_args("sbxm-demo-claude"),
            ["rm", "-f", "sbxm-demo-claude"]
        );
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    // Captured from `sbx kit validate --json` (v0.43.0).
    const VALID: &str = include_str!("fixtures/kit-validate-valid.json");
    const INVALID: &str = include_str!("fixtures/kit-validate-invalid.json");

    #[test]
    fn parses_valid_result() {
        let result = parse_validate(VALID).unwrap();
        assert!(result.valid);
        assert_eq!(result.error, None);
    }

    #[test]
    fn parses_invalid_result_with_error() {
        let result = parse_validate(INVALID).unwrap();
        assert!(!result.valid);
        assert!(result.error.unwrap().starts_with("manifest: invalid name"));
    }

    #[test]
    fn validate_args_match_sbx_cli() {
        assert_eq!(
            validate_args(Path::new("/k/common")),
            ["kit", "validate", "--json", "/k/common"].map(std::ffi::OsString::from)
        );
    }
}
