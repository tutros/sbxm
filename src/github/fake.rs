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
    IssuesAll(String, u32),
    IssueEdit(String, u32, String),
}

#[derive(Default)]
pub struct FakeGitHub {
    user: String,
    default_branch: String,
    open_issues: Vec<Issue>,
    issue_texts: Vec<IssueText>,
    prs: Vec<PrInfo>,
    labels: Vec<String>,
    /// Every issue in any state: the scripted ones, then those created through the fake.
    all_issues: Mutex<Vec<Issue>>,
    next_issue: Mutex<u32>,
    creates: Mutex<usize>,
    edits: Mutex<usize>,
    fail_create_at: Option<usize>,
    fail_edit_at: Option<usize>,
    fail_issues_all: bool,
    failure: Option<String>,
    comment_failure: Option<String>,
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

    pub fn with_issues(self, issues: Vec<Issue>) -> Self {
        *self.all_issues.lock().unwrap() = issues;
        self
    }

    /// The nth `issue_create` (1-based) fails and creates nothing.
    pub fn failing_issue_create_at(mut self, n: usize) -> Self {
        self.fail_create_at = Some(n);
        self
    }

    /// The nth `issue_edit` (1-based) fails and changes nothing.
    pub fn failing_issue_edit_at(mut self, n: usize) -> Self {
        self.fail_edit_at = Some(n);
        self
    }

    pub fn failing_issues_all(mut self) -> Self {
        self.fail_issues_all = true;
        self
    }

    pub fn with_next_issue_number(self, number: u32) -> Self {
        *self.next_issue.lock().unwrap() = number;
        self
    }

    /// Only `pr_comment` fails (and is still recorded).
    pub fn failing_comments(mut self, message: &str) -> Self {
        self.comment_failure = Some(message.to_owned());
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
        self.record(GhCall::PrComment(repo.to_owned(), number, body.to_owned()))?;
        match &self.comment_failure {
            Some(message) => Err(anyhow!("{message}")),
            None => Ok(()),
        }
    }

    fn issue_create(&self, repo: &str, request: &IssueRequest) -> Result<u32> {
        self.record(GhCall::IssueCreate(repo.to_owned(), request.clone()))?;
        let attempt = {
            let mut creates = self.creates.lock().unwrap();
            *creates += 1;
            *creates
        };
        if self.fail_create_at == Some(attempt) {
            return Err(anyhow!("FakeGitHub: issue create {attempt} fails"));
        }
        let mut next = self.next_issue.lock().unwrap();
        let number = *next;
        *next += 1;
        self.all_issues.lock().unwrap().push(Issue {
            number,
            title: request.title.clone(),
            labels: request.labels.clone(),
            body: request.body.clone(),
        });
        Ok(number)
    }

    fn labels(&self, repo: &str) -> Result<Vec<String>> {
        self.record(GhCall::Labels(repo.to_owned()))?;
        Ok(self.labels.clone())
    }

    fn issues_all(&self, repo: &str, limit: u32) -> Result<Vec<Issue>> {
        self.record(GhCall::IssuesAll(repo.to_owned(), limit))?;
        if self.fail_issues_all {
            return Err(anyhow!("FakeGitHub: `gh issue list` fails"));
        }
        Ok(self
            .all_issues
            .lock()
            .unwrap()
            .iter()
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn issue_edit(&self, repo: &str, number: u32, body: &str) -> Result<()> {
        self.record(GhCall::IssueEdit(repo.to_owned(), number, body.to_owned()))?;
        let attempt = {
            let mut edits = self.edits.lock().unwrap();
            *edits += 1;
            *edits
        };
        if self.fail_edit_at == Some(attempt) {
            return Err(anyhow!("FakeGitHub: issue edit {attempt} fails"));
        }
        let mut all = self.all_issues.lock().unwrap();
        match all.iter_mut().find(|i| i.number == number) {
            Some(issue) => {
                issue.body = body.to_owned();
                Ok(())
            }
            None => Err(anyhow!("FakeGitHub: issue {number} does not exist")),
        }
    }
}
