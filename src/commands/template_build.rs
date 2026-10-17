//! `sbxm template build <profile>` (decisions 201, 206, 209-213).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Error, Result, anyhow, bail};

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
    let build = Build {
        sandbox: format!("sbxm-template-{key}"),
        workspace: build_dir.join(&key),
        kits_dir: build_dir.join(format!("{key}.kits")),
        key,
        record: template::Record {
            tag,
            template_hash,
            warm: false,
            lock_files: Vec::new(),
            lock_hash: None,
            sbx_version,
            size_bytes: 0,
            created_at: format_timestamp(now),
            previous: Vec::new(),
        },
    };
    let mut progress = Progress::default();
    build
        .run(
            &config,
            profile_name,
            &profile,
            options.harness,
            backend,
            &mut progress,
        )
        .map_err(|err| build.clean_up(backend, &progress, err))?;
    writeln!(out, "{}", build.record.tag)?;
    Ok(())
}

/// The names and folders one build uses (decision 210), and its record before the size is known.
struct Build {
    sandbox: String,
    workspace: PathBuf,
    kits_dir: PathBuf,
    key: String,
    record: template::Record,
}

/// What a build has made so far, for [`Build::clean_up`].
#[derive(Default)]
struct Progress {
    /// Set before `create`: a failed `sbx create` can leave a sandbox behind.
    sandbox: bool,
    /// Set once the tag is saved, until its record is written.
    saved: bool,
}

impl Build {
    /// Writes and validates the kits, creates, stops, saves and removes the build sandbox, then
    /// writes the record.
    fn run(
        &self,
        config: &GlobalConfig,
        profile_name: &str,
        profile: &Profile,
        harness: Harness,
        backend: &dyn SandboxBackend,
        progress: &mut Progress,
    ) -> Result<()> {
        let config_hash = config::config_hash(profile_name, profile, &config.resources, harness);
        let kits = kit::all(profile_name, profile, &config_hash, harness)
            .map(|(name, spec)| (self.kits_dir.join(name), spec));
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
        fs::create_dir_all(&self.workspace)
            .with_context(|| format!("cannot create {}", self.workspace.display()))?;

        let tag = &self.record.tag;
        progress.sandbox = true;
        backend.create(&CreateSpec {
            name: self.sandbox.clone(),
            agent: harness.agent_arg().into(),
            workspace: self.workspace.clone(),
            cpus: config.resources.cpus,
            memory: config.resources.memory.clone(),
            skills: profile.skills_store,
            kits: kits.into_iter().map(|(dir, _)| dir).collect(),
            template: None,
        })?;
        backend.stop(&self.sandbox)?;
        backend.template_save(&self.sandbox, tag)?;
        progress.saved = true;
        backend.remove(&self.sandbox)?;
        progress.sandbox = false;
        for dir in [&self.workspace, &self.kits_dir] {
            fs::remove_dir_all(dir).with_context(|| format!("cannot remove {}", dir.display()))?;
        }

        let size_bytes = backend
            .template_ls()?
            .into_iter()
            .find(|t| t.reference() == *tag)
            .map(|t| t.size)
            .with_context(|| {
                format!("template {tag} is missing after saving it; run `sbx template ls`")
            })?;
        template::Record {
            size_bytes,
            ..self.record.clone()
        }
        .save(&config.base_dir, &self.key)?;
        progress.saved = false;
        Ok(())
    }

    /// Removes what a failed build made: the build sandbox, a tag saved without its record, and
    /// the build folders. Each cleanup failure is added to `err` with the command that finishes
    /// the job; `err` stays the cause.
    fn clean_up(&self, backend: &dyn SandboxBackend, progress: &Progress, err: Error) -> Error {
        let mut leftovers = Vec::new();
        if progress.sandbox && backend.remove(&self.sandbox).is_err() {
            leftovers.push(format!(
                "could not remove build sandbox {0}; remove it with `sbx rm {0}`",
                self.sandbox
            ));
        }
        let tag = &self.record.tag;
        if progress.saved && backend.template_rm(tag).is_err() {
            leftovers.push(format!(
                "could not remove template {tag}; remove it with `sbx template rm {tag}`"
            ));
        }
        for dir in [&self.workspace, &self.kits_dir] {
            if dir.exists() && fs::remove_dir_all(dir).is_err() {
                leftovers.push(format!(
                    "could not remove {}; delete it by hand",
                    dir.display()
                ));
            }
        }
        if leftovers.is_empty() {
            err
        } else {
            anyhow!("{err:#}\n{}", leftovers.join("\n"))
        }
    }
}
