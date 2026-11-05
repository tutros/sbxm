//! A strict profile's warm template and the lock files of a task's branch (decisions 290, 291).
//!
//! When the base branch's lock files differ from the ones the template was warmed with, the
//! template is rebuilt from a clean checkout of the base's tip in the task's host-owned
//! `repo.git` (never from agent-written code), and the task goes on. When the task's own branch
//! changes them, it stops with a hand-back: a worker never adds a crate, the user approves each
//! one, and the crate reaches the base in a pull request of its own.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::record::ProcessProbe;
use super::repo;
use crate::backend::SandboxBackend;
use crate::commands::template_build::{self, Refreshed};
use crate::config::{GlobalConfig, Profile};
use crate::harness::Harness;
use crate::kit;
use crate::template::{self, Locks};

/// The words every branch-changed-lock stop carries, whichever stage hits it.
pub const BRANCH_CHANGED: &str = "this branch's own commits change them";

/// What [`refresh`] needs from the task.
pub struct Refresh<'a> {
    pub config_dir: &'a Path,
    pub backend: &'a dyn SandboxBackend,
    pub probe: &'a dyn ProcessProbe,
    /// Waits between checks of another build's lock.
    pub sleep: &'a (dyn Fn(Duration) + Sync),
    /// The task's base branch in the `repo.git` its sandbox's commit is read from.
    pub base: &'a str,
    /// A folder of the task's own, for the base's checkout while a rebuild runs.
    pub scratch: &'a Path,
}

/// The stop shared by every stage (decisions 290, 291): `commit`'s lock files differ from
/// `base`'s, so a strict sandbox couldn't build it, and adding a crate is the user's decision.
pub fn branch_changed(
    profile_name: &str,
    base: &str,
    commit: &str,
    files: &[String],
) -> anyhow::Error {
    anyhow!(
        "the lock files ({}) at commit {commit} differ from {base}'s: {BRANCH_CHANGED}, and a \
         sandbox of strict profile '{profile_name}' can't fetch crates (decision 291). Needs a \
         decision: a worker doesn't add, remove or bump a crate, the user approves each one; put \
         the crate change on {base} in a pull request of its own, naming the crate, its version \
         and why it's needed, and once it is merged the template is rebuilt from {base} by itself",
        files.join(", ")
    )
}

/// The warm template a strict profile's `harness` sandbox would be created from, when it is one
/// this module looks after: built for the profile as it is now, and still in `sbx`. Anything else
/// is left to [`template::choose`], which says what is wrong.
fn warm_record(
    base_dir: &Path,
    backend: &dyn SandboxBackend,
    profile_name: &str,
    profile: &Profile,
    harness: Harness,
) -> Result<Option<template::Record>> {
    if !profile.template.as_ref().is_some_and(|t| t.strict)
        || !matches!(harness, Harness::Claude | Harness::Codex)
        || template::check_sbx_version(&backend.version()?).is_err()
    {
        return Ok(None);
    }
    let Some(record) = template::Record::load(base_dir, &template::key(profile_name, harness))?
    else {
        return Ok(None);
    };
    if !record.warm
        || record.template_hash != kit::template_hash(profile, harness, true)
        || !backend
            .template_ls()?
            .iter()
            .any(|t| t.reference() == record.tag)
    {
        return Ok(None);
    }
    Ok(Some(record))
}

/// Before a task's `harness` sandbox is chosen a template for `locks`: a stale warm template of a
/// strict profile is rebuilt from the base's tip as `origin` has it now (one line for `events`), then
/// a commit whose lock files differ from the task's base stops with [`branch_changed`]. A lock file that can't be read is
/// left to [`template::choose`], which names why.
pub fn refresh(
    r: &Refresh,
    profile_name: &str,
    profile: &Profile,
    harness: Harness,
    locks: Locks,
    events: &mut Vec<String>,
) -> Result<()> {
    let Locks::Commit { repo_git, commit } = locks else {
        return Ok(());
    };
    let global = GlobalConfig::load(r.config_dir)?;
    let Some(mut record) =
        warm_record(&global.base_dir, r.backend, profile_name, profile, harness)?
    else {
        return Ok(());
    };
    let base_tip = repo::branch_tip(repo_git, r.base)?;
    let hash_at = |commit: &str, record: &template::Record| {
        Locks::Commit { repo_git, commit }
            .hash(&record.lock_files)
            .ok()
    };
    let Some(at_task_base) = hash_at(&base_tip, &record) else {
        return Ok(());
    };
    if record.lock_hash.as_deref() != Some(at_task_base.as_str()) {
        // The task's base may be older than the base is now (a resumed task): the template
        // follows the base's tip as `origin` has it now, never an older one.
        let fresh = repo::fetch_fresh_base(repo_git, r.base)?;
        if let Some(fresh_hash) = hash_at(&fresh, &record)
            && record.lock_hash.as_deref() != Some(fresh_hash.as_str())
        {
            events.push(rebuild(
                r,
                profile_name,
                harness,
                repo_git,
                &fresh,
                &fresh_hash,
                &record.lock_files,
            )?);
            // The base's own `[warm]` may name other lock files.
            if let Some(rebuilt) =
                template::Record::load(&global.base_dir, &template::key(profile_name, harness))?
            {
                record = rebuilt;
            }
        }
    }
    // What the task's own commits changed: against the base it started from.
    let Some(at_task_base) = hash_at(&base_tip, &record) else {
        return Ok(());
    };
    if locks
        .hash(&record.lock_files)
        .is_ok_and(|at_commit| at_commit != at_task_base)
    {
        return Err(branch_changed(
            profile_name,
            r.base,
            commit,
            &record.lock_files,
        ));
    }
    Ok(())
}

