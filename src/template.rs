//! Prebuilt `sbx` templates (milestone 3): which harnesses support them and the `sbx` version
//! they need.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::backend::SandboxBackend;
use crate::config::Profile;
use crate::harness::Harness;
use crate::kit;
use crate::run::results::format_timestamp;

/// The `sbx` version templates were tested with (spike S11); older is unsupported (decision 213).
/// The global `min_sbx_version` stays for everything else.
pub const MIN_SBX_VERSION: &str = "0.46.0";

/// `claude` and `codex` (decisions 204, 217) and `pi` (spike S15); the rest are refused.
pub fn check_supported(harness: Harness) -> Result<()> {
    if !matches!(harness, Harness::Claude | Harness::Codex | Harness::Pi) {
        bail!(
            "templates are not supported for the {} harness yet; build one for claude, codex or pi",
            harness.as_str()
        );
    }
    Ok(())
}

/// Refuses an `sbx` older than [`MIN_SBX_VERSION`].
pub fn check_sbx_version(version: &str) -> Result<()> {
    if crate::commands::doctor::older(version, MIN_SBX_VERSION) {
        bail!(
            "sbx {version} is older than {MIN_SBX_VERSION}, the version templates need; upgrade sbx"
        );
    }
    Ok(())
}

/// `<base>/.sbxm/templates`, where records, build locks and build leftovers live.
pub fn dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("templates")
}

/// `<profile>-<harness>`, the stem of a template's record, build sandbox and build folders.
pub fn key(profile: &str, harness: Harness) -> String {
    format!("{profile}-{}", harness.as_str())
}

/// `sbxm-<profile>-<harness>:<hash12>-<yyyymmdd>t<hhmm>` in UTC (decision 211).
pub fn tag(profile: &str, harness: Harness, template_hash: &str, now: u64) -> String {
    let stamp: String = format_timestamp(now)
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'T')
        .take(13)
        .collect();
    format!(
        "sbxm-{}:{}-{}",
        key(profile, harness),
        &template_hash[..12],
        stamp.to_ascii_lowercase()
    )
}

/// `<base>/.sbxm/templates/<profile>-<harness>.json` (decision 206).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub tag: String,
    pub template_hash: String,
    pub warm: bool,
    pub lock_files: Vec<String>,
    pub lock_hash: Option<String>,
    pub sbx_version: String,
    pub size_bytes: u64,
    pub created_at: String,
    /// Earlier tags kept because a sandbox still used them (decision 205a).
    pub previous: Vec<String>,
}

