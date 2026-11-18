//! `sbxm task ready-check --issue N [--json]` (issue 725): the readiness rules of
//! [`crate::task::ready`] for one open issue, one line per rule (`pass`, `fail`, `warn` or `skip`
//! with its reason), or a JSON object with `--json`. Ready when no rule fails; warns never make an
//! issue unready. Reads only: GitHub is read, nothing is written anywhere.
//!
//! With `--judge` (issue 726, decision 331(2)) the rules are followed by one model call in a
//! fresh sandbox ([`crate::task::ready_judge`]): its advice is printed with its cost, and an
//! answer that doesn't parse is printed raw with a warning (exit 2). `--write-files` then writes
//! the `**Files:**` line it proposes into the issue body, only with `--yes`. It never sets a label.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::task_start::resolve_repo;
use crate::backend::SandboxBackend;
use crate::disk;
use crate::github::{GitHubBackend, Issue};
use crate::task::ready::{self, Status, Verdict};
use crate::task::ready_judge;

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

/// What `--judge` adds (issue 726).
pub struct Judge {
    /// Where `config.toml` and its profiles are read.
    pub config_dir: PathBuf,
    /// The drive check and the sandbox slot, as for every command creating a sandbox.
    pub guard: disk::Guard,
    /// The model's time limit, enforced inside the sandbox.
    pub time_limit: Duration,
    /// `--write-files`: write the proposed `**Files:**` line into the issue body.
    pub write_files: bool,
    /// `--yes`: really write it; without, the line and the command to write it are printed.
    pub yes: bool,
}

/// Prints every rule's verdict on issue `opts.issue`; `Ok(true)` when none fails.
pub fn run(opts: &Options, github: &dyn GitHubBackend, out: &mut dyn Write) -> Result<bool> {
    let (repo, issue) = read_issue(opts, github)?;
    print_rules(opts, &repo, &issue, github, out)
}

/// The repo and issue `opts.issue`, open.
fn read_issue(opts: &Options, github: &dyn GitHubBackend) -> Result<(String, Issue)> {
    let number = opts.issue;
    if number == 0 {
        bail!("issue numbers start at 1; pass --issue <n>");
    }
    let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
    let issue = open_issue(&repo, github, number)?;
    Ok((repo, issue))
}

/// [`run`]'s output for an issue already read.
fn print_rules(
    opts: &Options,
    repo: &str,
    issue: &Issue,
    github: &dyn GitHubBackend,
    out: &mut dyn Write,
) -> Result<bool> {
    let number = issue.number;
    let verdicts = ready::check(repo, github, issue);
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

/// `--judge`: the rules as [`run`] prints them, then the model's advice and its cost; with
/// `write_files`, the proposed `**Files:**` line goes into the issue body when `yes` is set. The
/// exit code: 2 when the answer doesn't parse, else 1 when a rule fails, else 0. Every GitHub read
/// the model needs is done before a sandbox exists.
pub fn run_judged(
    opts: &Options,
    judge: &Judge,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<i32> {
    if opts.json {
        bail!("--judge doesn't go with --json");
    }
    let (repo, issue) = read_issue(opts, github)?;
    let ready = print_rules(opts, &repo, &issue, github, out)?;
    let inputs = ready_judge::gather(&repo, github, &issue)?;
    out.flush()?;
    let reply = ready_judge::run(
        &judge.config_dir,
        &judge.guard,
        judge.time_limit,
        issue.number,
        &inputs,
        backend,
        warn,
    )?;
    let cost = reply
        .cost_usd
        .map_or_else(|| "cost unknown".to_owned(), |usd| format!("${usd:.2}"));
    let model = reply.model.as_deref().unwrap_or("its default model");
    writeln!(
        out,
        "judge (claude, {model}): {}s, {cost}; advice only, the label stays yours",
        reply.seconds
    )?;
    let answer = match ready_judge::parse(&reply.answer) {
        Ok(answer) => answer,
        Err(why) => {
            writeln!(
                warn,
                "warning: the judge's answer isn't in the format ({why}); here it is as given"
            )?;
            writeln!(out, "{}", reply.answer.trim_end())?;
            return Ok(2);
        }
    };
    for (name, text) in answer.sections() {
        if text.contains('\n') {
            writeln!(out, "{name}:\n{text}")?;
        } else {
            writeln!(out, "{name}: {text}")?;
        }
    }
    if judge.write_files {
        write_files(opts, judge, &repo, &answer, github, out)?;
    }
    Ok(if ready { 0 } else { 1 })
}

/// `--write-files`: the proposed `**Files:**` line into the issue's current body (read again,
/// so an edit made while the model ran is kept), only with `--yes`.
fn write_files(
    opts: &Options,
    judge: &Judge,
    repo: &str,
    answer: &ready_judge::Answer,
    github: &dyn GitHubBackend,
    out: &mut dyn Write,
) -> Result<()> {
    let number = opts.issue;
    let Some(line) = answer.files_line() else {
        writeln!(
            out,
            "the judge proposes no **Files:** line, so nothing is written"
        )?;
        return Ok(());
    };
    writeln!(out, "proposed: {line}")?;
    if !judge.yes {
        writeln!(
            out,
            "to write it into #{number}, run `sbxm task ready-check --issue {number} --judge \
             --write-files --yes` (the judge runs again)"
        )?;
        return Ok(());
    }
    let issue = open_issue(repo, github, number)?;
    let body = ready_judge::with_files_line(&issue.body, &line);
    if body == issue.body {
        writeln!(out, "#{number} already has this **Files:** line")?;
        return Ok(());
    }
    github
        .issue_edit(repo, number, &body)
        .with_context(|| format!("cannot write the **Files:** line into issue #{number}"))?;
    writeln!(out, "wrote the **Files:** line into #{number}")?;
    Ok(())
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
