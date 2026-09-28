use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use super::harness_flag;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::{self, GlobalConfig, Profile};
use crate::harness::Harness;
use crate::kit;
use crate::project;
use crate::seed;
use crate::state::{self, SandboxState, State};

#[derive(Debug, Default)]
pub struct Options {
    /// Directory whose contents are copied into a new workspace.
    pub seed: Option<PathBuf>,
    /// Profile to apply; `default_profile` from the global config if `None`.
    pub profile: Option<String>,
    /// Remove the existing sandbox after the new kit validates and before
    /// creating (`open --rebuild`), so an invalid kit leaves it untouched.
    pub replace: bool,
    /// The project's state entry for this harness is stale: its sandbox is
    /// gone from `sbx` (`open` recreating it), so the entry doesn't block.
    pub recreate: bool,
    pub harness: Harness,
}

pub fn run(
    config_dir: &Path,
    name: &str,
    options: &Options,
    backend: &dyn SandboxBackend,
    warn: &mut dyn Write,
) -> Result<()> {
    project::validate_name(name)?;
    let config = GlobalConfig::load(config_dir)?;
    let profile_name = options
        .profile
        .as_deref()
        .unwrap_or(&config.default_profile);
    let profile = Profile::load(config.profiles_dir(), profile_name)?
        .with_project(&project::metadata_dir(&config.base_dir, name))?;
    let config_hash =
        config::config_hash(profile_name, &profile, &config.resources, options.harness);

    if !config.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            config.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }
    let harness = options.harness.as_str();
    let sandbox = project::sandbox_name(name, harness);
    let metadata_dir = project::metadata_dir(&config.base_dir, name);
    if !options.replace
        && !options.recreate
        && state::load(&metadata_dir)?.is_some_and(|s| s.sandboxes.contains_key(harness))
    {
        bail!(
            "sandbox {sandbox} already exists; open it with `sbxm open {name}{}`",
            harness_flag(options.harness)
        );
    }
    let workspace = config.base_dir.join(name);
    if let Some(seed_dir) = &options.seed {
        if workspace.exists() {
            bail!(
                "project {} already exists; run without --seed to reuse it",
                workspace.display()
            );
        }
        if !seed_dir.is_dir() {
            bail!(
                "seed {} is not a directory; pass a folder with --seed",
                seed_dir.display()
            );
        }
        let seed_abs = fs::canonicalize(seed_dir)
            .with_context(|| format!("cannot resolve seed {}", seed_dir.display()))?;
        let base_abs = fs::canonicalize(&config.base_dir)
            .with_context(|| format!("cannot resolve base dir {}", config.base_dir.display()))?;
        if base_abs.starts_with(&seed_abs) {
            bail!(
                "seed {} contains the base dir {}; copying it would recurse into itself",
                seed_dir.display(),
                config.base_dir.display()
            );
        }
        seed::reject_links(seed_dir)?;
    }
    check_secrets(&profile.secrets.services, backend)?;
    // Named after the hash, so a changed config never overwrites the kit an
    // existing sandbox was built from (decision 55).
    let kits_dir = project::metadata_dir(&config.base_dir, name)
        .join("kits")
        .join(&config_hash[..12]);
    let kits = kit::all(profile_name, &profile, &config_hash, options.harness)
        .map(|(name, spec)| (kits_dir.join(name), spec));
    for (dir, spec) in &kits {
        kit::write(dir, spec)?;
    }
    for (dir, _) in &kits {
        let validation = backend.validate_kit(dir)?;
        for warning in &validation.warnings {
            writeln!(warn, "warning: kit {}: {warning}", dir.display())?;
        }
        if !validation.valid {
            let profile_toml = config
                .profiles_dir()
                .join(profile_name)
                .join("profile.toml");
            let sandbox_toml = metadata_dir.join("sandbox.toml");
            let mut checked = format!("profile '{profile_name}' ({})", profile_toml.display());
            if sandbox_toml.is_file() {
                checked.push_str(&format!(
                    " and the project's sandbox.toml ({})",
                    sandbox_toml.display()
                ));
            }
            bail!(
                "generated kit {} is invalid: {}; check {checked}",
                dir.display(),
                validation.error.as_deref().unwrap_or("no details from sbx"),
            );
        }
    }

    match &options.seed {
        Some(seed_dir) => seed::copy(seed_dir, &workspace)?,
        None => fs::create_dir_all(&workspace)
            .with_context(|| format!("cannot create {}", workspace.display()))?,
    }

    for warning in options.harness.unsupported(&profile, &sandbox) {
        writeln!(warn, "warning: {warning}")?;
    }
    if options.replace {
        backend.remove(&sandbox)?;
    }
    backend.create(&CreateSpec {
        name: sandbox.clone(),
        agent: options.harness.agent_arg().into(),
        workspace: workspace.clone(),
        cpus: config.resources.cpus,
        memory: config.resources.memory,
        skills: profile.skills_store,
        kits: kits.into_iter().map(|(dir, _)| dir).collect(),
    })?;

    let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut state = state::load(&metadata_dir)?.unwrap_or_else(State::default);
    state.sandboxes.insert(
        harness.into(),
        SandboxState {
            sandbox,
            workspace,
            created_at,
            profile: Some(profile_name.to_owned()),
            config_hash: Some(config_hash),
        },
    );
    state.save(&metadata_dir)
}

/// Every named service must be a stored `sbx` secret, or the sandbox would
/// start without it (decisions 30, 58). sbxm never sets secrets itself.
fn check_secrets(services: &[String], backend: &dyn SandboxBackend) -> Result<()> {
    if services.is_empty() {
        return Ok(());
    }
    require_secrets(services, &backend.secret_services()?)
}

/// Fails naming the first of `services` that isn't in `stored`.
pub(super) fn require_secrets(services: &[String], stored: &[String]) -> Result<()> {
    if let Some(missing) = services.iter().find(|s| !stored.contains(s)) {
        bail!(
            "secret '{missing}' (secrets.services) is not stored in sbx; add it with \
             `sbx secret set {missing}` or import it with `sbx setup`"
        );
    }
    Ok(())
}
