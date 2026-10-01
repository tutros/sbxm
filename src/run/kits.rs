//! A run's kits: generated once, up front, for every (profile, harness) pair
//! in use, and validated before any sandbox exists (decisions 55, 95, 108,
//! 112, 115). The same `kit` functions as `sbxm new` build them. A contestant
//! without its own `profile` uses the run's, and so does the judge.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::config::RunConfig;
use crate::backend::{SandboxBackend, SkillsStore};
use crate::commands::invalid_kit_check;
use crate::config::{self, GlobalConfig, Profile, Resources};
use crate::harness::Harness;
use crate::kit;

/// The kits of one (profile, harness) pair, ready for `CreateSpec::kits`.
#[derive(Debug, Clone)]
pub struct HarnessKits {
    /// The profile these kits were built from.
    pub profile: String,
    pub harness: Harness,
    /// Hash of the merged config for this harness, with the run's `cpus` and
    /// `memory` overrides applied (decisions 55, 108).
    pub config_hash: String,
    /// `common` then `harness-<h>`, the order `sbx create --kit` needs.
    pub dirs: Vec<PathBuf>,
    /// This profile's `skills.store`, passed to `sbx create --skills`.
    pub skills_store: SkillsStore,
}

#[derive(Debug)]
pub struct RunKits {
    /// The run-level profile: the default for contestants and the judge.
    pub profile_name: String,
    /// The global `[resources]` with the run's overrides applied: what every
    /// sandbox of the run is created with.
    pub resources: Resources,
    /// The run-level profile's `skills.store`.
    pub skills_store: SkillsStore,
    /// One entry per (profile, harness) used by a contestant or the judge, in
    /// first-use order.
    pub harnesses: Vec<HarnessKits>,
}

impl RunKits {
    /// The kits of `harness` under the run-level profile.
    pub fn get(&self, harness: Harness) -> Option<&HarnessKits> {
        self.get_for(&self.profile_name, harness)
    }

    pub fn get_for(&self, profile: &str, harness: Harness) -> Option<&HarnessKits> {
        self.harnesses
            .iter()
            .find(|h| h.harness == harness && h.profile == profile)
    }
}

/// Writes every kit under `kits_root` (never mounted into a sandbox, decision
/// 112) and validates each with `sbx`. Any failure returns before a sandbox
/// could be created, so an invalid kit leaves nothing behind.
pub fn build(
    config_dir: &Path,
    run_config: &RunConfig,
    kits_root: &Path,
    backend: &dyn SandboxBackend,
) -> Result<RunKits> {
    let global = GlobalConfig::load(config_dir)?;
    let run_profile = run_config
        .run
        .profile
        .clone()
        .unwrap_or_else(|| global.default_profile.clone());
    let overrides = Overrides {
        cpus: run_config.run.cpus,
        memory: run_config.run.memory.clone(),
    };

    // The (profile, harness) pairs in first-use order: each contestant under
    // its own profile or the run's, then the judge under the run's.
    let mut wanted: Vec<(String, Harness)> = Vec::new();
    let judge = run_config
        .eval
        .judge
        .as_ref()
        .map(|j| (run_profile.clone(), j.harness));
    let contestants = run_config.contestants.iter().map(|c| {
        (
            c.profile.clone().unwrap_or_else(|| run_profile.clone()),
            c.harness,
        )
    });
    for pair in contestants.chain(judge) {
        if !wanted.contains(&pair) {
            wanted.push(pair);
        }
    }
    build_pairs(&global, run_profile, &overrides, wanted, kits_root, backend)
}

/// `cpus` and `memory` that replace the global `[resources]` for one set of kits.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub cpus: Option<u32>,
    pub memory: Option<String>,
}

/// Kits for `harnesses` under one `profile`, for callers with no run-config (`sbxm task`).
/// Same writing, hashing and validation as [`build`].
pub fn build_for(
    config_dir: &Path,
    profile: &str,
    harnesses: &[Harness],
    overrides: &Overrides,
    kits_root: &Path,
    backend: &dyn SandboxBackend,
) -> Result<RunKits> {
    let global = GlobalConfig::load(config_dir)?;
    let mut wanted: Vec<(String, Harness)> = Vec::new();
    for harness in harnesses {
        let pair = (profile.to_owned(), *harness);
        if !wanted.contains(&pair) {
            wanted.push(pair);
        }
    }
    build_pairs(
        &global,
        profile.to_owned(),
        overrides,
        wanted,
        kits_root,
        backend,
    )
}

fn build_pairs(
    global: &GlobalConfig,
    run_profile: String,
    overrides: &Overrides,
    wanted: Vec<(String, Harness)>,
    kits_root: &Path,
    backend: &dyn SandboxBackend,
) -> Result<RunKits> {
    let resources = Resources {
        cpus: overrides.cpus.unwrap_or(global.resources.cpus),
        memory: overrides
            .memory
            .clone()
            .unwrap_or_else(|| global.resources.memory.clone()),
    };

    // Every profile loads before anything is written, so an unknown one
    // leaves no trace.
    let mut profiles: Vec<(String, Profile)> = Vec::new();
    for (name, _) in &wanted {
        if !profiles.iter().any(|(n, _)| n == name) {
            profiles.push((name.clone(), Profile::load(global.profiles_dir(), name)?));
        }
    }
    let profile_named = |name: &str| &profiles.iter().find(|(n, _)| n == name).unwrap().1;

    let mut all = Vec::new();
    for (profile_name, harness) in wanted {
        let profile = profile_named(&profile_name);
        let config_hash = config::config_hash(&profile_name, profile, &resources, harness);
        let dir = kits_root.join(&config_hash[..12]);
        let specs: Vec<_> = kit::all(&profile_name, profile, &config_hash, harness)
            .map(|(name, spec)| (dir.join(name), spec))
            .into_iter()
            .collect();
        for (path, spec) in &specs {
            kit::write(path, spec)?;
        }
        all.push(HarnessKits {
            profile: profile_name,
            harness,
            config_hash,
            dirs: specs.into_iter().map(|(path, _)| path).collect(),
            skills_store: profile.skills_store,
        });
    }

    for kits in &all {
        for dir in &kits.dirs {
            let validation = backend.validate_kit(dir)?;
            if !validation.valid {
                let checked = invalid_kit_check(global.profiles_dir(), &kits.profile, None);
                bail!(
                    "generated kit {} is invalid: {}; check {checked}",
                    dir.display(),
                    validation.error.as_deref().unwrap_or("no details from sbx"),
                );
            }
        }
    }
    Ok(RunKits {
        skills_store: profile_named(&run_profile).skills_store,
        profile_name: run_profile,
        resources,
        harnesses: all,
    })
}
