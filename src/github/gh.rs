//! `GhBackend`: `GitHubBackend` over the `gh` CLI. Bodies go through stdin (`--body-file -`),
//! never the command line.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use super::{
    CommitStatus, GitHubBackend, Issue, IssueRequest, IssueText, JobLog, ListedPr, Mergeable,
    PrComment, PrInfo, PrMergeState, PrRequest, PrState, RunStatus, WorkflowRun,
};

/// Runs `gh` with arguments and optional stdin, returning stdout.
pub trait GhRunner: Send + Sync {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<String>;
}

struct Process;

impl GhRunner for Process {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<String> {
        let mut child = Command::new("gh")
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not run `gh`; install the GitHub CLI and run `gh auth login`")?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(text.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

pub struct GhBackend {
    runner: Box<dyn GhRunner>,
}

impl Default for GhBackend {
    fn default() -> Self {
        Self::with_runner(Box::new(Process))
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

impl GhBackend {
    pub fn with_runner(runner: Box<dyn GhRunner>) -> Self {
        Self { runner }
    }

    /// Runs `gh`, naming the command in any failure; fix hint included.
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<String> {
        let command = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
        self.runner.run(args, stdin).with_context(|| {
            format!("`gh {command}` failed (is `gh` installed and logged in? try `gh auth status`)")
        })
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, args: &[String]) -> Result<T> {
        let out = self.run(args, None)?;
        let command = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
        serde_json::from_str(&out)
            .with_context(|| format!("could not parse the output of `gh {command}`"))
    }

    fn list_issues(&self, repo: &str, state: &str, limit: u32) -> Result<Vec<Issue>> {
        let raw: Vec<RawIssue> = self.json(&strings(&[
            "issue",
            "list",
            "--repo",
            repo,
            "--state",
            state,
            "--limit",
            &limit.to_string(),
            "--json",
            "number,title,labels,body,state",
        ]))?;
        Ok(raw
            .into_iter()
            .map(|i| Issue {
                number: i.number,
                open: i.state.eq_ignore_ascii_case("open"),
                title: i.title,
                labels: i.labels.into_iter().map(|l| l.name).collect(),
                body: i.body,
            })
            .collect())
    }
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct RawIssue {
    number: u32,
    state: String,
    title: String,
    #[serde(default)]
    labels: Vec<Named>,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct Number {
    number: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    head_ref_name: String,
    head_ref_oid: String,
    #[serde(default)]
    head_repository: Option<Named>,
    #[serde(default)]
    head_repository_owner: Option<Login>,
    is_cross_repository: bool,
    state: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    closing_issues_references: Vec<Number>,
    #[serde(default)]
    mergeable: String,
    #[serde(default)]
    merge_state_status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMergeState {
    state: String,
    /// `null` unless auto-merge is armed; its fields aren't read.
    #[serde(default)]
    auto_merge_request: Option<serde_json::Value>,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    mergeable: String,
    #[serde(default)]
    merge_state_status: String,
}

#[derive(Deserialize)]
struct RawBranch {
    commit: RawCommit,
}

#[derive(Deserialize)]
struct RawCommit {
    sha: String,
}

#[derive(Deserialize)]
struct RawRuns {
    #[serde(default)]
    total_count: usize,
    #[serde(default)]
    workflow_runs: Vec<RawRun>,
}

#[derive(Deserialize)]
struct RawRun {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    run_attempt: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    actor: Option<Login>,
}

/// A run's status and conclusion as GitHub names them (issue 442): anything not completed is
/// still going; only a failure, a time-out or a run that couldn't start counts as failed.
fn run_status(status: Option<&str>, conclusion: Option<&str>) -> RunStatus {
    if status != Some("completed") {
        return RunStatus::Going;
    }
    match conclusion {
        Some("failure" | "timed_out" | "startup_failure") => RunStatus::Failed,
        _ => RunStatus::Passed,
    }
}

#[derive(Deserialize)]
struct RawJobs {
    #[serde(default)]
    jobs: Vec<RawJob>,
}

#[derive(Deserialize)]
struct RawJob {
    name: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMerged {
    state: String,
    #[serde(default)]
    merge_commit: Option<RawOid>,
}

#[derive(Deserialize)]
struct RawOid {
    oid: String,
}

/// A full commit id: 40 or 64 hex digits.
fn full_commit(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Decision 298: conflicting only; `BEHIND` still merges, so it is mergeable.
fn mergeable(mergeable: &str, merge_state_status: &str) -> Mergeable {
    if mergeable == "CONFLICTING" || merge_state_status == "DIRTY" {
        Mergeable::Conflicting
    } else if mergeable == "UNKNOWN" || mergeable.is_empty() {
        Mergeable::Unknown
    } else {
        Mergeable::Mergeable
    }
}

/// A PR state as `gh` names it.
fn pr_state(number: u32, state: &str) -> Result<PrState> {
    match state {
        "OPEN" => Ok(PrState::Open),
        "CLOSED" => Ok(PrState::Closed),
        "MERGED" => Ok(PrState::Merged),
        other => Err(anyhow!(
            "`gh pr view {number}` reported an unknown state {other}"
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawListedPr {
    number: u32,
    url: String,
    head_ref_name: String,
    base_ref_name: String,
    is_cross_repository: bool,
    state: String,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
struct RawComment {
    /// `null` for a deleted account.
    #[serde(default)]
    author: Option<Login>,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct RawComments {
    comments: Vec<RawComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepo {
    default_branch_ref: Named,
}

impl GitHubBackend for GhBackend {
    fn whoami(&self) -> Result<String> {
        let out = self.run(&strings(&["api", "user", "--jq", ".login"]), None)?;
        Ok(out.trim().to_owned())
    }

    fn default_branch(&self, repo: &str) -> Result<String> {
        let raw: RawRepo = self.json(&strings(&[
            "repo",
            "view",
            repo,
            "--json",
            "defaultBranchRef",
        ]))?;
        Ok(raw.default_branch_ref.name)
    }

    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>> {
        self.list_issues(repo, "open", 200)
    }

    fn issues_all(&self, repo: &str, limit: u32) -> Result<Vec<Issue>> {
        self.list_issues(repo, "all", limit)
    }

    fn issue_edit(&self, repo: &str, number: u32, body: &str) -> Result<()> {
        self.run(
            &strings(&[
                "issue",
                "edit",
                &number.to_string(),
                "--repo",
                repo,
                "--body-file",
                "-",
            ]),
            Some(body),
        )?;
        Ok(())
    }

    fn issue_labels(
        &self,
        repo: &str,
        number: u32,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        let mut args = strings(&["issue", "edit", &number.to_string(), "--repo", repo]);
        for label in add {
            args.extend(strings(&["--add-label", label]));
        }
        for label in remove {
            args.extend(strings(&["--remove-label", label]));
        }
        self.run(&args, None)?;
        Ok(())
    }

    fn issue(&self, repo: &str, number: u32) -> Result<IssueText> {
        let text = self.run(
            &strings(&["issue", "view", &number.to_string(), "--repo", repo]),
            None,
        )?;
        let header = |key: &str| {
            text.lines()
                .take_while(|line| *line != "--")
                .find_map(|line| line.strip_prefix(key))
                .map(|value| value.trim().to_owned())
        };
        let (Some(title), Some(state)) = (header("title:"), header("state:")) else {
            bail!("`gh issue view {number}` printed no title/state header");
        };
        Ok(IssueText {
            number,
            title,
            state,
            text,
        })
    }

    fn pr(&self, repo: &str, number: u32) -> Result<PrInfo> {
        let raw: RawPr = self.json(&strings(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "headRefName,headRefOid,headRepository,headRepositoryOwner,isCrossRepository,state,title,body,closingIssuesReferences,mergeable,mergeStateStatus",
        ]))?;
        let state = pr_state(number, &raw.state)?;
        Ok(PrInfo {
            number,
            head_ref: raw.head_ref_name,
            head_oid: raw.head_ref_oid,
            is_cross_repository: raw.is_cross_repository,
            head_repo: match (raw.head_repository_owner, raw.head_repository) {
                (Some(owner), Some(repo)) => format!("{}/{}", owner.login, repo.name),
                _ => String::new(),
            },
            state,
            title: raw.title,
            body: raw.body,
            closing_issues: raw
                .closing_issues_references
                .into_iter()
                .map(|n| n.number)
                .collect(),
            base_moved: raw.mergeable == "CONFLICTING"
                || matches!(raw.merge_state_status.as_str(), "BEHIND" | "DIRTY"),
        })
    }

    fn pr_merge_state(&self, repo: &str, number: u32) -> Result<PrMergeState> {
        let raw: RawMergeState = self.json(&strings(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "state,autoMergeRequest,isDraft,mergeable,mergeStateStatus",
        ]))?;
        Ok(PrMergeState {
            state: pr_state(number, &raw.state)?,
            auto_merge: raw.auto_merge_request.is_some_and(|r| !r.is_null()),
            draft: raw.is_draft,
            mergeable: mergeable(&raw.mergeable, &raw.merge_state_status),
        })
    }

    fn pr_disarm_auto_merge(&self, repo: &str, number: u32) -> Result<()> {
        self.run(
            &strings(&[
                "pr",
                "merge",
                &number.to_string(),
                "--repo",
                repo,
                "--disable-auto",
            ]),
            None,
        )?;
        Ok(())
    }

    fn branch_head(&self, repo: &str, branch: &str) -> Result<String> {
        // The branch goes into the API path: only a usable branch name is accepted.
        if !crate::task::repo::valid_ref_name(branch) {
            bail!("{branch:?} isn't a usable branch name, so its head can't be read");
        }
        let raw: RawBranch = self.json(&strings(&[
            "api",
            &format!("repos/{repo}/branches/{branch}"),
        ]))?;
        let sha = raw.commit.sha;
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("`gh api` named `{sha}` as the head of {branch}, which is not a full commit id");
        }
        Ok(sha)
    }

    fn pr_arm_auto_merge(&self, repo: &str, number: u32, head_commit: &str) -> Result<()> {
        // The commit goes to `--match-head-commit`, which is what keeps a push after the review
        // from being merged: only a full commit id is accepted.
        if !matches!(head_commit.len(), 40 | 64)
            || !head_commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("`{head_commit}` is not a full commit id, so auto-merge can't be armed on it");
        }
        self.run(
            &strings(&[
                "pr",
                "merge",
                &number.to_string(),
                "--repo",
                repo,
                "--auto",
                "--merge",
                "--match-head-commit",
                head_commit,
            ]),
            None,
        )?;
        Ok(())
    }

    fn open_pr_for(&self, repo: &str, head: &str, base: &str) -> Result<Option<ListedPr>> {
        // No `--base`: an open PR of the same head with another base is returned too, so the
        // caller can name it. `--head` matches the branch name only, a fork's included, so the
        // head's repository and the state are checked here as well.
        let raw: Vec<RawListedPr> = self.json(&strings(&[
            "pr",
            "list",
            "--repo",
            repo,
            "--head",
            head,
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,url,headRefName,baseRefName,isCrossRepository,state",
        ]))?;
        let mut candidates: Vec<ListedPr> = raw
            .into_iter()
            .filter(|p| p.state == "OPEN" && p.head_ref_name == head && !p.is_cross_repository)
            .map(|p| ListedPr {
                number: p.number,
                url: p.url,
                head_ref: p.head_ref_name,
                base_ref: p.base_ref_name,
                is_cross_repository: p.is_cross_repository,
            })
            .collect();
        let pick = candidates
            .iter()
            .position(|p| p.base_ref == base)
            .unwrap_or(0);
        Ok((!candidates.is_empty()).then(|| candidates.swap_remove(pick)))
    }

    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String> {
        let mut args = strings(&[
            "pr",
            "create",
            "--repo",
            repo,
            "--head",
            &request.head,
            "--base",
            &request.base,
            "--title",
            &request.title,
        ]);
        if request.draft {
            args.push("--draft".into());
        }
        args.extend(strings(&["--body-file", "-"]));
        let out = self.run(&args, Some(&request.body))?;
        Ok(out.trim().to_owned())
    }

    fn pr_comment(&self, repo: &str, number: u32, body: &str) -> Result<()> {
        self.run(
            &strings(&[
                "pr",
                "comment",
                &number.to_string(),
                "--repo",
                repo,
                "--body-file",
                "-",
            ]),
            Some(body),
        )?;
        Ok(())
    }

    fn pr_comments(&self, repo: &str, number: u32) -> Result<Vec<PrComment>> {
        let raw: RawComments = self.json(&strings(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "comments",
        ]))?;
        Ok(raw
            .comments
            .into_iter()
            .map(|c| PrComment {
                author: c.author.map(|a| a.login).unwrap_or_default(),
                body: c.body,
            })
            .collect())
    }

    fn issue_create(&self, repo: &str, request: &IssueRequest) -> Result<u32> {
        let mut args = strings(&["issue", "create", "--repo", repo, "--title", &request.title]);
        for label in &request.labels {
            args.push("--label".into());
            args.push(label.clone());
        }
        args.extend(strings(&["--body-file", "-"]));
        let out = self.run(&args, Some(&request.body))?;
        out.trim()
            .rsplit('/')
            .next()
            .and_then(|last| last.parse().ok())
            .ok_or_else(|| {
                anyhow!(
                    "could not read the issue number from `gh issue create` output: {}",
                    out.trim()
                )
            })
    }

    fn labels(&self, repo: &str) -> Result<Vec<String>> {
        let raw: Vec<Named> = self.json(&strings(&[
            "label", "list", "--repo", repo, "--json", "name", "--limit", "200",
        ]))?;
        Ok(raw.into_iter().map(|l| l.name).collect())
    }

    fn commit_status(&self, repo: &str, sha: &str, status: &CommitStatus) -> Result<()> {
        // The sha goes into the API path: only a full commit id is accepted.
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("`{sha}` is not a full commit id, so no status can be posted on it");
        }
        self.run(
            &strings(&[
                "api",
                &format!("repos/{repo}/statuses/{sha}"),
                "--method",
                "POST",
                "--silent",
                "-f",
                &format!("state={}", status.state.word()),
                "-f",
                &format!("context={}", status.context),
                "-f",
                &format!("description={}", status.description),
            ]),
            None,
        )?;
        Ok(())
    }

    fn commit_runs(&self, repo: &str, sha: &str) -> Result<Vec<WorkflowRun>> {
        // The sha goes into the API path: only a full commit id is accepted.
        if !full_commit(sha) {
            bail!("`{sha}` is not a full commit id, so its workflow runs can't be read");
        }
        // Every page (review M-4): a page that can't be read fails the whole read, and an empty
        // page ends it even if GitHub counted more (a run deleted meanwhile).
        let mut all = Vec::new();
        for page in 1u32.. {
            let raw: RawRuns = self.json(&strings(&[
                "api",
                &format!("repos/{repo}/actions/runs?head_sha={sha}&per_page=100&page={page}"),
            ]))?;
            let empty = raw.workflow_runs.is_empty();
            all.extend(raw.workflow_runs);
            if empty || all.len() >= raw.total_count {
                break;
            }
        }
        Ok(all
            .into_iter()
            .map(|r| WorkflowRun {
                id: r.id,
                workflow: r.name.unwrap_or_default(),
                attempt: r.run_attempt.unwrap_or(1),
                status: run_status(r.status.as_deref(), r.conclusion.as_deref()),
                url: r.html_url,
                actor: r.actor.map(|a| a.login).unwrap_or_default(),
            })
            .collect())
    }

    fn run_rerun_failed(&self, repo: &str, id: u64) -> Result<()> {
        self.run(
            &strings(&["run", "rerun", &id.to_string(), "--failed", "--repo", repo]),
            None,
        )?;
        Ok(())
    }

    fn run_failed_log(&self, repo: &str, id: u64) -> Result<Vec<JobLog>> {
        let id = id.to_string();
        let raw: RawJobs = self.json(&strings(&[
            "run", "view", &id, "--repo", repo, "--json", "jobs",
        ]))?;
        let mut jobs: Vec<JobLog> = raw
            .jobs
            .into_iter()
            .filter(|j| {
                run_status(j.status.as_deref(), j.conclusion.as_deref()) == RunStatus::Failed
            })
            .map(|j| JobLog {
                job: j.name,
                log: String::new(),
            })
            .collect();
        let log = self.run(
            &strings(&["run", "view", &id, "--repo", repo, "--log-failed"]),
            None,
        )?;
        // Each line starts with its job's name and a tab; a line without one goes with the line
        // before it.
        let mut last = None;
        for line in log.split_inclusive('\n') {
            let at = match line.split_once('\t') {
                Some((name, _)) => match jobs.iter().position(|j| j.job == name) {
                    Some(at) => at,
                    None => {
                        jobs.push(JobLog {
                            job: name.to_owned(),
                            log: String::new(),
                        });
                        jobs.len() - 1
                    }
                },
                None => match last {
                    Some(at) => at,
                    None => continue,
                },
            };
            last = Some(at);
            jobs[at].log.push_str(line);
            if !line.ends_with('\n') {
                jobs[at].log.push('\n');
            }
        }
        Ok(jobs)
    }

    fn pr_merge_commit(&self, repo: &str, number: u32) -> Result<Option<String>> {
        let raw: RawMerged = self.json(&strings(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            repo,
            "--json",
            "state,mergeCommit",
        ]))?;
        if pr_state(number, &raw.state)? != PrState::Merged {
            return Ok(None);
        }
        let Some(commit) = raw.merge_commit else {
            bail!("`gh pr view {number}` reported it merged without a merge commit");
        };
        if !full_commit(&commit.oid) {
            bail!(
                "`gh pr view {number}` named `{}` as its merge commit, which is not a full commit id",
                commit.oid
            );
        }
        Ok(Some(commit.oid))
    }
}
