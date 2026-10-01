//! `GitHubBackend`: everything sbxm asks of GitHub, behind a trait so tests use `FakeGitHub`
//! and the real one shells out to `gh` (M2b spec §9, decision 142).

pub mod fake;
pub mod gh;

use anyhow::Result;

/// An open issue as `gh issue list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u32,
    pub title: String,
    pub labels: Vec<String>,
    pub body: String,
}

/// `gh issue view` output kept verbatim (it is the worker's prompt material), with the
/// title and state read from its header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueText {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u32,
    pub head_ref: String,
    pub is_cross_repository: bool,
    pub state: PrState,
    pub title: String,
    pub body: String,
    pub closing_issues: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRequest {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRequest {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

pub trait GitHubBackend: Send + Sync {
    fn whoami(&self) -> Result<String>;
    fn default_branch(&self, repo: &str) -> Result<String>;
    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>>;
    fn issue(&self, repo: &str, number: u32) -> Result<IssueText>;
    fn pr(&self, repo: &str, number: u32) -> Result<PrInfo>;
    /// Returns the new PR's URL.
    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String>;
    fn pr_comment(&self, repo: &str, number: u32, body: &str) -> Result<()>;
    /// Returns the new issue's number.
    fn issue_create(&self, repo: &str, request: &IssueRequest) -> Result<u32>;
    fn labels(&self, repo: &str) -> Result<Vec<String>>;
}
