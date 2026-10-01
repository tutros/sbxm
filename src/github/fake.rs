//! `FakeGitHub`: records calls, answers from what a test scripted.

use std::sync::Mutex;

use anyhow::{Result, anyhow};

use super::{GitHubBackend, Issue, IssueRequest, IssueText, PrInfo, PrRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Whoami,
    DefaultBranch(String),
    IssuesOpen(String),
    Issue(String, u32),
    Pr(String, u32),
    PrCreate(String, PrRequest),
    PrComment(String, u32, String),
    IssueCreate(String, IssueRequest),
    Labels(String),
}

#[derive(Default)]
pub struct FakeGitHub {
    user: String,
    default_branch: String,
    open_issues: Vec<Issue>,
    issue_texts: Vec<IssueText>,
    prs: Vec<PrInfo>,
    labels: Vec<String>,
    next_issue: Mutex<u32>,
    failure: Option<String>,
    calls: Mutex<Vec<GhCall>>,
}

impl FakeGitHub {
    pub fn with_user(mut self, user: &str) -> Self {
        self.user = user.to_owned();
        self
    }

    pub fn with_default_branch(mut self, branch: &str) -> Self {
        self.default_branch = branch.to_owned();
        self
    }

    pub fn with_open_issues(mut self, issues: Vec<Issue>) -> Self {
        self.open_issues = issues;
        self
    }

    pub fn with_issue_text(mut self, text: IssueText) -> Self {
        self.issue_texts.push(text);
        self
    }

    pub fn with_pr(mut self, pr: PrInfo) -> Self {
        self.prs.push(pr);
        self
    }

    pub fn with_labels(mut self, labels: &[&str]) -> Self {
        self.labels = labels.iter().map(|l| (*l).to_owned()).collect();
        self
    }

    pub fn with_next_issue_number(self, number: u32) -> Self {
        *self.next_issue.lock().unwrap() = number;
        self
    }

    /// Every call fails with `message` (and is still recorded).
    pub fn failing(mut self, message: &str) -> Self {
        self.failure = Some(message.to_owned());
        self
    }

    pub fn calls(&self) -> Vec<GhCall> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: GhCall) -> Result<()> {
        self.calls.lock().unwrap().push(call);
        match &self.failure {
            Some(message) => Err(anyhow!("{message}")),
            None => Ok(()),
        }
    }
}

impl GitHubBackend for FakeGitHub {
    fn whoami(&self) -> Result<String> {
        self.record(GhCall::Whoami)?;
        Ok(self.user.clone())
    }

    fn default_branch(&self, repo: &str) -> Result<String> {
        self.record(GhCall::DefaultBranch(repo.to_owned()))?;
        Ok(self.default_branch.clone())
    }

    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>> {
        self.record(GhCall::IssuesOpen(repo.to_owned()))?;
        Ok(self.open_issues.clone())
    }

    fn issue(&self, repo: &str, number: u32) -> Result<IssueText> {
        self.record(GhCall::Issue(repo.to_owned(), number))?;
        self.issue_texts
            .iter()
            .find(|t| t.number == number)
            .cloned()
            .ok_or_else(|| anyhow!("FakeGitHub: issue {number} is not scripted"))
    }

    fn pr(&self, repo: &str, number: u32) -> Result<PrInfo> {
        self.record(GhCall::Pr(repo.to_owned(), number))?;
        self.prs
            .iter()
            .find(|p| p.number == number)
            .cloned()
            .ok_or_else(|| anyhow!("FakeGitHub: pr {number} is not scripted"))
    }

    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String> {
        self.record(GhCall::PrCreate(repo.to_owned(), request.clone()))?;
        Ok(format!(
            "https://github.com/{repo}/pull/{}",
            request.head.len() + 1
        ))
    }

    fn pr_comment(&self, repo: &str, number: u32, body: &str) -> Result<()> {
        self.record(GhCall::PrComment(repo.to_owned(), number, body.to_owned()))
    }

    fn issue_create(&self, repo: &str, request: &IssueRequest) -> Result<u32> {
        self.record(GhCall::IssueCreate(repo.to_owned(), request.clone()))?;
        let mut next = self.next_issue.lock().unwrap();
        let number = *next;
        *next += 1;
        Ok(number)
    }

    fn labels(&self, repo: &str) -> Result<Vec<String>> {
        self.record(GhCall::Labels(repo.to_owned()))?;
        Ok(self.labels.clone())
    }
}
