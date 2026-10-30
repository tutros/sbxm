//! `sbxm auth antigravity` (decision 270, point 1): the user signs in inside a throwaway
//! Antigravity sandbox made from the pinned kit; sbxm copies `agy`'s token file and its client
//! pairs out with `sbx exec`, keeps the pair that refreshes the login, saves both to
//! `<config dir>/auth/antigravity.json` and removes the sandbox on every path. No token, secret
//! or token file ever reaches the screen or a log.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use crate::auth::{self, Host};
use crate::backend::{CreateSpec, ExecSpec, SandboxBackend, SkillsStore, Stdin};
use crate::config::GlobalConfig;
use crate::disk;
use crate::harness::Harness;

/// The sign-in sandbox. A project sandbox is `sbxm-<project>-<harness>` and a project name has no
/// dot, so this name is sbxm's own and a leftover one can be removed.
pub const SANDBOX: &str = "sbxm-auth.antigravity";

/// `agy`'s OAuth token file, read by a shell in the sandbox.
const READ_TOKEN: &str = r#"cat "$HOME/.gemini/antigravity-cli/antigravity-oauth-token""#;

/// `agy`'s installed-app client ids and secrets, as they appear in its binary (spike S16, Q3).
const READ_CLIENTS: &str = r#"grep -aoE '[0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com|GOCSPX-[A-Za-z0-9_-]+' "$(readlink -f "$(command -v agy)")" | sort -u"#;

pub fn run(
    config_dir: &Path,
    backend: &dyn SandboxBackend,
    host: &Host,
    guard: &disk::Guard,
    out: &mut dyn Write,
) -> Result<()> {
    let config = GlobalConfig::load(config_dir)?;
    auth::check_config_dir(config_dir, &config.base_dir)?;
    host.endpoint.check()?;
    disk::require(&config, config_dir, guard)?;
    let workspace = config.base_dir.join(".sbxm").join(".auth-antigravity");
    // One left by a killed sign-in goes first.
    if backend.list()?.iter().any(|s| s.name == SANDBOX) {
        backend
            .remove(SANDBOX)
            .with_context(|| format!("cannot remove {SANDBOX}, left by an earlier sign-in"))?;
    }
    if workspace.exists() {
        fs::remove_dir_all(&workspace)
            .with_context(|| format!("cannot clear {}", workspace.display()))?;
    }
    fs::create_dir_all(&workspace)
        .with_context(|| format!("cannot create {}", workspace.display()))?;

    writeln!(
        out,
        "Sign in to Antigravity in the sandbox that opens (Google, then paste the code from \
         antigravity.google), then quit agy."
    )?;
    let signed_in = sign_in(&config, &workspace, backend);
    let removed = backend.remove(SANDBOX);
    let cleared = fs::remove_dir_all(&workspace);
    let (refresh_token, pairs) = signed_in?;
    removed.with_context(|| format!("cannot remove {SANDBOX}; run `sbx rm -f {SANDBOX}`"))?;
    cleared.with_context(|| format!("cannot remove {}", workspace.display()))?;

    auth::save_login(config_dir, &refresh_token, &pairs, host.now, host.endpoint)?;
    writeln!(
        out,
        "Saved the Antigravity login to {}; set antigravity_auth = \"login\" in config.toml to \
         use it.",
        auth::login_path(config_dir).display()
    )?;
    Ok(())
}

/// Creates the sandbox, attaches the user to it and reads back the refresh token and every
/// client pair `agy` holds.
fn sign_in(
    config: &GlobalConfig,
    workspace: &Path,
    backend: &dyn SandboxBackend,
) -> Result<(String, Vec<(String, String)>)> {
    backend.create(&CreateSpec {
        name: SANDBOX.into(),
        agent: Harness::Antigravity.agent_arg().into(),
        workspace: Some(workspace.to_path_buf()),
        cpus: config.resources.cpus,
        memory: config.resources.memory.clone(),
        skills: SkillsStore::Off,
        kits: Vec::new(),
        template: None,
        deny_network: Vec::new(),
        ttl: None,
    })?;
    // How the user left agy doesn't matter once the token file is there.
    let attached = backend.attach(SANDBOX);

    let token = read(backend, READ_TOKEN).map_err(|_| {
        let why = attached
            .err()
            .map(|e| format!(" ({e:#})"))
            .unwrap_or_default();
        anyhow!(
            "found no Antigravity token file in {SANDBOX}: the sign-in didn't finish{why}; run \
             `sbxm auth antigravity` again"
        )
    })?;
    let refresh_token = serde_json::from_str::<serde_json::Value>(&token)
        .ok()
        .and_then(|v| v.get("refresh_token")?.as_str().map(str::to_owned))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "agy's token file in {SANDBOX} has no refresh_token; run `sbxm auth antigravity` \
                 again"
            )
        })?;

    let found = read(backend, READ_CLIENTS)
        .map_err(|e| anyhow!("cannot read agy's OAuth clients in {SANDBOX}: {e:#}"))?;
    let words: Vec<&str> = found.split_whitespace().collect();
    let ids = words
        .iter()
        .filter(|w| w.ends_with(".apps.googleusercontent.com"));
    let secrets: Vec<&&str> = words.iter().filter(|w| w.starts_with("GOCSPX-")).collect();
    let pairs: Vec<(String, String)> = ids
        .flat_map(|id| secrets.iter().map(|s| ((*id).to_owned(), (**s).to_owned())))
        .collect();
    if pairs.is_empty() {
        bail!(
            "found no OAuth client id and secret in agy's binary in {SANDBOX}: agy may have \
             changed how it signs in; set antigravity_auth = \"api-key\" meanwhile"
        );
    }
    Ok((refresh_token, pairs))
}

/// Runs `script` in the sandbox and returns its output, which never goes into an error.
fn read(backend: &dyn SandboxBackend, script: &str) -> Result<String> {
    let output = backend.exec(
        SANDBOX,
        &ExecSpec {
            workdir: None,
            argv: vec!["sh".into(), "-c".into(), script.into()],
            stdin: Stdin::Closed,
        },
    )?;
    if output.exit_code != Some(0) {
        bail!("exited with {:?}", output.exit_code);
    }
    Ok(output.stdout)
}
