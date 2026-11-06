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

use super::finish;
use super::record::{Kind, ProcessProbe, Record};
use super::repo;
use crate::backend::SandboxBackend;
use crate::commands::template_build::{self, Refreshed};
use crate::config::{GlobalConfig, Profile};
use crate::disk;
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
    /// The invoking command's free-space check, for the rebuild it may start (decision 194).
    pub disk: &'a disk::Guard,
    /// A fix of a pull request (decision 292): its worker merges the base first, so a head that
    /// only inherits an older lock than main's starts from main's template instead of stopping.
    pub merges_base: bool,
    /// The task the sandbox is for, so a branch behind the template is told its way out.
    pub task: Task,
}

/// What [`refresh`] needs to name the way out for a task whose branch only inherits older lock
/// files than the template's (issue 424).
#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub kind: Kind,
    /// The issue's number; unused for a spec task.
    pub number: u32,
    /// The pull request the task has (its `task.json` records one), if any.
    pub pr: Option<u32>,
}

impl Task {
    /// The task `record` is: its PR is the one `task finish` opened, else the one it continues, else
    /// the one a PR or fix task is of.
    pub fn of(record: &Record) -> Self {
        let pr = record
            .pr
            .as_deref()
            .and_then(finish::pr_number)
            .or_else(|| record.continues.as_ref().map(|c| c.pr))
            .or_else(|| matches!(record.kind, Kind::Pr | Kind::Fix).then_some(record.number));
        Self {
            kind: record.kind,
            number: record.number,
            pr,
        }
    }

    /// The command that brings the task's branch up to `main`'s lock files.
    fn way_out(&self, main: &str) -> String {
        let restart = match (self.pr, self.kind) {
            (Some(pr), _) => {
                return format!(
                    "merge {main} into its branch with `sbxm task fix --pr {pr}` (decision 292)"
                );
            }
            (None, Kind::Spec) => "--spec <file>".to_owned(),
            (None, _) => format!("--issue {}", self.number),
        };
        format!(
            "start it again from {main} with `sbxm task run --restart {restart}` (its old task is \
             set aside, and put back if the restart fails: decision 185)"
        )
    }
}

/// The stop for a commit whose lock files are the ones its branch forked with, older than the
/// ones the template follows now: no rebuild helps, since the next task would rebuild it back.
fn behind_main(
    profile_name: &str,
    harness: Harness,
    main: &str,
    commit: &str,
    files: &[String],
    task: &Task,
) -> anyhow::Error {
    anyhow!(
        "the template of profile '{profile_name}' for {} follows {main}'s lock files now \
         (decision 290), and commit {commit} has the older ones ({}) its branch forked with; {}",
        harness.as_str(),
        files.join(", "),
        task.way_out(main)
    )
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
         sandbox of strict profile '{profile_name}' can't fetch crates (decision 291). {}",
        files.join(", "),
        hand_back(base)
    )
}

/// The stop for a lock file that can't be read at `commit`, a deleted or oversized one included
/// (decision 236): not evidence that nothing changed.
fn lock_unreadable(
    profile_name: &str,
    base: &str,
    commit: &str,
    files: &[String],
    why: &anyhow::Error,
) -> anyhow::Error {
    anyhow!(
        "the lock files ({}) can't be read at commit {commit}: {why:#}; that counts as a change \
         from {base}'s: {BRANCH_CHANGED}, and a sandbox of strict profile '{profile_name}' can't \
         fetch crates (decision 291). {}",
        files.join(", "),
        hand_back(base)
    )
}

