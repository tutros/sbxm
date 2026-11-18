//! `sbxm task ready-check --issue N [--json]` (issue 725): the readiness rules of
//! [`crate::task::ready`] for one open issue, one line per rule (`pass`, `fail`, `warn` or `skip`
//! with its reason), or a JSON object with `--json`. Ready when no rule fails; warns never make an
//! issue unready. Reads only: GitHub is read, nothing is written anywhere.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Serialize;

use super::task_start::resolve_repo;
use crate::github::GitHubBackend;
use crate::task::ready::{self, Status, Verdict};

pub struct Options {
    /// The checkout whose origin names the GitHub repo.
    pub repo_root: PathBuf,
    /// For tests: the GitHub repo, `owner/name`, instead of the checkout's origin.
    pub repo: Option<String>,
    pub issue: u32,
    /// A JSON object instead of one line per rule.
    pub json: bool,
}

#[derive(Serialize)]
struct Report<'a> {
    issue: u32,
    ready: bool,
    rules: &'a [Verdict],
}

/// Prints every rule's verdict on issue `opts.issue`; `Ok(true)` when none fails.
pub fn run(opts: &Options, github: &dyn GitHubBackend, out: &mut dyn Write) -> Result<bool> {
    let number = opts.issue;
    if number == 0 {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let open = github.issues_open(&repo)?;
    let Some(issue) = open.iter().find(|i| i.number == number) else {
        bail!("#{number} isn't an open issue of {repo}");
    };
    let verdicts = ready::check(&repo, github, issue);
    let ready = verdicts.iter().all(|v| v.status != Status::Fail);
    if opts.json {
        let report = Report {
            issue: number,
            ready,
            rules: &verdicts,
        };
        writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    } else {
        for v in &verdicts {
            writeln!(out, "{} {}: {}", v.status.word(), v.rule, v.reason)?;
        }
    }
    Ok(ready)
}
