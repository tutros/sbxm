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
use crate::task::record::{Process, ProcessProbe};
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
    probe: &dyn ProcessProbe,
    out: &mut dyn Write,
    warn: &mut dyn Write,
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
    if !config.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            config.base_dir.display(),
            config_dir.join("config.toml").display()
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
    let mut build = Build {
        profile_name: profile_name.to_owned(),
        harness: options.harness,
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
    let _lock = build.lock(&config.base_dir, backend, probe, warn)?;
    // Again under the lock: a build that finished since the first check may have saved this tag.
    if backend
        .template_ls()?
        .iter()
        .any(|t| t.reference() == build.record.tag)
    {
        bail!(
            "template {} already exists; wait a minute and build again",
            build.record.tag
        );
    }
    // Read under the lock, so a build that finished meanwhile is not lost. The record lists every
    // earlier tag until it is removed, so one that cannot go now is retried later (decision 225).
    build.record.previous = template::Record::load(&config.base_dir, &build.key)?
        .map(|old| std::iter::once(old.tag).chain(old.previous).collect())
        .unwrap_or_default();
    let mut progress = Progress::default();
    let record = build
        .run(&config, &profile, backend, &mut progress, warn)
        .map_err(|err| build.clean_up(backend, &progress, err))?;
    writeln!(out, "{}", record.tag)?;
    let kept = retire_previous(&config.base_dir, &record.previous, backend, out)?;
    if kept != record.previous {
        template::Record {
            previous: kept,
            ..record
        }
        .save(&config.base_dir, &build.key)?;
    }
    Ok(())
}

/// Removes each earlier tag that no sandbox or task records (decision 205a), once the new record
/// points at the new tag. Returns the tags kept: in use, or not removable now. A tag whose image
/// is already gone is dropped without a word.
fn retire_previous(
    base_dir: &Path,
    previous: &[String],
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
) -> Result<Vec<String>> {
    if previous.is_empty() {
        return Ok(Vec::new());
    }
    let images = match backend.template_ls() {
        Ok(images) => images,
        Err(err) => {
            writeln!(
                out,
                "kept previous templates {}: cannot list templates ({err:#}); a later build \
                 retries",
                previous.join(", ")
            )?;
            return Ok(previous.to_vec());
        }
    };
    let mut kept = Vec::new();
    for tag in previous {
        if !images.iter().any(|t| t.reference() == *tag) {
            continue;
        }
        let users = match template::users(base_dir, tag) {
            Ok(users) => users,
            Err(err) => {
                writeln!(
                    out,
                    "kept previous template {tag}: cannot tell whether it is in use ({err:#}); a \
                     later build retries"
                )?;
                kept.push(tag.clone());
                continue;
            }
        };
        if !users.is_empty() {
            writeln!(
                out,
                "kept previous template {tag}: still used by {}; a later build removes it once \
                 nothing uses it",
                users.join(", ")
            )?;
            kept.push(tag.clone());
            continue;
        }
        match backend.template_rm(tag) {
            Ok(()) => writeln!(out, "removed previous template {tag}")?,
            Err(err) => {
                writeln!(
                    out,
                    "could not remove previous template {tag} ({err:#}); a later build retries, \
                     or remove it with `sbx template rm {tag}`"
                )?;
                kept.push(tag.clone());
            }
        }
    }
    Ok(kept)
}

/// The names and folders one build uses (decision 210), and its record before the size is known.
struct Build {
    profile_name: String,
    harness: Harness,
    sandbox: String,
    workspace: PathBuf,
    kits_dir: PathBuf,
    key: String,
    record: template::Record,
}

/// Held for a whole build (decision 212); dropping it removes the lock file, whether the build
/// passed or failed.
struct BuildLock {
    path: PathBuf,
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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
    /// Takes `<base>/.sbxm/templates/<key>.build.json` for this process. A lock whose process is
    /// alive refuses the build; one whose process is gone has its build's leftovers removed and
    /// is taken over (decision 212). Created with `create_new`, so two builds can't both take it.
    fn lock(
        &self,
        base_dir: &Path,
        backend: &dyn SandboxBackend,
        probe: &dyn ProcessProbe,
        warn: &mut dyn Write,
    ) -> Result<BuildLock> {
        let dir = template::dir(base_dir);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("{}.build.json", self.key));
        let me = serde_json::to_string(&Process::current(probe))?;
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let lock = BuildLock { path };
                    file.write_all(me.as_bytes())
                        .with_context(|| format!("cannot write {}", lock.path.display()))?;
                    return Ok(lock);
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("cannot create {}", path.display()));
                }
            }
            let held: Process = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .with_context(|| {
                    format!(
                        "cannot read the build lock {}; if no template build of {} is running, \
                         delete it",
                        path.display(),
                        self.key
                    )
                })?;
            if held.alive(probe) {
                bail!(
                    "a template build of {} is already running (process {}, started {}); wait \
                     for it to finish",
                    self.key,
                    held.pid,
                    held.started_at
                );
            }
            self.remove_leftovers(backend)?;
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            writeln!(
                warn,
                "removed the leftovers of an interrupted template build (process {}, started {})",
                held.pid, held.started_at
            )?;
        }
        bail!(
            "cannot take the build lock {}; another template build of {} may have started, try \
             again",
            path.display(),
            self.key
        )
    }

    /// An interrupted build's sandbox and folders (decision 210).
    fn remove_leftovers(&self, backend: &dyn SandboxBackend) -> Result<()> {
        if backend.list()?.iter().any(|s| s.name == self.sandbox) {
            backend.remove(&self.sandbox)?;
        }
        for dir in [&self.workspace, &self.kits_dir] {
            if dir.exists() {
                fs::remove_dir_all(dir)
                    .with_context(|| format!("cannot remove {}", dir.display()))?;
            }
        }
        Ok(())
    }

    /// Writes and validates the kits, creates, stops, saves and removes the build sandbox, then
    /// writes the record and returns it.
    fn run(
        &self,
        config: &GlobalConfig,
        profile: &Profile,
        backend: &dyn SandboxBackend,
        progress: &mut Progress,
        warn: &mut dyn Write,
    ) -> Result<template::Record> {
        let (profile_name, harness) = (self.profile_name.as_str(), self.harness);
        let config_hash = config::config_hash(profile_name, profile, &config.resources, harness);
        let kits = kit::all(profile_name, profile, &config_hash, harness)
            .map(|(name, spec)| (self.kits_dir.join(name), spec));
        for (dir, spec) in &kits {
            kit::write(dir, spec)?;
        }
        for (dir, _) in &kits {
            let validation = backend.validate_kit(dir)?;
            for warning in &validation.warnings {
                writeln!(warn, "warning: kit {}: {warning}", dir.display())?;
            }
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
        let record = template::Record {
            size_bytes,
            ..self.record.clone()
        };
        record.save(&config.base_dir, &self.key)?;
        progress.saved = false;
        Ok(record)
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
