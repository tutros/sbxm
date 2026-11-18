//! `sbxm task ready-check --issue N [--json]` (issue 725): the readiness rules of
//! [`crate::task::ready`] for one open issue, one line per rule (`pass`, `fail`, `warn` or `skip`
//! with its reason), or a JSON object with `--json`. Ready when no rule fails; warns never make an
//! issue unready. Reads only: GitHub is read, nothing is written anywhere.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::task_start::resolve_repo;
use crate::github::{GitHubBackend, Issue};
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
    let issue = open_issue(&repo, github, number)?;
    let verdicts = ready::check(&repo, github, &issue);
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

/// Issue `number` read by itself (review M-1): its text from `gh issue view`, the body after its
/// `--` line, and its labels, every page. Never looked for in the open-issue listing, which holds
/// only the newest open issues. Refused when it is closed or can't be read.
fn open_issue(repo: &str, github: &dyn GitHubBackend, number: u32) -> Result<Issue> {
    let text = github
        .issue(repo, number)
        .with_context(|| format!("cannot read issue #{number} of {repo}"))?;
    if !text.state.eq_ignore_ascii_case("open") {
        bail!(
            "#{number} isn't an open issue of {repo} (it is {})",
            text.state
        );
    }
    let labels = github
        .issue_label_names(repo, number)
        .with_context(|| format!("cannot read the labels of issue #{number} of {repo}"))?;
    let body = match text.text.lines().position(|line| line.trim_end() == "--") {
        Some(at) => text
            .text
            .lines()
            .skip(at + 1)
            .collect::<Vec<_>>()
            .join("\n"),
        None => text.text.clone(),
    };
    Ok(Issue {
        number,
        open: true,
        title: text.title,
        labels,
        body,
    })
}
