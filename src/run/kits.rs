//! A run's kits: generated once, up front, for every harness in use, and
//! validated before any sandbox exists (decisions 55, 95, 108, 112). The
//! same `kit` functions as `sbxm new` build them, from the run's profile.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::config::RunConfig;
use crate::backend::{SandboxBackend, SkillsStore};
use crate::commands::invalid_kit_check;
use crate::config::{self, GlobalConfig, Profile, Resources};
use crate::harness::Harness;
use crate::kit;

/// The kits of one harness, ready for `CreateSpec::kits`.
#[derive(Debug, Clone)]
pub struct HarnessKits {
    pub harness: Harness,
    /// Hash of the merged config for this harness, with the run's `cpus` and
    /// `memory` overrides applied (decisions 55, 108).
    pub config_hash: String,
    /// `common` then `harness-<h>`, the order `sbx create --kit` needs.
    pub dirs: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct RunKits {
    pub profile_name: String,
    /// The global `[resources]` with the run's overrides applied: what every
    /// sandbox of the run is created with.
    pub resources: Resources,
    /// The profile's `skills.store`, passed to `sbx create --skills`.
    pub skills_store: SkillsStore,
    /// One entry per harness used by a contestant or the judge, in first-use order.
    pub harnesses: Vec<HarnessKits>,
}

impl RunKits {
    pub fn get(&self, harness: Harness) -> Option<&HarnessKits> {
        self.harnesses.iter().find(|h| h.harness == harness)
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
    let profile_name = run_config
        .run
        .profile
        .clone()
        .unwrap_or_else(|| global.default_profile.clone());
    for (i, c) in run_config.contestants.iter().enumerate() {
        if c.profile.as_ref().is_some_and(|p| *p != profile_name) {
            bail!(
                "contestants[{i}].profile is set, but per-contestant profiles are not supported \
                 yet; remove it and use run.profile for all contestants"
            );
        }
    }
    let profile = Profile::load(global.profiles_dir(), &profile_name)?;
    let resources = Resources {
        cpus: run_config.run.cpus.unwrap_or(global.resources.cpus),
        memory: run_config
            .run
            .memory
            .clone()
            .unwrap_or_else(|| global.resources.memory.clone()),
    };

    let mut harnesses: Vec<Harness> = Vec::new();
    let judge = run_config.eval.judge.as_ref().map(|j| j.harness);
    for harness in run_config
        .contestants
        .iter()
        .map(|c| c.harness)
        .chain(judge)
    {
        if !harnesses.contains(&harness) {
            harnesses.push(harness);
        }
    }

    let mut all = Vec::new();
    for harness in harnesses {
        let config_hash = config::config_hash(&profile_name, &profile, &resources, harness);
        let dir = kits_root.join(&config_hash[..12]);
        let specs: Vec<_> = kit::all(&profile_name, &profile, &config_hash, harness)
            .map(|(name, spec)| (dir.join(name), spec))
            .into_iter()
            .collect();
        for (path, spec) in &specs {
            kit::write(path, spec)?;
        }
        all.push(HarnessKits {
            harness,
            config_hash,
            dirs: specs.into_iter().map(|(path, _)| path).collect(),
        });
    }

    for dir in all.iter().flat_map(|h| &h.dirs) {
        let validation = backend.validate_kit(dir)?;
        if !validation.valid {
            let checked = invalid_kit_check(global.profiles_dir(), &profile_name, None);
            bail!(
                "generated kit {} is invalid: {}; check {checked}",
                dir.display(),
                validation.error.as_deref().unwrap_or("no details from sbx"),
            );
        }
    }
    Ok(RunKits {
        profile_name,
        resources,
        skills_store: profile.skills_store,
        harnesses: all,
    })
}