fn hand_back(base: &str) -> String {
    format!(
        "Needs a decision: a worker doesn't add, remove or bump a crate, the user approves each \
         one; put the crate change on {base} in a pull request of its own, naming the crate, its \
         version and why it's needed, and once it is merged the template is rebuilt from {base} \
         by itself"
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

/// What a branch did to the lock files of a warm template.
enum Verdict {
    /// Its lock files are what it inherited from the base, or what `main` has by now.
    Fine,
    /// Its own commits changed them to something `main` doesn't have (yet).
    Changed,
    /// They can't be read at the commit: deleted, oversized (decision 236), or damaged. Not
    /// evidence that nothing changed.
    Unreadable(anyhow::Error),
}

/// Judges `commit`'s lock `files`. The branch's own change is a difference from where it left
/// `base` (their merge base: a pull request made before the base moved inherits an older lock, and
/// that is not a change of its own), unless `main`'s tip (`fresh`, fetched only when needed) has
/// those very bytes: the user approved the crate there (decision 291).
fn judge(
    repo_git: &Path,
    base: &str,
    commit: &str,
    files: &[String],
    fresh: &dyn Fn() -> Result<String>,
) -> Result<Verdict> {
    let hash = |commit: &str| Locks::Commit { repo_git, commit }.hash(files);
    let at_commit = match hash(commit) {
        Ok(hash) => hash,
        Err(why) => return Ok(Verdict::Unreadable(why)),
    };
    let base_tip = repo::branch_tip(repo_git, base)?;
    let forked = repo::merge_base(repo_git, &base_tip, commit).unwrap_or(base_tip);
    if hash(&forked).is_ok_and(|at_forked| at_forked == at_commit) {
        return Ok(Verdict::Fine);
    }
    if hash(&fresh()?).is_ok_and(|at_main| at_main == at_commit) {
        return Ok(Verdict::Fine);
    }
    Ok(Verdict::Changed)
}

/// The stop for a [`Verdict`] that isn't `Fine`.
fn stop(
    verdict: Verdict,
    profile_name: &str,
    base: &str,
    commit: &str,
    files: &[String],
) -> Result<()> {
    match verdict {
        Verdict::Fine => Ok(()),
        Verdict::Changed => Err(branch_changed(profile_name, base, commit, files)),
        Verdict::Unreadable(why) => Err(lock_unreadable(profile_name, base, commit, files, &why)),
    }
}

/// Before a task's `harness` sandbox is chosen a template for `locks`: a stale warm template of a
/// strict profile is rebuilt from the base's tip as `origin` has it now (one line for `events`), then
/// a commit whose own history changes the lock files, or whose lock files can't be read, stops with
/// [`branch_changed`]. Returns the commit whose lock files the template is to be chosen for when
/// that isn't `commit`: for a fix that merges the base ([`Refresh::merges_base`]) whose head only
/// inherits an older lock, main's tip, so the worker starts from main's template to merge it.
pub fn refresh(
    r: &Refresh,
    profile_name: &str,
    profile: &Profile,
    harness: Harness,
    locks: Locks,
    events: &mut Vec<String>,
) -> Result<Option<String>> {
    let Locks::Commit { repo_git, commit } = locks else {
        return Ok(None);
    };
    let global = GlobalConfig::load(r.config_dir)?;
    let Some(mut record) =
        warm_record(&global.base_dir, r.backend, profile_name, profile, harness)?
    else {
        return Ok(None);
    };
    let hash_at = |commit: &str, record: &template::Record| {
        Locks::Commit { repo_git, commit }
            .hash(&record.lock_files)
            .ok()
    };
    // The template follows `main` as `origin` has it now (decision 290), never the task's base: an
    // unmerged base or an older one (a resumed task) must not decide what every later task inherits.
    let (main, fresh) = repo::fetch_fresh_base(repo_git)?;
    if let Some(fresh_hash) = hash_at(&fresh, &record)
        && record.lock_hash.as_deref() != Some(fresh_hash.as_str())
    {
        events.push(rebuild(
            r,
            profile_name,
            harness,
            repo_git,
            (&main, &fresh),
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
    let verdict = judge(repo_git, r.base, commit, &record.lock_files, &|| {
        Ok(fresh.clone())
    })?;
    stop(verdict, profile_name, r.base, commit, &record.lock_files)?;
    // Inherited, and older than what the template was warmed with: `template::choose` would refuse
    // it, so the refusal names what brings the branch up to main instead (issue 424).
    if !r.merges_base
        && record.warm
        && hash_at(commit, &record).as_deref() != record.lock_hash.as_deref()
    {
        return Err(behind_main(
            profile_name,
            harness,
            &main,
            commit,
            &record.lock_files,
            &r.task,
        ));
    }
    // Unchanged by the branch but not what the template was warmed with: the base moved on.
    let inherits_older = r.merges_base
        && hash_at(&fresh, &record)
            .is_some_and(|at_main| hash_at(commit, &record) != Some(at_main));
    Ok(inherits_older.then_some(fresh))
}

/// Whether `tip`'s lock files are the branch's own change, or can't be read, for any warm template
/// of the strict profile `profile_name` the task's `harnesses` use: the stop of [`refresh`] if so.
/// Checked right after a worker or fix round, before its gates (decision 291).
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
    for &harness in harnesses {
        let Some(record) = warm_record(&global.base_dir, backend, profile_name, &profile, harness)?
        else {
            continue;
        };
        let verdict = judge(repo_git, base, tip, &record.lock_files, &|| {
            repo::fetch_fresh_base(repo_git).map(|(_, fresh)| fresh)
        })?;
        stop(verdict, profile_name, base, tip, &record.lock_files)?;
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
    (main, base_tip): (&str, &str),
    base_hash: &str,
    files: &[String],
) -> Result<String> {
    let base = main;
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
                disk: r.disk.clone(),
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