/// Whether `tip`'s lock files differ from `base`'s for any warm template of the strict profile
/// `profile_name` the task's `harnesses` use: [`branch_changed`] if they do. Checked right after a
/// worker or fix round, before its gates (decision 291).
pub fn check_branch(
    config_dir: &Path,
    backend: &dyn SandboxBackend,
    profile_name: &str,
    harnesses: &[Harness],
    repo_git: &Path,
    base: &str,
    tip: &str,
) -> Result<()> {
    let global = GlobalConfig::load(config_dir)?;
    let profile = Profile::load(global.profiles_dir(), profile_name)?;
    let base_tip = repo::branch_tip(repo_git, base)?;
    for &harness in harnesses {
        let Some(record) = warm_record(&global.base_dir, backend, profile_name, &profile, harness)?
        else {
            continue;
        };
        let hash = |commit: &str| {
            Locks::Commit { repo_git, commit }
                .hash(&record.lock_files)
                .ok()
        };
        if let (Some(at_base), Some(at_tip)) = (hash(&base_tip), hash(tip))
            && at_base != at_tip
        {
            return Err(branch_changed(profile_name, base, tip, &record.lock_files));
        }
    }
    Ok(())
}

/// Rebuilds the template from a clean checkout of `base_tip`, the base as just fetched into
/// [`repo::FRESH_BASE`] (decision 290) and returns the line
/// that says so; a failed rebuild stops the task, naming the `template build` command.
fn rebuild(
    r: &Refresh,
    profile_name: &str,
    harness: Harness,
    repo_git: &Path,
    base_tip: &str,
    base_hash: &str,
    files: &[String],
) -> Result<String> {
    let base = r.base;
    let flag = match harness {
        Harness::Claude => String::new(),
        other => format!(" --harness {}", other.as_str()),
    };
    let checkout = r
        .scratch
        .join(format!("template-base-{}", harness.as_str()));
    let built = (|| {
        if checkout.exists() {
            fs::remove_dir_all(&checkout)?;
        }
        repo::clean_checkout(repo_git, repo::FRESH_BASE, &checkout)?;
        let (mut out, mut progress) = (Vec::new(), Vec::new());
        template_build::refresh(
            r.config_dir,
            &template_build::Options {
                profile: profile_name.to_owned(),
                harness,
                warm: Some(checkout.clone()),
                ..Default::default()
            },
            base_hash,
            r.sleep,
            r.backend,
            r.probe,
            &mut out,
            &mut progress,
        )
    })();
    let _ = fs::remove_dir_all(&checkout);
    let stale = format!(
        "the template of profile '{profile_name}' for {} was warmed with other lock files ({}) \
         than {base}'s",
        harness.as_str(),
        files.join(", ")
    );
    match built {
        Ok(Refreshed::Built(tag)) => Ok(format!(
            "{stale}; rebuilt it from {base}'s tip {base_tip} (decision 290): {tag}"
        )),
        Ok(Refreshed::Fresh(tag)) => Ok(format!(
            "{stale}; another task rebuilt it from {base} meanwhile: {tag}"
        )),
        Err(err) => Err(anyhow!(
            "{stale}, and rebuilding it from {base}'s tip {base_tip} failed: {err:#}; rebuild it \
             with `sbxm template build {profile_name}{flag} --warm <a clean checkout of {base}>`"
        )),
    }
}
