//! Where a task runs when no `--cloud` or `--local` says (milestone 5 slice 2, issue 600,
//! decisions 317 and 321): the `run-cloud` and `run-local` labels pin an issue's location for the
//! loop, and a fix runs where the task that opened its PR ran.

use std::path::Path;

use anyhow::Result;

use super::jobs;
use crate::backend::Location;
use crate::github::{GitHubBackend, PrInfo};

/// The label that runs an issue's task in the cloud.
pub const RUN_CLOUD_LABEL: &str = "run-cloud";
/// The label that runs an issue's task on this machine.
pub const RUN_LOCAL_LABEL: &str = "run-local";
/// Why the loop skips a `run-cloud` issue while `[resources] cloud_slots` is unset: the cloud is
/// opt-in for the loop (decision 317(3)).
pub const NEEDS_CLOUD_SLOTS: &str = "run-cloud needs [resources] cloud_slots";

/// The location an issue's labels pin, `None` for neither; both is the reason the loop skips it.
pub fn pinned(labels: &[String]) -> Result<Option<Location>, String> {
    let has = |label: &str| labels.iter().any(|l| l == label);
    match (has(RUN_CLOUD_LABEL), has(RUN_LOCAL_LABEL)) {
        (true, true) => Err(format!(
            "labeled both {RUN_CLOUD_LABEL} and {RUN_LOCAL_LABEL}; take one off"
        )),
        (true, false) => Ok(Some(Location::Cloud)),
        (false, true) => Ok(Some(Location::Local)),
        (false, false) => Ok(None),
    }
}

/// Where the loop runs an issue labeled `labels`, `None` leaving it to `[sandbox] location` as
/// before; `Err` is why the loop skips it, creating nothing: both labels, or `run-cloud` while
/// `cloud_slots` is unset.
pub fn for_loop(labels: &[String], cloud_slots: Option<u32>) -> Result<Option<Location>, String> {
    let pinned = pinned(labels)?;
    if pinned == Some(Location::Cloud) && cloud_slots.is_none() {
        return Err(NEEDS_CLOUD_SLOTS.to_owned());
    }
    Ok(pinned)
}

/// Where a fix of PR `pr` of `repo` runs with neither `--cloud` nor `--local`: the location
/// [`recorded`] by the task that opened it, else the one its closing issue's label pins
/// ([`labeled`]); else `None`, the default. The cloud needs no `cloud_slots` here: only the
/// loop's own choice of it does.
pub fn of_pr(
    base_dir: &Path,
    repo: &str,
    pr: u32,
    github: &dyn GitHubBackend,
) -> Result<Option<Location>> {
    if let Some(location) = recorded(base_dir, repo, pr) {
        return Ok(Some(location));
    }
    labeled(github, repo, &github.pr(repo, pr)?)
}

/// The location recorded by the task that opened PR `pr` of `repo` ([`jobs::opener`], an archived
/// record too); `None` without one (a hand-made PR, or the task removed).
pub fn recorded(base_dir: &Path, repo: &str, pr: u32) -> Option<Location> {
    jobs::opener(base_dir, repo, pr).map(|rec| rec.location)
}

/// The location the `run-cloud` or `run-local` label of the first open issue `pr` closes pins;
/// `None` for no such issue, neither label or both.
pub fn labeled(github: &dyn GitHubBackend, repo: &str, pr: &PrInfo) -> Result<Option<Location>> {
    if pr.closing_issues.is_empty() {
        return Ok(None);
    }
    let open = github.issues_open(repo)?;
    Ok(pr.closing_issues.iter().find_map(|n| {
        let issue = open.iter().find(|i| i.number == *n)?;
        pinned(&issue.labels).ok().flatten()
    }))
}