impl Record {
    /// Written to a temporary file and renamed over the old record, so a failed write never
    /// loses the tags the old one names (slice 4 review S-2).
    pub fn save(&self, base_dir: &Path, key: &str) -> Result<()> {
        let dir = dir(base_dir);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("{key}.json"));
        let tmp = dir.join(format!("{key}.json.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }
}

/// Who still records `tag` as their template (decision 205): `sandbox <name>` for each
/// `state.json` entry and `task <id>` for each task whose worker or reviewer has it.
pub fn users(base_dir: &Path, tag: &str) -> Result<Vec<String>> {
    let mut users = Vec::new();
    for (_, state) in crate::state::load_all(base_dir)? {
        for sandbox in state.sandboxes.values() {
            if sandbox.template.as_deref() == Some(tag) {
                users.push(format!("sandbox {}", sandbox.sandbox));
            }
        }
    }
    for task in crate::task::record::load_all(base_dir)? {
        let agents = [&task.worker, &task.reviewer];
        if agents
            .iter()
            .any(|a| a.as_ref().and_then(|a| a.template.as_deref()) == Some(tag))
        {
            users.push(format!("task {}", task.id));
        }
    }
    Ok(users)
}

impl Record {
    /// The record of `key`, or `None` if there is none.
    pub fn load(base_dir: &Path, key: &str) -> Result<Option<Self>> {
        let path = dir(base_dir).join(format!("{key}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).map(Some).with_context(|| {
            format!(
                "template record {} is not valid; delete it, then remove the sbxm- templates it \
                 named with `sbx template ls` and `sbx template rm`",
                path.display()
            )
        })
    }
}

/// The combined SHA-256 of `files` (paths relative to `repo`), in order (decision 207).
/// Each file's path and length go in before its contents, so moving bytes between files changes
/// the hash.
pub fn lock_hash(repo: &Path, files: &[String]) -> Result<String> {
    let mut bytes = Vec::new();
    for file in files {
        let path = repo.join(file);
        let contents =
            fs::read(&path).with_context(|| format!("cannot read lock file {}", path.display()))?;
        bytes.extend_from_slice(file.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&contents);
    }
    Ok(crate::config::sha256_hex(&bytes))
}

/// What a sandbox of a profile is created from: its template's tag, or `None` for the profile's
/// install steps.
pub struct Request<'a> {
    pub base_dir: &'a Path,
    pub profile_name: &'a str,
    /// The profile without the project's `sandbox.toml`.
    pub profile: &'a Profile,
    /// The profile with it.
    pub merged: &'a Profile,
    /// The project's `sandbox.toml`, named when it adds a build input (decision 214).
    pub project_config: &'a Path,
    pub harness: Harness,
    pub sandbox: &'a str,
    /// Where the lock files of a warm template are read for the workspace (decision 203(b)).
    pub lock_dir: &'a Path,
}

/// The tag to create `request.sandbox` from, `None` for the install steps: a profile without
/// `[template]` never asks `sbx`; an unusable template refuses a strict profile (decision 200)
/// and warns once otherwise (decision 203).
pub fn choose(
    request: &Request,
    backend: &dyn SandboxBackend,
    warn: &mut dyn Write,
) -> Result<Option<String>> {
    let Some(table) = &request.merged.template else {
        return Ok(None);
    };
    match usable(request, backend)? {
        Ok(tag) => Ok(Some(tag)),
        Err((reason, fix)) if table.strict => bail!("{reason}; {fix}"),
        Err((reason, fix)) => {
            writeln!(
                warn,
                "warning: {reason}; creating {} with the profile's install steps instead ({fix})",
                request.sandbox
            )?;
            Ok(None)
        }
    }
}

/// Whether `request.sandbox` could be created from the template now, for `open`'s hint on an
/// install sandbox (decision 208).
pub fn ready(request: &Request, backend: &dyn SandboxBackend) -> Result<bool> {
    Ok(request.merged.template.is_some() && usable(request, backend)?.is_ok())
}

/// The template's tag, or why it can't be used and how to fix that.
fn usable(
    request: &Request,
    backend: &dyn SandboxBackend,
) -> Result<std::result::Result<String, (String, String)>> {
    let (profile_name, harness) = (request.profile_name, request.harness);
    let flag = match harness {
        Harness::Claude => String::new(),
        other => format!(" --harness {}", other.as_str()),
    };
    if let Some(key) = added_build_input(request.profile, request.merged) {
        return Ok(Err((
            format!(
                "project config {} adds {key}, which the template of profile '{profile_name}' \
                 can't hold",
                request.project_config.display()
            ),
            "move it to the profile and rebuild the template, or remove it".to_owned(),
        )));
    }
    let version = backend.version()?;
    if check_sbx_version(&version).is_err() {
        return Ok(Err((
            format!("sbx {version} is older than {MIN_SBX_VERSION}, the version templates need"),
            "upgrade sbx".to_owned(),
        )));
    }
    let missing = || {
        Err((
            format!(
                "profile '{profile_name}' has no template for {}",
                harness.as_str()
            ),
            format!("build one with `sbxm template build {profile_name}{flag}`"),
        ))
    };
    let Some(record) = Record::load(request.base_dir, &key(profile_name, harness))? else {
        return Ok(missing());
    };
    if !backend
        .template_ls()?
        .iter()
        .any(|t| t.reference() == record.tag)
    {
        return Ok(missing());
    }
    let stale = format!(
        "the template of profile '{profile_name}' for {} is stale",
        harness.as_str()
    );
    if record.template_hash != kit::template_hash(request.merged, harness, record.warm) {
        return Ok(Err((
            format!(
                "{stale}: the profile's install steps, build allow list or harness kit changed \
                 since it was built"
            ),
            format!("rebuild it with `sbxm template build {profile_name}{flag}`"),
        )));
    }
    if record.warm && lock_hash(request.lock_dir, &record.lock_files).ok() != record.lock_hash {
        return Ok(Err((
            format!(
                "{stale}: the lock files of {} ({}) differ from the ones it was warmed with",
                request.lock_dir.display(),
                record.lock_files.join(", ")
            ),
            format!("rebuild it with `sbxm template build {profile_name}{flag} --warm <repo>`"),
        )));
    }
    Ok(Ok(record.tag))
}

/// The first build input the project's `sandbox.toml` adds to the profile (decision 214): the
/// project config only ever extends these lists.
fn added_build_input(profile: &Profile, merged: &Profile) -> Option<&'static str> {
    if merged.setup.install.len() != profile.setup.install.len() {
        Some("setup.install")
    } else if merged.network.allow.len() != profile.network.allow.len() {
        Some("network.allow")
    } else if merged.network.deny.len() != profile.network.deny.len() {
        Some("network.deny")
    } else {
        None
    }
}

