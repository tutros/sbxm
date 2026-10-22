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

/// The largest lock file read from a commit (decision 236): a PR's head is written by someone
/// else, so it is never read into memory whole without a bound.
pub const MAX_LOCK_FILE_BYTES: u64 = 16 * 1024 * 1024;

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

/// Every `<profile>-<harness>.json` under `<base>/.sbxm/templates`, sorted by file name; build
/// locks, temporary files and folders are skipped.
pub fn records(base_dir: &Path) -> Result<Vec<(String, Harness, Record)>> {
    let dir = dir(base_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = name.strip_suffix(".json") else {
            continue;
        };
        if key.ends_with(".build") || !entry.file_type()?.is_file() {
            continue;
        }
        let Some((profile, harness)) = key.rsplit_once('-') else {
            continue;
        };
        let Ok(harness) = <Harness as clap::ValueEnum>::from_str(harness, false) else {
            continue;
        };
        keys.push((key.to_owned(), profile.to_owned(), harness));
    }
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    let mut records = Vec::new();
    for (key, profile, harness) in keys {
        if let Some(record) = Record::load(base_dir, &key)? {
            records.push((profile, harness, record));
        }
    }
    Ok(records)
}

/// The combined SHA-256 of `files` (paths relative to `repo`), in order (decision 207).
/// Each file's path and length go in before its contents, so moving bytes between files changes
/// the hash.
pub fn lock_hash(repo: &Path, files: &[String]) -> Result<String> {
    Locks::Dir(repo).hash(files)
}

/// Where a sandbox's lock files are read for a warm template's check (decision 203(b)): a host
/// folder, or a commit in a task's host-owned `repo.git`, so no host git runs in an agent's clone
/// (decisions 159, 216). A run without a seed has none, so a warm template never matches it, and
/// a pair only `sbxm run`'s judge uses builds nothing, so it is never checked (decision 237).
#[derive(Debug, Clone, Copy)]
pub enum Locks<'a> {
    Dir(&'a Path),
    Commit { repo_git: &'a Path, commit: &'a str },
    Unseeded,
    Unchecked,
}

impl Locks<'_> {
    /// [`lock_hash`] of `files` read from here. CRLF counts as LF, so a Windows checkout with
    /// `core.autocrlf` hashes like its commit (decision 238).
    pub fn hash(&self, files: &[String]) -> Result<String> {
        let mut bytes = Vec::new();
        for file in files {
            let contents = crlf_to_lf(self.read(file)?);
            bytes.extend_from_slice(file.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&contents);
        }
        Ok(crate::config::sha256_hex(&bytes))
    }

    fn read(&self, file: &str) -> Result<Vec<u8>> {
        match self {
            Locks::Dir(dir) => {
                let path = dir.join(file);
                fs::read(&path).with_context(|| format!("cannot read lock file {}", path.display()))
            }
            Locks::Commit { repo_git, commit } => {
                let object = format!("{commit}:{file}");
                let cannot = || format!("cannot read lock file {file} at commit {commit}");
                // Someone else may have written the commit (a PR's head): the size first.
                let size: u64 =
                    crate::git::run(repo_git, Some(repo_git), None, &["cat-file", "-s", &object])
                        .with_context(cannot)?
                        .trim()
                        .parse()
                        .with_context(cannot)?;
                if size > MAX_LOCK_FILE_BYTES {
                    bail!(
                        "lock file {file} at commit {commit} is {size} bytes, over the \
                         {MAX_LOCK_FILE_BYTES}-byte limit sbxm reads (decision 236)"
                    );
                }
                crate::git::run_bytes(
                    repo_git,
                    Some(repo_git),
                    None,
                    &["cat-file", "blob", &object],
                )
                .with_context(cannot)
            }
            Locks::Unseeded | Locks::Unchecked => bail!("{self} has no lock files to read"),
        }
    }
}

fn crlf_to_lf(bytes: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'\r' || bytes.get(i + 1) != Some(&b'\n') {
            out.push(b);
        }
    }
    out
}

impl std::fmt::Display for Locks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Locks::Dir(dir) => write!(f, "{}", dir.display()),
            Locks::Commit { commit, .. } => write!(f, "commit {commit}"),
            Locks::Unseeded => write!(f, "a run without a seed"),
            Locks::Unchecked => write!(f, "the judge's workspace"),
        }
    }
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
    /// Where the lock files of a warm template are read for the workspace (decisions 203(b), 216).
    pub locks: Locks<'a>,
}

