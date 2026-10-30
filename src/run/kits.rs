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
use crate::template;

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
    /// The template the pair's sandboxes are created from, `None` for the install steps
    /// (decision 237).
    pub template: Option<String>,
    /// [`template::denied_hosts`] for the pair's sandboxes.
    pub deny_network: Option<Vec<String>>,
    /// The loaded profile, for [`template::check_codex_rules`] after each create.
    pub loaded: Profile,
    /// sbxm's own path, for an Antigravity sandbox signed in with the user's login (decision
    /// 270): each one gets a scoped secret served by it and a placeholder token file. `None`
    /// otherwise.
    pub login: Option<PathBuf>,
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

/// The (profile, harness) pairs of a run in first-use order: each contestant under its own
/// profile or the run's (`run_profile`), then the judge under the run's.
pub fn pairs(run_config: &RunConfig, run_profile: &str) -> Vec<(String, Harness)> {
    let mut wanted: Vec<(String, Harness)> = Vec::new();
    let judge = run_config
        .eval
        .judge
        .as_ref()
        .map(|j| (run_profile.to_owned(), j.harness));
    let contestants = run_config.contestants.iter().map(|c| {
        (
            c.profile.clone().unwrap_or_else(|| run_profile.to_owned()),
            c.harness,
        )
    });
    for pair in contestants.chain(judge) {
        if !wanted.contains(&pair) {
            wanted.push(pair);
        }
    }
    wanted
}

/// Writes every kit under `kits_root` (never mounted into a sandbox, decision
/// 112) and validates each with `sbx`. Any failure returns before a sandbox
/// could be created, so an invalid kit leaves nothing behind. Every pair uses the
/// install steps; [`build_with_templates`] takes the preflight's choice.
pub fn build(
    config_dir: &Path,
    run_config: &RunConfig,
    kits_root: &Path,
    backend: &dyn SandboxBackend,
) -> Result<RunKits> {
    build_with_templates(config_dir, run_config, &[], kits_root, backend)
}

/// Like [`build`], with the template `templates` names for a (profile, harness) pair (the
/// preflight's choice, decision 237); a pair it doesn't name uses the install steps.
pub fn build_with_templates(
    config_dir: &Path,
    run_config: &RunConfig,
    templates: &[(String, Harness, Option<String>)],
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
    let wanted = pairs(run_config, &run_profile)
        .into_iter()
        .map(|(profile, harness)| {
            let template = templates
                .iter()
                .find(|(p, h, _)| *p == profile && *h == harness)
                .and_then(|(_, _, tag)| tag.clone());
            (profile, harness, template)
        })
        .collect();
    build_pairs(&global, run_profile, &overrides, wanted, kits_root, backend)
}

/// `cpus` and `memory` that replace the global `[resources]` for one set of kits.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub cpus: Option<u32>,
    pub memory: Option<String>,
}

/// Kits for `harnesses` under one `profile`, for callers with no run-config (`sbxm task`).
/// Same writing, hashing and validation as [`build`]. With a `template` tag, the baked variant
/// for a sandbox created from it (decisions 200, 230).
pub fn build_for(
    config_dir: &Path,
    profile: &str,
    harnesses: &[Harness],
    overrides: &Overrides,
    template: Option<&str>,
    kits_root: &Path,
    backend: &dyn SandboxBackend,
) -> Result<RunKits> {
    let global = GlobalConfig::load(config_dir)?;
    let mut wanted: Vec<(String, Harness, Option<String>)> = Vec::new();
    for harness in harnesses {
        if !wanted.iter().any(|(_, h, _)| h == harness) {
            wanted.push((profile.to_owned(), *harness, template.map(str::to_owned)));
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
    wanted: Vec<(String, Harness, Option<String>)>,
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
    for (name, _, _) in &wanted {
        if !profiles.iter().any(|(n, _)| n == name) {
            profiles.push((name.clone(), Profile::load(global.profiles_dir(), name)?));
        }
    }
    let profile_named = |name: &str| &profiles.iter().find(|(n, _)| n == name).unwrap().1;

    let mut all = Vec::new();
    for (profile_name, harness, template) in wanted {
        let profile = profile_named(&profile_name);
        let table = template.as_ref().and(profile.template.as_ref());
        let config_hash = match table {
            Some(_) => config::template_config_hash,
            None => config::config_hash,
        }(&profile_name, profile, &resources, harness);
        let dir = kits_root.join(&config_hash[..12]);
        let specs: Vec<_> = match table {
            Some(table) => {
                kit::all_from_template(&profile_name, profile, table, &config_hash, harness)
            }
            None => kit::all(&profile_name, profile, &config_hash, harness),
        }
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
            deny_network: template::denied_hosts(profile, harness, template.as_deref()),
            template,
            loaded: profile.clone(),
            login: None,
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
