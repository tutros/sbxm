//! Everything `sbxm run` checks before it writes a file or calls `sbx`:
//! the seed dir, the profiles, the stored secrets and the warnings
//! (decisions 11, 30, 58, 98, 102). Read-only, apart from `sbx secret ls`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::config::RunConfig;
use crate::backend::SandboxBackend;
use crate::config::{GlobalConfig, Profile};
use crate::harness::Harness;
use crate::seed;

#[derive(Debug)]
pub struct Preflight {
    /// One line each, without the `warning: ` prefix.
    pub warnings: Vec<String>,
}

pub fn check(
    config_dir: &Path,
    run_config: &RunConfig,
    backend: &dyn SandboxBackend,
) -> Result<Preflight> {
    let global = GlobalConfig::load(config_dir)?;
    if let Some(seed_dir) = &run_config.task.seed {
        check_seed(seed_dir, &global.base_dir, config_dir)?;
    }

    // The run's profile applies to every contestant and the judge unless a
    // contestant sets its own (decision 115).
    let run_profile = run_config
        .run
        .profile
        .clone()
        .unwrap_or_else(|| global.default_profile.clone());
    let profile_of = |own: Option<&String>| own.cloned().unwrap_or_else(|| run_profile.clone());
    let mut profiles: Vec<(String, Profile)> = Vec::new();
    let mut load = |name: String| -> Result<()> {
        if !profiles.iter().any(|(n, _)| *n == name) {
            let profile = Profile::load(global.profiles_dir(), &name)?;
            profiles.push((name, profile));
        }
        Ok(())
    };
    for c in &run_config.contestants {
        load(profile_of(c.profile.as_ref()))?;
    }
    load(run_profile.clone())?;

    // Every needed secret, deduplicated, with the first reason it's needed.
    let mut needed: Vec<(String, String)> = Vec::new();
    let mut need = |service: String, reason: String| {
        if !needed.iter().any(|(s, _)| *s == service) {
            needed.push((service, reason));
        }
    };
    for (i, c) in run_config.contestants.iter().enumerate() {
        need(
            c.harness.provider_secret().to_owned(),
            format!("contestants[{i}], {}", c.harness.as_str()),
        );
    }
    if let Some(judge) = &run_config.eval.judge {
        need(
            judge.harness.provider_secret().to_owned(),
            format!("eval.judge, {}", judge.harness.as_str()),
        );
    }
    for (name, profile) in &profiles {
        for service in &profile.secrets.services {
            need(
                service.clone(),
                format!("secrets.services of profile '{name}'"),
            );
        }
    }
    let stored = backend.secret_services()?;
    if let Some((missing, reason)) = needed.iter().find(|(s, _)| !stored.contains(s)) {
        bail!(
            "secret '{missing}' (needed by {reason}) is not stored in sbx; add it with \
             `sbx secret set {missing}` or import it with `sbx setup`"
        );
    }

    let mut warnings = Vec::new();
    if run_config.run.budget_usd.is_some() {
        for (i, c) in run_config.contestants.iter().enumerate() {
            if c.harness.budget_flag(run_config.run.budget_usd).is_none() {
                warnings.push(format!(
                    "run.budget_usd is set, but contestants[{i}] ({}) has no budget flag: only \
                     the timeout limits its cost",
                    c.harness.as_str()
                ));
            }
        }
    }
    let profile_named = |name: &str| &profiles.iter().find(|(n, _)| n == name).unwrap().1;
    let mut unsupported = |harness: Harness, profile: &str, at: String| {
        for warning in harness.unsupported(profile_named(profile), &at) {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
    };
    for (i, c) in run_config.contestants.iter().enumerate() {
        unsupported(
            c.harness,
            &profile_of(c.profile.as_ref()),
            format!("the sandbox of contestants[{i}]"),
        );
    }
    if let Some(judge) = &run_config.eval.judge {
        unsupported(judge.harness, &run_profile, "the judge's sandbox".into());
    }
    Ok(Preflight { warnings })
}

/// The same checks `new --seed` makes (decision 47), before anything is copied.
fn check_seed(seed_dir: &Path, base_dir: &Path, config_dir: &Path) -> Result<()> {
    if !seed_dir.is_dir() {
        bail!(
            "seed {} is not a directory; point task.seed at a folder",
            seed_dir.display()
        );
    }
    if !base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }
    let seed_abs = fs::canonicalize(seed_dir)
        .with_context(|| format!("cannot resolve seed {}", seed_dir.display()))?;
    let base_abs = fs::canonicalize(base_dir)
        .with_context(|| format!("cannot resolve base dir {}", base_dir.display()))?;
    if base_abs.starts_with(&seed_abs) {
        bail!(
            "seed {} contains the base dir {}; copying it would recurse into itself",
            seed_dir.display(),
            base_dir.display()
        );
    }
    seed::reject_links(seed_dir)
}
