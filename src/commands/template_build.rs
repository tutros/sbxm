//! `sbxm template build <profile>` (decisions 201, 206, 209-213).

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::{self, GlobalConfig, Profile};
use crate::harness::Harness;
use crate::run::results::format_timestamp;
use crate::template;
use crate::{disk, kit};

#[derive(Debug, Default)]
pub struct Options {
    pub profile: String,
    pub harness: Harness,
    /// The free-space check of the sandbox drive (decision 194).
    pub disk: disk::Guard,
    /// Seconds since the Unix epoch for the tag and record; the clock when `None`.
    pub now: Option<u64>,
}

pub fn run(
    config_dir: &Path,
    options: &Options,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
) -> Result<()> {
    template::check_supported(options.harness)?;
    let config = GlobalConfig::load(config_dir)?;
    let profile = Profile::load(config.profiles_dir(), &options.profile)?;
    if profile.template.is_none() {
        bail!(
            "profile '{}' has no [template] table; add one with its runtime allow list to {} to \
             opt in",
            options.profile,
            config
                .profiles_dir()
                .join(&options.profile)
                .join("profile.toml")
                .display()
        );
    }
    let sbx_version = backend.version()?;
    template::check_sbx_version(&sbx_version)?;
    disk::require(&config, config_dir, &options.disk)?;
    let now = match options.now {
        Some(now) => now,
        None => SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    let profile_name = options.profile.as_str();
    let template_hash = kit::template_hash(&profile, options.harness, false);
    let tag = template::tag(profile_name, options.harness, &template_hash, now);
    // Saving onto an existing tag replaces it without a word (S11, question 8).
    if backend.template_ls()?.iter().any(|t| t.reference() == tag) {
        bail!("template {tag} already exists; wait a minute and build again");
    }

    let key = template::key(profile_name, options.harness);
    let build_dir = template::dir(&config.base_dir).join("build");
    let workspace = build_dir.join(&key);
    let kits_dir = build_dir.join(format!("{key}.kits"));
    let config_hash =
        config::config_hash(profile_name, &profile, &config.resources, options.harness);
    let kits = kit::all(profile_name, &profile, &config_hash, options.harness)
        .map(|(name, spec)| (kits_dir.join(name), spec));
    for (dir, spec) in &kits {
        kit::write(dir, spec)?;
    }
    for (dir, _) in &kits {
        let validation = backend.validate_kit(dir)?;
        if !validation.valid {
            bail!(
                "generated kit {} is invalid: {}; check profile '{profile_name}'",
                dir.display(),
                validation.error.as_deref().unwrap_or("no details from sbx"),
            );
        }
    }
    fs::create_dir_all(&workspace)
        .with_context(|| format!("cannot create {}", workspace.display()))?;

    let sandbox = format!("sbxm-template-{key}");
    backend.create(&CreateSpec {
        name: sandbox.clone(),
        agent: options.harness.agent_arg().into(),
        workspace: workspace.clone(),
        cpus: config.resources.cpus,
        memory: config.resources.memory.clone(),
        skills: profile.skills_store,
        kits: kits.into_iter().map(|(dir, _)| dir).collect(),
        template: None,
    })?;
    backend.stop(&sandbox)?;
    backend.template_save(&sandbox, &tag)?;
    backend.remove(&sandbox)?;
    for dir in [&workspace, &kits_dir] {
        fs::remove_dir_all(dir).with_context(|| format!("cannot remove {}", dir.display()))?;
    }

    let size_bytes = backend
        .template_ls()?
        .into_iter()
        .find(|t| t.reference() == tag)
        .map(|t| t.size)
        .with_context(|| {
            format!("template {tag} is missing after saving it; run `sbx template ls`")
        })?;
    template::Record {
        tag: tag.clone(),
        template_hash,
        warm: false,
        lock_files: Vec::new(),
        lock_hash: None,
        sbx_version,
        size_bytes,
        created_at: format_timestamp(now),
        previous: Vec::new(),
    }
    .save(&config.base_dir, &key)?;
    writeln!(out, "{tag}")?;
    Ok(())
}
