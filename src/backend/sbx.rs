use std::ffi::OsString;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use serde::Deserialize;

use std::path::Path;

use super::{
    CreateSpec, ExecOutput, ExecSpec, KitValidation, NetworkRule, SandboxBackend, SandboxInfo,
    Stdin, TemplateImage, check_template_tag,
};

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
        // sbx's own "sandbox not found" is held back: callers check `list` and treat a sandbox
        // that is already gone as removed, so printing it would be alarming noise.
        let output = Command::new("sbx")
            .args(remove_args(name))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx rm` failed for sandbox {name} ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    fn attach(&self, name: &str) -> Result<()> {
        run_sbx(attach_args(name), name)
    }

    fn secret_services(&self) -> Result<Vec<String>> {
        let output = Command::new("sbx")
            .args(secret_ls_args())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx secret ls --json` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        parse_secret_ls(&String::from_utf8_lossy(&output.stdout))
    }

    fn version(&self) -> Result<String> {
        let output = Command::new("sbx")
            .args(version_args())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx version --json` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        parse_version(&String::from_utf8_lossy(&output.stdout))
    }

    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput> {
        let mut child = Command::new("sbx")
            .args(exec_args(sandbox, spec))
            .stdin(match spec.stdin {
                Stdin::Closed => Stdio::null(),
                Stdin::Piped(_) => Stdio::piped(),
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(SBX_MISSING)?;
        // Written from a thread so a large input can't deadlock against the
        // output pipes; dropping the handle closes stdin.
        let writer = match (&spec.stdin, child.stdin.take()) {
            (Stdin::Piped(text), Some(mut pipe)) => {
                let text = text.clone();
                Some(std::thread::spawn(move || pipe.write_all(text.as_bytes())))
            }
            _ => None,
        };
        let output = child.wait_with_output()?;
        if let Some(writer) = writer {
            // A broken pipe only means the command didn't read its stdin.
            let _ = writer.join();
        }
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
        })
    }

    fn skills(&self) -> Result<serde_json::Value> {
        let output = Command::new("sbx")
            .args(skills_args())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx skills ls --json` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        serde_json::from_slice(&output.stdout).context("unexpected `sbx skills ls --json` output")
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

    fn template_save(&self, sandbox: &str, tag: &str) -> Result<()> {
        run_sbx_unprompted(&template_save_args(sandbox, tag))
    }

    fn template_ls(&self) -> Result<Vec<TemplateImage>> {
        let output = Command::new("sbx")
            .args(template_ls_args())
            .stdin(Stdio::null())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx template ls --json` failed ({}): {}",
                output.status,
                failure_reason(&output.stdout, &output.stderr)
            );
        }
        parse_template_ls(&String::from_utf8_lossy(&output.stdout))
    }

    fn template_rm(&self, tag: &str) -> Result<()> {
        run_sbx_unprompted(&template_rm_args(tag)?)
    }

    fn network_rules(&self, sandbox: &str) -> Result<Vec<NetworkRule>> {
        let output = Command::new("sbx")
            .args(policy_ls_args(sandbox))
            .stdin(Stdio::null())
            .output()
            .context(SBX_MISSING)?;
        if !output.status.success() {
            bail!(
                "`sbx policy ls {sandbox} --json` failed ({}): {}",
                output.status,
                failure_reason(&output.stdout, &output.stderr)
            );
        }
        parse_policy_ls(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Runs `sbx <args>` with stdin closed, so a question `sbx` asks (S11: `template save` on a
/// running sandbox, `template rm` without `--force`) fails instead of waiting.
fn run_sbx_unprompted(args: &[&str]) -> Result<()> {
    let output = Command::new("sbx")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(SBX_MISSING)?;
    if !output.status.success() {
        bail!(
            "`sbx {}` failed ({}): {}",
            args.join(" "),
            output.status,
            failure_reason(&output.stdout, &output.stderr)
        );
    }
    Ok(())
}

/// `sbx`'s stderr, or its stdout when stderr is empty: S11 didn't check which one carries a
/// template command's error.
fn failure_reason(stdout: &[u8], stderr: &[u8]) -> String {
    let text = |bytes| String::from_utf8_lossy(bytes).trim().to_owned();
    let (stdout, stderr) = (text(stdout), text(stderr));
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "no output from sbx".into()
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

fn exec_args(sandbox: &str, spec: &ExecSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["exec".into()];
    if let Some(dir) = &spec.workdir {
        args.extend(["-w".into(), dir.into()]);
    }
    args.push(sandbox.into());
    args.extend(spec.argv.iter().map(OsString::from));
    args
}

fn skills_args() -> [&'static str; 3] {
    ["skills", "ls", "--json"]
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

fn secret_ls_args() -> [&'static str; 3] {
    ["secret", "ls", "--json"]
}

/// Names of global service secrets. Sandbox-scoped secrets and host env vars
/// `sbx` hasn't imported don't reach a new sandbox, so they don't count
/// (decision 58). The masked values are never read.
fn parse_secret_ls(json: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct SecretLs {
        secrets: Vec<Secret>,
    }
    #[derive(Deserialize)]
    struct Secret {
        scope: String,
        #[serde(rename = "type")]
        kind: String,
        name: String,
    }
    let ls: SecretLs =
        serde_json::from_str(json).context("unexpected `sbx secret ls --json` output")?;
    Ok(ls
        .secrets
        .into_iter()
        .filter(|s| s.scope == "global" && s.kind == "service")
        .map(|s| s.name)
        .collect())
}

fn version_args() -> [&'static str; 2] {
    ["version", "--json"]
}

/// The client version; the server part is left to `sbx ls` (daemon check).
fn parse_version(json: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Version {
        client: Client,
    }
    #[derive(Deserialize)]
    struct Client {
        version: String,
    }
    let version: Version =
        serde_json::from_str(json).context("unexpected `sbx version --json` output")?;
    let client = version.client.version;
    Ok(client.strip_prefix('v').unwrap_or(&client).to_owned())
}

fn template_save_args<'a>(sandbox: &'a str, tag: &'a str) -> [&'a str; 4] {
    ["template", "save", sandbox, tag]
}

fn template_ls_args() -> [&'static str; 3] {
    ["template", "ls", "--json"]
}

fn template_rm_args(tag: &str) -> Result<[&str; 4]> {
    check_template_tag(tag)?;
    Ok(["template", "rm", "--force", tag])
}

fn policy_ls_args(sandbox: &str) -> [&str; 4] {
    ["policy", "ls", sandbox, "--json"]
}

/// The network rules of `sbx policy ls --json`; filesystem rules are left out.
fn parse_policy_ls(json: &str) -> Result<Vec<NetworkRule>> {
    #[derive(Deserialize)]
    struct Ls {
        rules: Vec<Rule>,
    }
    #[derive(Deserialize)]
    struct Rule {
        name: String,
        resource_type: String,
        decision: String,
        #[serde(default)]
        resources: Vec<String>,
    }
    let ls: Ls = serde_json::from_str(json).context("unexpected `sbx policy ls --json` output")?;
    ls.rules
        .into_iter()
        .filter(|rule| rule.resource_type == "network")
        .map(|rule| {
            let allow = match rule.decision.as_str() {
                "allow" => true,
                "deny" => false,
                other => bail!(
                    "unexpected decision '{other}' of network rule '{}' in `sbx policy ls --json`",
                    rule.name
                ),
            };
            Ok(NetworkRule {
                name: rule.name,
                allow,
                resources: rule.resources,
            })
        })
        .collect()
}

fn parse_template_ls(json: &str) -> Result<Vec<TemplateImage>> {
    #[derive(Deserialize)]
    struct Ls {
        images: Vec<TemplateImage>,
    }
    let ls: Ls =
        serde_json::from_str(json).context("unexpected `sbx template ls --json` output")?;
    Ok(ls.images)
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
    let mut args: Vec<OsString> = vec![
        "create".into(),
        "--name".into(),
        spec.name.clone().into(),
        "--cpus".into(),
        spec.cpus.to_string().into(),
        "-m".into(),
        spec.memory.clone().into(),
        "--skills".into(),
        spec.skills.as_arg().into(),
    ];
    if let Some(tag) = &spec.template {
        args.extend(["-t".into(), tag.into(), "--pull".into(), "never".into()]);
    }
    for kit in &spec.kits {
        args.push("--kit".into());
        args.push(kit.into());
    }
    for host in &spec.deny_network {
        args.extend(["--deny-network".into(), host.into()]);
    }
    args.extend([spec.agent.clone().into(), spec.workspace.clone().into()]);
    args
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn exec_spec(workdir: Option<&str>) -> ExecSpec {
        ExecSpec {
            workdir: workdir.map(PathBuf::from),
            argv: vec!["echo".into(), "hi".into()],
            stdin: Stdin::Closed,
        }
    }

    #[test]
    fn exec_args_match_sbx_cli() {
        assert_eq!(
            exec_args("sb", &exec_spec(None)),
            ["exec", "sb", "echo", "hi"].map(OsString::from)
        );
        assert_eq!(
            exec_args("sb", &exec_spec(Some("/work"))),
            ["exec", "-w", "/work", "sb", "echo", "hi"].map(OsString::from)
        );
    }

    #[test]
    fn skills_args_match_sbx_cli() {
        assert_eq!(skills_args(), ["skills", "ls", "--json"]);
    }

    #[test]
    fn create_args_match_sbx_cli() {
        let spec = CreateSpec {
            name: "sbxm-demo-claude".into(),
            agent: "claude".into(),
            workspace: PathBuf::from("/work/demo"),
            cpus: 4,
            memory: "8g".into(),
            skills: crate::backend::SkillsStore::Off,
            kits: vec![PathBuf::from("/k/common")],
            template: None,
            deny_network: Vec::new(),
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
                "--skills",
                "off",
                "--kit",
                "/k/common",
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

#[cfg(test)]
mod secret_tests {
    use super::*;

    /// Reconstructed (decision 58): field names and the global `anthropic`
    /// entry match `sbx secret ls --json` (v0.43.0) read with values redacted;
    /// the sandbox-scoped `github` entry is made up in the same shape.
    const SECRET_LS: &str = include_str!("fixtures/sbx-secret-ls.json");

    #[test]
    fn keeps_only_global_service_secrets() {
        assert_eq!(parse_secret_ls(SECRET_LS).unwrap(), ["anthropic"]);
    }

    #[test]
    fn secret_ls_args_match_sbx_cli() {
        assert_eq!(secret_ls_args(), ["secret", "ls", "--json"]);
    }

    /// Captured from `sbx version --json`, sbx v0.43.0.
    const VERSION: &str = include_str!("fixtures/sbx-version.json");

    #[test]
    fn parses_the_client_version() {
        assert_eq!(parse_version(VERSION).unwrap(), "0.43.0");
    }

    #[test]
    fn version_args_match_sbx_cli() {
        assert_eq!(version_args(), ["version", "--json"]);
    }
}

#[cfg(test)]
mod template_tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backend::TemplateImage;

    /// The first four entries captured from `sbx template ls --json` (v0.46.0, 2026-10-16). The
    /// fifth is a saved template in the shape spike S15 recorded (repository, tag, flavor and size
    /// as reported; id and created_at made up).
    const TEMPLATE_LS: &str = include_str!("fixtures/sbx-template-ls.json");

    #[test]
    fn create_args_from_a_template_pin_it_and_never_pull() {
        let spec = CreateSpec {
            name: "sbxm-demo-claude".into(),
            agent: "claude".into(),
            workspace: PathBuf::from("/work/demo"),
            cpus: 4,
            memory: "8g".into(),
            skills: crate::backend::SkillsStore::ReadOnly,
            kits: vec![PathBuf::from("/k/common")],
            template: Some("sbxm-dev-claude:2a9974ee6210-20261015t2210".into()),
            deny_network: Vec::new(),
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
                "--skills",
                "readonly",
                "-t",
                "sbxm-dev-claude:2a9974ee6210-20261015t2210",
                "--pull",
                "never",
                "--kit",
                "/k/common",
                "claude",
                "/work/demo"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn template_save_args_match_sbx_cli() {
        assert_eq!(
            template_save_args(
                "sbxm-template-dev-claude",
                "sbxm-dev-claude:abc-20261016t1200"
            ),
            [
                "template",
                "save",
                "sbxm-template-dev-claude",
                "sbxm-dev-claude:abc-20261016t1200"
            ]
        );
    }

    #[test]
    fn template_ls_args_match_sbx_cli() {
        assert_eq!(template_ls_args(), ["template", "ls", "--json"]);
    }

    #[test]
    fn template_rm_args_force_the_removal_of_an_sbxm_tag() {
        assert_eq!(
            template_rm_args("sbxm-dev-claude:abc-20261016t1200").unwrap(),
            [
                "template",
                "rm",
                "--force",
                "sbxm-dev-claude:abc-20261016t1200"
            ]
        );
    }

    #[test]
    fn template_rm_refuses_anything_but_an_sbxm_tag() {
        for tag in [
            "docker.io/docker/sandbox-templates:claude-code-docker",
            "claude-code-docker",
            "docker.io/library/sbxm-dev-claude:abc",
            "sbxm-dev-claude",
            "sbxm-dev-claude:",
            "other:abc",
            "sbxm-:abc",
            "sbxm-dev-claude:abc --all",
        ] {
            let err = template_rm_args(tag).unwrap_err();
            let message = format!("{err:#}");
            assert!(
                message.contains(&format!("refusing to remove template '{tag}'")),
                "{message}"
            );
        }
    }

    #[test]
    fn parses_sbx_template_ls_json() {
        let images = parse_template_ls(TEMPLATE_LS).unwrap();

        assert_eq!(images.len(), 5);
        assert_eq!(
            images[4],
            TemplateImage {
                id: "3f1b2c4d5e6a".into(),
                repository: "docker.io/library/sbxm-spike-s15".into(),
                tag: "pi-1".into(),
                flavor: "pi".into(),
                created_at: "2026-10-16T21:04:12Z".into(),
                size: 734_437_048,
            }
        );
    }

    #[test]
    fn a_failure_reason_falls_back_to_stdout_when_stderr_is_empty() {
        assert_eq!(failure_reason(b"on stdout\n", b" \n"), "on stdout");
        assert_eq!(failure_reason(b"on stdout", b"on stderr\n"), "on stderr");
        assert_eq!(failure_reason(b"", b""), "no output from sbx");
    }

    #[test]
    fn a_saved_template_is_named_as_it_was_saved() {
        let images = parse_template_ls(TEMPLATE_LS).unwrap();

        assert_eq!(images[4].reference(), "sbxm-spike-s15:pi-1");
        assert_eq!(
            images[0].reference(),
            "docker.io/docker/sandbox-templates:claude-code-docker"
        );
    }
}

// Milestone 3 slice 6: a strict `codex` sandbox's deny rules and the check of its kit rule
// (decision 221).
#[cfg(test)]
mod policy_tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backend::NetworkRule;

    /// Captured from `sbx policy ls <sandbox> --json` (v0.46.0, 2026-10-19) for a `codex` sandbox
    /// created with `--deny-network github.com --deny-network registry.npmjs.org`.
    const POLICY_LS: &str = include_str!("fixtures/sbx-policy-ls.json");

    #[test]
    fn create_args_deny_each_host_before_the_agent() {
        let spec = CreateSpec {
            name: "sbxm-demo-codex".into(),
            agent: "codex".into(),
            workspace: PathBuf::from("/work/demo"),
            cpus: 4,
            memory: "8g".into(),
            skills: crate::backend::SkillsStore::ReadOnly,
            kits: vec![PathBuf::from("/k/common")],
            template: None,
            deny_network: vec!["github.com".into(), "registry.npmjs.org".into()],
        };

        let args = create_args(&spec);

        assert_eq!(
            args[args.len() - 8..],
            [
                "--kit",
                "/k/common",
                "--deny-network",
                "github.com",
                "--deny-network",
                "registry.npmjs.org",
                "codex",
                "/work/demo"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn policy_ls_args_match_sbx_cli() {
        assert_eq!(
            policy_ls_args("sbxm-demo-codex"),
            ["policy", "ls", "sbxm-demo-codex", "--json"]
        );
    }

    #[test]
    fn policy_ls_keeps_the_network_rules_only() {
        let rules = parse_policy_ls(POLICY_LS).unwrap();

        assert_eq!(rules.len(), 3, "{rules:?}");
        assert_eq!(
            rules[0],
            NetworkRule {
                name: "kit:sbxm-fixture-policy-codex".into(),
                allow: true,
                resources: [
                    "api.openai.com:443",
                    "openai.com:443",
                    "auth.openai.com:443",
                    "chatgpt.com:443",
                    "files.openai.com:443",
                    "registry.npmjs.org:443",
                    "releases.openai.com:443",
                    "api.github.com:443",
                    "github.com:443",
                    "release-assets.githubusercontent.com:443",
                    "codeload.github.com:443",
                    "archive.ubuntu.com:80",
                    "security.ubuntu.com:80",
                    "ports.ubuntu.com:80",
                    "download.docker.com:443",
                ]
                .map(String::from)
                .to_vec(),
            }
        );
        assert!(!rules[1].allow);
        assert_eq!(rules[1].resources, ["github.com"]);
        assert_eq!(rules[2].resources, ["registry.npmjs.org"]);
    }

    #[test]
    fn a_rule_with_an_unknown_decision_is_an_error() {
        let json = POLICY_LS.replacen("\"decision\": \"deny\"", "\"decision\": \"ask\"", 1);

        let err = parse_policy_ls(&json).unwrap_err();

        assert!(format!("{err:#}").contains("ask"), "{err:#}");
    }
}
