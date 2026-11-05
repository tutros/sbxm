//! `GitHubBackend`: everything sbxm asks of GitHub, behind a trait so tests use `FakeGitHub`
//! and the real one shells out to `gh` (M2b spec §9, decision 142).

pub mod fake;
pub mod gh;

use anyhow::Result;

/// An issue as `gh issue list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u32,
    /// `false` for a closed issue; only `issues_all` lists those.
    pub open: bool,
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
    /// The commit the PR's head branch is at (`headRefOid`).
    pub head_oid: String,
    pub is_cross_repository: bool,
    /// `owner/name` of the repository the head branch lives in (`headRepositoryOwner` and
    /// `headRepository`); empty when `gh` didn't say.
    pub head_repo: String,
    pub state: PrState,
    pub title: String,
    pub body: String,
    pub closing_issues: Vec<u32>,
    /// GitHub reports the PR conflicting with its base or behind it (`mergeable` is
    /// `CONFLICTING`, or `mergeStateStatus` is `BEHIND` or `DIRTY`): it needs the base merged.
    pub base_moved: bool,
}

/// A PR's state and whether auto-merge is armed on it (`gh pr view --json
/// state,autoMergeRequest`): what `task loop` needs to count the PRs awaiting the user (issue 320,
/// decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrMergeState {
    pub state: PrState,
    pub auto_merge: bool,
}

/// An open pull request as `gh pr list` lists it, for finding the PR a branch already has
/// (issue 60).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedPr {
    pub number: u32,
    pub url: String,
    pub head_ref: String,
    pub base_ref: String,
    /// `true` when the head branch lives in a fork, not in the repository the PR is listed in.
    pub is_cross_repository: bool,
}

/// A comment on a pull request's conversation, as `gh pr view --json comments` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    /// The author's login.
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRequest {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    /// Open it as a draft (`gh pr create --draft`), for a task with something left unresolved
    /// (decision 173(b)).
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRequest {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// A commit status's state (`gh api repos/<repo>/statuses/<sha>`); sbxm only posts these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
    Success,
    Failure,
}

impl StatusState {
    pub fn word(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// A commit status, as GitHub shows it next to a commit and checks it against a ruleset's
/// required checks (issue 317).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStatus {
    pub state: StatusState,
    pub context: String,
    pub description: String,
}

pub trait GitHubBackend: Send + Sync {
    fn whoami(&self) -> Result<String>;
    fn default_branch(&self, repo: &str) -> Result<String>;
    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>>;
    fn issue(&self, repo: &str, number: u32) -> Result<IssueText>;
    fn pr(&self, repo: &str, number: u32) -> Result<PrInfo>;
    fn pr_merge_state(&self, repo: &str, number: u32) -> Result<PrMergeState>;
    /// The open PR of `repo` whose head is the branch `head` of `repo` itself (never a fork's
    /// branch of the same name), with base `base` if there is one, else one with another base,
    /// which the caller reports instead of adopting (issue 60). Closed and merged PRs are never
    /// returned.
    fn open_pr_for(&self, repo: &str, head: &str, base: &str) -> Result<Option<ListedPr>>;
    /// Returns the new PR's URL.
    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String>;
    fn pr_comment(&self, repo: &str, number: u32, body: &str) -> Result<()>;
    /// Every comment on PR `number`'s conversation, oldest first.
    fn pr_comments(&self, repo: &str, number: u32) -> Result<Vec<PrComment>>;
    /// Returns the new issue's number.
    fn issue_create(&self, repo: &str, request: &IssueRequest) -> Result<u32>;
    fn labels(&self, repo: &str) -> Result<Vec<String>>;
    /// Every issue in any state, up to `limit`, newest first (`gh issue list --state all`).
    fn issues_all(&self, repo: &str, limit: u32) -> Result<Vec<Issue>>;
    /// Replaces an issue's body.
    fn issue_edit(&self, repo: &str, number: u32, body: &str) -> Result<()>;
    /// Adds and removes labels on an issue, leaving its other labels alone.
    fn issue_labels(
        &self,
        repo: &str,
        number: u32,
        add: &[String],
        remove: &[String],
    ) -> Result<()>;
    /// Posts `status` on commit `sha` (a full commit id).
    fn commit_status(&self, repo: &str, sha: &str, status: &CommitStatus) -> Result<()>;
}