/// The OpenAI hosts the built-in `codex` kit allows, which a strict `codex` sandbox keeps (decision
/// 221).
const CODEX_PROVIDER_HOSTS: [&str; 6] = [
    "api.openai.com",
    "openai.com",
    "auth.openai.com",
    "chatgpt.com",
    "files.openai.com",
    "releases.openai.com",
];

/// The other hosts the built-in `codex` kit allows (`sbx` 0.46.0, decision 221(a)). They can't be
/// read before the sandbox exists, so sbxm keeps the list; [`check_codex_rules`] catches a newer
/// kit that adds one.
const CODEX_EXTRA_HOSTS: [&str; 9] = [
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "release-assets.githubusercontent.com",
    "registry.npmjs.org",
    "download.docker.com",
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "ports.ubuntu.com",
];

/// For a strict `codex` sandbox from `template`, the hosts it is created with `--deny-network`
/// for: every extra host of the `codex` kit not in `[template].allow`, possibly none (decision
/// 221). `None` for any other sandbox, which [`check_codex_rules`] then leaves unchecked.
pub fn denied_hosts(
    merged: &Profile,
    harness: Harness,
    template: Option<&str>,
) -> Option<Vec<String>> {
    match (&merged.template, harness, template) {
        (Some(table), Harness::Codex, Some(_)) if table.strict => Some(
            CODEX_EXTRA_HOSTS
                .iter()
                .filter(|host| !table.allow.iter().any(|a| a == *host))
                .map(|host| (*host).to_owned())
                .collect(),
        ),
        _ => None,
    }
}

/// After a strict `codex` sandbox is created and before any agent command: every host its kit
/// rule allows must be an OpenAI host, in `[template].allow` or denied by a rule, or the sandbox
/// is removed and refused, so a newer `codex` kit can't widen it silently (decision 221).
pub fn check_codex_rules(
    request: &Request,
    backend: &dyn SandboxBackend,
    denied: Option<&[String]>,
) -> Result<()> {
    if denied.is_none() {
        return Ok(());
    }
    let allow = request
        .merged
        .template
        .as_ref()
        .map(|t| t.allow.as_slice())
        .unwrap_or_default();
    let rules = backend.network_rules(request.sandbox)?;
    let deny_rules: Vec<&str> = rules
        .iter()
        .filter(|rule| !rule.allow)
        .flat_map(|rule| rule.resources.iter().map(|r| host_of(r)))
        .collect();
    let kit = format!("kit:{}", request.sandbox);
    let unexpected = rules
        .iter()
        .filter(|rule| rule.allow && rule.name == kit)
        .flat_map(|rule| rule.resources.iter().map(|r| host_of(r)))
        .find(|host| {
            !CODEX_PROVIDER_HOSTS.contains(host)
                && !allow.iter().any(|a| a == host)
                && !deny_rules.contains(host)
        });
    if let Some(host) = unexpected {
        backend.remove(request.sandbox)?;
        bail!(
            "the codex kit of {} allows {host}, which strict profile '{}' doesn't list in \
             [template].allow and sbxm doesn't deny; sbxm removed the sandbox. Add the host to \
             [template].allow if codex may reach it, or set strict = false",
            request.sandbox,
            request.profile_name
        );
    }
    Ok(())
}

/// `github.com` from `github.com:443`.
fn host_of(resource: &str) -> &str {
    resource.rsplit_once(':').map_or(resource, |(host, _)| host)
}
