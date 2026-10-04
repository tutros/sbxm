//! `GhBackend`: `GitHubBackend` over the `gh` CLI. Bodies go through stdin (`--body-file -`),
//! never the command line.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use super::{GitHubBackend, Issue, IssueRequest, IssueText, PrInfo, PrRequest, PrState};

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
    is_cross_repository: bool,
    state: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    closing_issues_references: Vec<Number>,
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
            "headRefName,isCrossRepository,state,title,body,closingIssuesReferences",
        ]))?;
        let state = match raw.state.as_str() {
            "OPEN" => PrState::Open,
            "CLOSED" => PrState::Closed,
            "MERGED" => PrState::Merged,
            other => {
                return Err(anyhow!(
                    "`gh pr view {number}` reported an unknown state {other}"
                ));
            }
        };
        Ok(PrInfo {
            number,
            head_ref: raw.head_ref_name,
            is_cross_repository: raw.is_cross_repository,
            state,
            title: raw.title,
            body: raw.body,
            closing_issues: raw
                .closing_issues_references
                .into_iter()
                .map(|n| n.number)
                .collect(),
        })
    }

    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String> {
        let out = self.run(
            &strings(&[
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
                "--body-file",
                "-",
            ]),
            Some(&request.body),
        )?;
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
}