/// The tag to create `request.sandbox` from, `None` for the install steps: a profile without
/// `[template]` never asks `sbx`; an unusable template refuses a strict profile (decision 200)
/// and warns once otherwise (decision 203). A strict profile is refused with `pi`, whose kit
/// allows a host strict mode can't close yet (decision 231).
pub fn choose(
    request: &Request,
    backend: &dyn SandboxBackend,
    warn: &mut dyn Write,
) -> Result<Option<String>> {
    let Some(table) = &request.merged.template else {
        return Ok(None);
    };
    if table.strict && request.harness == Harness::Pi {
        bail!(
            "strict profile '{}' can't be used with the pi harness yet: pi's own kit allows \
             registry.npmjs.org, which strict mode can't close (decision 231); set strict = false \
             in the profile, or use --harness claude or codex",
            request.profile_name
        );
    }
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
    if record.warm && matches!(request.locks, Locks::Unseeded) {
        // No rebuild can match a run with no lock files (review M3S9-2).
        return Ok(Err((
            format!(
                "the template of profile '{profile_name}' for {} is warm, and a run without a \
                 seed has no lock files to check it against",
                harness.as_str()
            ),
            format!(
                "set task.seed in the run-config, or build the template without --warm \
                 (`sbxm template build {profile_name}{flag}`)"
            ),
        )));
    }
    let locks = (record.warm && !matches!(request.locks, Locks::Unchecked))
        .then(|| request.locks.hash(&record.lock_files))
        .filter(|locks| locks.as_ref().ok() != record.lock_hash.as_ref());
    if let Some(locks) = locks {
        // An unreadable or too big lock file counts as differing; its reason goes along.
        let why = locks.err().map(|e| format!(" ({e:#})")).unwrap_or_default();
        return Ok(Err((
            format!(
                "{stale}: the lock files of {} ({}) differ from the ones it was warmed with{why}",
                request.locks,
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
                .filter(|host| !allows(&table.allow, host))
                .map(|host| (*host).to_owned())
                .collect(),
        ),
        _ => None,
    }
}

/// After a strict `codex` sandbox is created and before any agent command: every host an allow
/// rule scoped to the sandbox allows must be an OpenAI host, in `[template].allow` or denied by a
/// rule, or the sandbox is removed and refused, so a newer `codex` kit can't widen it silently
/// (decision 221). It fails closed: no kit rule at all is refused too, and global rules are the
/// user's own (decision 232).
pub fn check_codex_rules(
    profile_name: &str,
    merged: &Profile,
    sandbox: &str,
    backend: &dyn SandboxBackend,
    denied: Option<&[String]>,
) -> Result<()> {
    if denied.is_none() {
        return Ok(());
    }
    let allow = merged
        .template
        .as_ref()
        .map(|t| t.allow.as_slice())
        .unwrap_or_default();
    // Every refusal removes the sandbox first and keeps its own reason even when that fails
    // (review PR 269 M-2, decision 232).
    let refuse = |reason: String, fix: &str| -> anyhow::Error {
        match backend.remove(sandbox) {
            Ok(()) => anyhow::anyhow!("{reason}; sbxm removed the sandbox. {fix}"),
            Err(err) => anyhow::anyhow!(
                "{reason}; removing it failed; remove it with `sbx rm {sandbox}` ({err:#}). {fix}"
            ),
        }
    };
    let rules = match backend.network_rules(sandbox) {
        Ok(rules) => rules,
        Err(err) => {
            return Err(refuse(
                format!("sbxm can't read the network rules of {sandbox} ({err:#})"),
                "Check that `sbx policy ls` works, or set strict = false",
            ));
        }
    };
    let deny_rules: Vec<&str> = rules
        .iter()
        .filter(|rule| !rule.allow)
        .flat_map(|rule| rule.resources.iter().map(|r| host_of(r)))
        .collect();
    let (scope, kit) = (format!("sandbox:{sandbox}"), format!("kit:{sandbox}"));
    let scoped: Vec<_> = rules
        .iter()
        .filter(|rule| rule.allow && rule.scope == scope)
        .collect();
    if !scoped.iter().any(|rule| rule.name == kit) {
        return Err(refuse(
            format!(
                "sbx reported no network rule '{kit}' for {sandbox}, so sbxm can't see what the \
                 codex kit allows"
            ),
            "Set strict = false, or update sbxm if a newer sbx renamed the rule",
        ));
    }
    let unexpected = scoped
        .iter()
        .flat_map(|rule| rule.resources.iter().map(move |r| (rule, host_of(r))))
        .find(|(_, host)| {
            !CODEX_PROVIDER_HOSTS.contains(host)
                && !allows(allow, host)
                && !deny_rules.contains(host)
        });
    if let Some((rule, host)) = unexpected {
        return Err(refuse(
            format!(
                "the network rule '{}' of {sandbox} allows {host}, which strict profile '{}' \
                 doesn't list in [template].allow and sbxm doesn't deny",
                rule.name, profile_name
            ),
            "Add the host to [template].allow if codex may reach it, or set strict = false",
        ));
    }
    Ok(())
}

/// `github.com` from `github.com:443`.
/// Whether an allow list names `host`, exactly or through a `*.<domain>` entry, which covers
/// every host under the domain but not the domain itself (review PR 269 M-1).
fn allows(allow: &[String], host: &str) -> bool {
    allow.iter().any(|entry| match entry.strip_prefix("*.") {
        Some(domain) => host
            .strip_suffix(domain)
            .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
        None => entry == host,
    })
}

fn host_of(resource: &str) -> &str {
    resource.rsplit_once(':').map_or(resource, |(host, _)| host)
}
