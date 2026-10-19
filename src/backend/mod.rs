//! All `sbx` interaction goes through [`SandboxBackend`] (decision 17).

mod fake;
mod sbx;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use fake::{ExecGate, FakeBackend};
pub use sbx::SbxBackend;

pub trait SandboxBackend: Send + Sync {
    fn create(&self, spec: &CreateSpec) -> Result<()>;
    fn list(&self) -> Result<Vec<SandboxInfo>>;
    fn stop(&self, name: &str) -> Result<()>;
    /// Removes the sandbox without prompting (`sbx rm -f`).
    fn remove(&self, name: &str) -> Result<()>;
    /// Attaches the terminal to the sandbox's agent, starting it if stopped.
    fn attach(&self, name: &str) -> Result<()>;
    /// Checks a kit directory (`sbx kit validate --json`).
    fn validate_kit(&self, dir: &Path) -> Result<KitValidation>;
    /// Names of the service secrets a new sandbox gets (`sbx secret ls
    /// --json`, global service entries only; decision 58).
    fn secret_services(&self) -> Result<Vec<String>>;
    /// The `sbx` client version without the leading `v`, e.g. `0.43.0`
    /// (`sbx version --json`).
    fn version(&self) -> Result<String>;
    /// Runs a command inside a running sandbox (`sbx exec`). A non-zero exit
    /// code is reported in [`ExecOutput`], not as an error; `Err` means `sbx`
    /// itself couldn't run.
    fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput>;
    /// The skills store listing (`sbx skills ls --json`), shape undocumented.
    fn skills(&self) -> Result<serde_json::Value>;
    /// Saves a stopped sandbox as a template under `tag` (`sbx template save`). Saving onto an
    /// existing tag replaces it silently (S11, question 8), so callers pass a fresh one.
    fn template_save(&self, sandbox: &str, tag: &str) -> Result<()>;
    /// Every local template, built-in agent images included (`sbx template ls --json`).
    fn template_ls(&self) -> Result<Vec<TemplateImage>>;
    /// Removes a template sbxm saved (`sbx template rm --force`), even while a sandbox created
    /// from it exists (S11, question 8): callers check that it is unused first (decision 205).
    fn template_rm(&self, tag: &str) -> Result<()>;
    /// The network rules that apply to a sandbox (`sbx policy ls <sandbox> --json`), for the check
    /// of a strict `codex` sandbox after its creation (decision 221).
    fn network_rules(&self, sandbox: &str) -> Result<Vec<NetworkRule>>;
}

/// One network rule of `sbx policy ls --json`: the `codex` kit's is named `kit:<sandbox>`, and
/// each `--deny-network` of `sbx create` is one deny rule (decision 221).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRule {
    pub name: String,
    /// `decision` is `allow`; `false` for `deny`.
    pub allow: bool,
    /// Hosts, the kit's with a port (`github.com:443`).
    pub resources: Vec<String>,
}

/// One entry of `sbx template ls --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateImage {
    pub id: String,
    /// `docker.io/library/<name>` for a saved template (S11).
    pub repository: String,
    pub tag: String,
    /// The agent image it came from, e.g. `claude-code-docker`.
    pub flavor: String,
    pub created_at: String,
    /// Bytes, as `sbx` reports them.
    pub size: u64,
}

/// `sbx template rm --force` deletes any image, built-in agent images included, so every backend
/// removes only a tag sbxm itself saves: `sbxm-<name>:<tag>`, in the characters sbxm's tags use
/// (decision 211).
pub fn check_template_tag(tag: &str) -> Result<()> {
    let allowed = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || extra.contains(&c))
    };
    let ours = tag.split_once(':').is_some_and(|(name, version)| {
        name.strip_prefix("sbxm-")
            .is_some_and(|rest| allowed(rest, &['-']))
            && allowed(version, &['-', '.', '_'])
    });
    if !ours {
        anyhow::bail!(
            "refusing to remove template '{tag}': sbxm only removes the templates it saves \
             (sbxm-<name>:<tag>); remove others with `sbx template rm` yourself"
        );
    }
    Ok(())
}

impl TemplateImage {
    /// The name the template was saved under, as `sbx create -t` takes it: `<name>:<tag>` for a
    /// saved template, the full repository otherwise.
    pub fn reference(&self) -> String {
        let repository = self
            .repository
            .strip_prefix("docker.io/library/")
            .unwrap_or(&self.repository);
        format!("{repository}:{}", self.tag)
    }
}

/// How stdin is wired for [`SandboxBackend::exec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    /// Closed (null device).
    Closed,
    /// A pipe that receives this text and is then closed; an empty string
    /// gives EOF straight away, which `codex exec` needs.
    Piped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    /// `sbx exec -w`; the sandbox's default when `None`.
    pub workdir: Option<PathBuf>,
    pub argv: Vec<String>,
    pub stdin: Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
}

/// Result of `sbx kit validate --json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KitValidation {
    pub valid: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Shown to the user as-is; the shape isn't documented.
    #[serde(default)]
    pub warnings: Vec<serde_json::Value>,
}

/// One entry of `sbx ls --json`; other fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxInfo {
    pub name: String,
    pub agent: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSpec {
    pub name: String,
    pub agent: String,
    pub workspace: PathBuf,
    pub cpus: u32,
    pub memory: String,
    /// Always passed explicitly: without `--skills`, `sbx` falls back to its
    /// own `skills.defaultMode` setting, which could be `readwrite`.
    pub skills: SkillsStore,
    /// Mixin kit directories, passed as `--kit` in this order.
    pub kits: Vec<PathBuf>,
    /// A saved template's tag, passed as `-t TAG --pull never` (S11); the agent's own image when
    /// `None`.
    pub template: Option<String>,
    /// Hosts passed as `--deny-network`, which win over the agent kit's own allow rule (decision
    /// 221).
    pub deny_network: Vec<String>,
}

/// How `sbx`'s shared skills store is mounted (decision 46). sbxm never uses
/// `readwrite`: an agent could plant skills that every other sandbox loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillsStore {
    #[default]
    ReadOnly,
    Off,
}

impl SkillsStore {
    pub fn as_arg(self) -> &'static str {
        match self {
            SkillsStore::ReadOnly => "readonly",
            SkillsStore::Off => "off",
        }
    }
}
