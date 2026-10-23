//! `sbxm template build <profile>` (decisions 201, 206, 209-213).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Error, Result, anyhow, bail};

use crate::backend::{CreateSpec, SandboxBackend};
use crate::config::{self, GlobalConfig, Profile};
use crate::harness::Harness;
use crate::run::checks::timed_exec;
use crate::run::orchestrate::in_sandbox_path;
use crate::run::results::format_timestamp;
use crate::task::config::DEFAULT_GATE_TIMEOUT;
use crate::task::gates::{self, CARGO_ENV};
use crate::task::record::{Process, ProcessProbe};
use crate::template;
use crate::{disk, kit, seed, task};

#[derive(Debug, Default)]
pub struct Options {
    pub profile: String,
    pub harness: Harness,
    /// The free-space check of the sandbox drive (decision 194).
    pub disk: disk::Guard,
    /// Seconds since the Unix epoch for the tag and record; the clock when `None`.
    pub now: Option<u64>,
    /// The repo whose warm-up fills the template's caches (decision 207).
    pub warm: Option<PathBuf>,
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
    let warm = options.warm.as_deref().map(Warm::check).transpose()?;
    let sbx_version = backend.version()?;
    template::check_sbx_version(&sbx_version)?;
    disk::require(&config, config_dir, &options.disk)?;
    let now = match options.now {
        Some(now) => now,
        None => SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    let profile_name = options.profile.as_str();
    let template_hash = kit::template_hash(&profile, options.harness, warm.is_some());
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
        warm: warm.clone(),
        workspace: build_dir.join(&key),
        kits_dir: build_dir.join(format!("{key}.kits")),
        key,
        record: template::Record {
            tag,
            template_hash,
            warm: warm.is_some(),
            lock_files: warm
                .as_ref()
                .map(|w| w.lock_files.clone())
                .unwrap_or_default(),
            lock_hash: warm.as_ref().map(|w| w.lock_hash.clone()),
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
                users
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
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

/// The built-in warm-up of a repo with a `Cargo.toml` (decision 207), in the default gates'
/// environment so sandboxes reuse the compiled dependencies.
const CARGO_STEPS: [&str; 3] = [
    "cargo fetch --locked",
    "cargo clippy --all-targets --locked",
    "cargo test --no-run --locked",
];
const CARGO_STEP_ENV: &str = "CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/tmp/target";

/// What `--warm <repo>` warms the template with, checked before any write.
#[derive(Clone)]
struct Warm {
    repo: PathBuf,
    /// The repo's `[warm]` commands; `None` runs the built-in cargo steps.
    commands: Option<Vec<String>>,
    lock_files: Vec<String>,
    lock_hash: String,
}

impl Warm {
    fn check(repo: &Path) -> Result<Self> {
        if !repo.is_dir() {
            bail!(
                "repo {} is not a directory; point --warm at the repo to warm the template with",
                repo.display()
            );
        }
        if let Some(warm) = task::config::load_warm(repo)? {
            seed::reject_repo_links(repo)?;
            let lock_hash = template::lock_hash(repo, &warm.lock_files)?;
            return Ok(Self {
                repo: repo.to_path_buf(),
                commands: Some(warm.commands),
                lock_files: warm.lock_files,
                lock_hash,
            });
        }
        if !repo.join("Cargo.toml").is_file() {
            bail!(
                "repo {} has neither a Cargo.toml nor a [warm] table in its sbxm-task.toml; add \
                 [warm] with the commands that fill its caches",
                repo.display()
            );
        }
        let lock = repo.join("Cargo.lock");
        if !lock.is_file() {
            bail!(
                "lock file {} does not exist; run `cargo generate-lockfile` in {} and build again",
                lock.display(),
                repo.display()
            );
        }
        seed::reject_repo_links(repo)?;
        let lock_files = vec!["Cargo.lock".to_owned()];
        let lock_hash = template::lock_hash(repo, &lock_files)?;
        Ok(Self {
            repo: repo.to_path_buf(),
            commands: None,
            lock_files,
            lock_hash,
        })
    }
}

/// Why a warm-up step stopped the build, with the end of its output.
fn step_failed(repo: &Path, failed: &gates::GateOutcome) -> Error {
    let command = &failed.result.command;
    let what = if failed.could_not_run {
        "could not run".to_owned()
    } else if failed.timed_out {
        format!(
            "timed out after {} minutes",
            DEFAULT_GATE_TIMEOUT.as_secs() / 60
        )
    } else {
        match failed.result.exit {
            Some(code) => format!("failed with exit {code}"),
            None => "failed".to_owned(),
        }
    };
    anyhow!(
        "warm-up step `{command}` {what}; fix it in {} and build again; its output ends:\n{}",
        repo.display(),
        failed.output_tail.trim_end()
    )
}

/// Writes one progress line and flushes it, so a long step after it is not mistaken for a hang.
fn announce(warn: &mut dyn Write, line: &str) -> Result<()> {
    writeln!(warn, "{line}")?;
    warn.flush()?;
    Ok(())
}

/// The names and folders one build uses (decision 210), and its record before the size is known.
struct Build {
    profile_name: String,
    harness: Harness,
    sandbox: String,
    /// What to warm the template with; its repo is copied into the workspace (decision 207).
    warm: Option<Warm>,
    workspace: PathBuf,
    kits_dir: PathBuf,
    key: String,
    record: template::Record,
}

/// Held for a whole build (decision 212); dropping it removes the lock file, whether the build
/// passed or failed.
struct BuildLock {
    path: PathBuf,
    /// The lock's contents: removed only while it still names this process.
    me: String,
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|text| text == self.me) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The right to clear an interrupted build (`<key>.build.takeover`); dropping it gives it up.
struct TakeoverClaim {
    path: PathBuf,
}

impl Drop for TakeoverClaim {
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
        for _ in 0..3 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let lock = BuildLock {
                        path,
                        me: me.clone(),
                    };
                    file.write_all(me.as_bytes())
                        .with_context(|| format!("cannot write {}", lock.path.display()))?;
                    return Ok(lock);
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("cannot create {}", path.display()));
                }
            }
            let Some((text, held)) = self.read_lock(&path)? else {
                continue;
            };
            if held.alive(probe) {
                bail!(
                    "a template build of {} is already running (process {}, started {}); wait \
                     for it to finish",
                    self.key,
                    held.pid,
                    held.started_at
                );
            }
            // Only the build that creates the takeover file clears an interrupted build, and only
            // if the lock is still the one it found dead: a build that took over meanwhile is live
            // and its sandbox must stay (slice 4 review M-1).
            let takeover = dir.join(format!("{}.build.takeover", self.key));
            let _claim = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&takeover)
            {
                Ok(_) => TakeoverClaim {
                    path: takeover.clone(),
                },
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => bail!(
                    "another template build of {} is clearing an interrupted build ({}); try \
                     again, or delete it if no template build is running",
                    self.key,
                    takeover.display()
                ),
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("cannot create {}", takeover.display()));
                }
            };
            if self.read_lock(&path)?.map(|(again, _)| again) != Some(text) {
                continue;
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

    /// The lock's text and the process it names, or `None` if it is gone.
    fn read_lock(&self, path: &Path) -> Result<Option<(String, Process)>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
        };
        let held = serde_json::from_str(&text).with_context(|| {
            format!(
                "cannot read the build lock {}; if no template build of {} is running, delete it",
                path.display(),
                self.key
            )
        })?;
        Ok(Some((text, held)))
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
        match &self.warm {
            Some(warm) => seed::copy_repo(&warm.repo, &self.workspace)?,
            None => fs::create_dir_all(&self.workspace)
                .with_context(|| format!("cannot create {}", self.workspace.display()))?,
        }

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
            deny_network: Vec::new(),
        })?;
        if let Some(warm) = &self.warm {
            self.warm_up(warm, backend, warn)?;
        }
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

    /// Runs the repo's `[warm]` commands in the build sandbox, or else the built-in cargo steps
    /// and then cleans the repo's own packages out of the target folder so only the dependencies
    /// stay (decision 207). Each step is announced on `warn` before it runs (issue 277).
    fn warm_up(
        &self,
        warm: &Warm,
        backend: &dyn SandboxBackend,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let repo = warm.repo.as_path();
        let workdir = in_sandbox_path(&self.workspace);
        let run = |command: String| -> Result<()> {
            let outcomes = gates::run_sandbox_tier(
                backend,
                &self.sandbox,
                &workdir,
                "warm",
                &[command],
                DEFAULT_GATE_TIMEOUT,
            );
            match outcomes.iter().find(|o| !o.result.passed) {
                Some(failed) => Err(step_failed(repo, failed)),
                None => Ok(()),
            }
        };
        if let Some(commands) = &warm.commands {
            for (i, command) in commands.iter().enumerate() {
                announce(
                    warn,
                    &format!("warm-up {} of {}: {command}", i + 1, commands.len()),
                )?;
                run(command.clone())?;
            }
            return Ok(());
        }
        for (i, step) in CARGO_STEPS.iter().enumerate() {
            announce(
                warn,
                &format!("warm-up {} of {}: {step}", i + 1, CARGO_STEPS.len()),
            )?;
            run(format!("{CARGO_STEP_ENV} {step}"))?;
        }
        announce(warn, "warm-up: cargo metadata --no-deps")?;
        let packages = self.packages(repo, backend, &workdir)?;
        for (j, package) in packages.iter().enumerate() {
            announce(
                warn,
                &format!(
                    "warm-up: cargo clean -p {package} ({} of {})",
                    j + 1,
                    packages.len()
                ),
            )?;
            run(format!("{CARGO_STEP_ENV} cargo clean -p {package}"))?;
        }
        Ok(())
    }

    /// The repo's own packages, as `cargo metadata --no-deps` names them in the build sandbox.
    fn packages(
        &self,
        repo: &Path,
        backend: &dyn SandboxBackend,
        workdir: &Path,
    ) -> Result<Vec<String>> {
        #[derive(serde::Deserialize)]
        struct Metadata {
            packages: Vec<Package>,
        }
        #[derive(serde::Deserialize)]
        struct Package {
            name: String,
        }
        let command = "cargo metadata --no-deps --format-version 1";
        let output = backend.exec(
            &self.sandbox,
            &timed_exec(
                workdir,
                DEFAULT_GATE_TIMEOUT.as_secs(),
                &format!("{CARGO_ENV}{command}"),
            ),
        )?;
        if output.exit_code != Some(0) {
            bail!(
                "warm-up step `{command}` failed{}; fix it in {} and build again; its output \
                 ends:\n{}",
                output
                    .exit_code
                    .map(|code| format!(" with exit {code}"))
                    .unwrap_or_default(),
                repo.display(),
                output.stderr.trim_end()
            );
        }
        let metadata: Metadata = serde_json::from_str(&output.stdout)
            .with_context(|| format!("cannot read the output of `{command}`"))?;
        let names: Vec<String> = metadata.packages.into_iter().map(|p| p.name).collect();
        // Each name goes into a shell command.
        if let Some(bad) = names.iter().find(|name| {
            name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }) {
            bail!(
                "cargo metadata named the package `{bad}`, which is not a valid package name; \
                 check {}",
                repo.join("Cargo.toml").display()
            );
        }
        Ok(names)
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
