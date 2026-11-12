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
/// state,autoMergeRequest,isDraft,mergeable,mergeStateStatus`): what `task loop` needs to count
/// the PRs awaiting the user (issue 320, decision 3) and to resolve the conflicting ones (issue
/// 432, decision 298).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrMergeState {
    pub state: PrState,
    pub auto_merge: bool,
    pub draft: bool,
    pub mergeable: Mergeable,
}

/// Whether GitHub can merge a PR into its base (decision 298): only a conflict needs the base
/// merged; a PR that is only behind still merges, so it counts as mergeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mergeable {
    #[default]
    Mergeable,
    /// `mergeable` is `CONFLICTING` or `mergeStateStatus` is `DIRTY`.
    Conflicting,
    /// GitHub is still computing it (`mergeable` is `UNKNOWN`), as for a while after a merge.
    Unknown,
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

/// One file of a pull request's diff, as `gh api repos/<repo>/pulls/<n>/files` lists it (issue
/// 549).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFile {
    /// The file's path in the PR's head.
    pub filename: String,
    /// The path it had before, for a renamed file.
    pub previous_filename: Option<String>,
}

/// A pull request's file list, every page of it, and the number of changed files GitHub reports
/// for the PR (`changed_files`): a list shorter than that is incomplete (issue 549).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrFiles {
    pub changed_files: u32,
    pub files: Vec<PrFile>,
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

/// A status posted on a commit, with when it was posted (`gh api
/// repos/<repo>/commits/<sha>/statuses`): the veto window of a medium-risk PR starts at its
/// head's latest `sbxm/review` status (issue 548).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedStatus {
    pub context: String,
    /// As GitHub names it: `success`, `failure`, `error` or `pending`.
    pub state: String,
    /// Seconds since the epoch (`created_at`).
    pub at: u64,
}

/// A label added to or taken off an issue or PR, as its events list it (`gh api
/// repos/<repo>/issues/<n>/events`): the medium-risk trial counts a PR's `hold` labellings and
/// reads when it was labelled `go` (issue 550).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelEvent {
    pub label: String,
    /// `true` when it was added, `false` when it was taken off.
    pub added: bool,
    /// Seconds since the epoch (`created_at`).
    pub at: u64,
}

/// Where a workflow run stands (issue 442, decision 299).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Queued, waiting or in progress.
    Going,
    /// Completed with any conclusion but a failure (success, skipped, neutral, cancelled...).
    Passed,
    /// Completed as `failure`, `timed_out` or `startup_failure`.
    Failed,
}

/// A GitHub Actions workflow run of a commit, at its latest attempt (`gh api
/// repos/<repo>/actions/runs?head_sha=<sha>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: u64,
    /// The workflow's name.
    pub workflow: String,
    /// GitHub's attempt number: 1 for the first run, 2 after one re-run.
    pub attempt: u32,
    pub status: RunStatus,
    pub url: String,
    /// The login of who triggered the run's first attempt (for a push to `main`, who merged);
    /// empty when GitHub didn't say.
    pub actor: String,
}

/// One failed job of a workflow run and its log, as `gh run view <id> --log-failed` prints it
/// (issue 448).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLog {
    /// The job's name.
    pub job: String,
    /// Its log lines, each ending in a newline; empty when the job printed none.
    pub log: String,
}

pub trait GitHubBackend: Send + Sync {
    fn whoami(&self) -> Result<String>;
    fn default_branch(&self, repo: &str) -> Result<String>;
    fn issues_open(&self, repo: &str) -> Result<Vec<Issue>>;
    /// Every open issue labeled `label`, the label being the listing's own filter, so none is
    /// missed behind newer issues without it; an error rather than a partial list when there may
    /// be more than the listing's limit (review M-1 of issue 463).
    fn issues_open_labeled(&self, repo: &str, label: &str) -> Result<Vec<Issue>>;
    /// Every issue labeled `label` in any state, open or closed, the label being the listing's own
    /// filter as in [`Self::issues_open_labeled`], so the repository's other issues never cut it
    /// short; an error when there may be more than the listing's limit (issue 594, review M-4).
    fn issues_labeled(&self, repo: &str, label: &str) -> Result<Vec<Issue>>;
    fn issue(&self, repo: &str, number: u32) -> Result<IssueText>;
    fn pr(&self, repo: &str, number: u32) -> Result<PrInfo>;
    fn pr_merge_state(&self, repo: &str, number: u32) -> Result<PrMergeState>;
    /// Arms GitHub's auto-merge on PR `number` (`gh pr merge --auto --merge --match-head-commit`):
    /// it merges once the required checks pass, and only if the PR's head is still
    /// `head_commit`, a full commit id (decisions 266, 280, 293).
    fn pr_arm_auto_merge(&self, repo: &str, number: u32, head_commit: &str) -> Result<()>;
    /// Disarms GitHub's auto-merge on PR `number` (`gh pr merge --disable-auto`), before `task
    /// loop` pushes a resolve onto it (decision 298).
    fn pr_disarm_auto_merge(&self, repo: &str, number: u32) -> Result<()>;
    /// The commit branch `branch` of `repo` is at, a full commit id (`gh api
    /// repos/<repo>/branches/<branch>`).
    fn branch_head(&self, repo: &str, branch: &str) -> Result<String>;
    /// Whether `repo` has a branch named exactly `branch` (`gh api
    /// repos/<repo>/git/matching-refs/heads/<branch>`), read before a task of that name would
    /// push it (issue 511).
    fn branch_exists(&self, repo: &str, branch: &str) -> Result<bool>;
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
    /// Posts `body` as a comment on issue `number` and returns the comment's id, which
    /// [`issue_comment_edit`](Self::issue_comment_edit) takes (issue 430).
    fn issue_comment(&self, repo: &str, number: u32, body: &str) -> Result<u64>;
    /// Replaces the body of the issue comment `id` of `repo` (issue 430).
    fn issue_comment_edit(&self, repo: &str, id: u64, body: &str) -> Result<()>;
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
    /// The state of the latest status of `context` on commit `sha` (a full commit id), as GitHub
    /// reports it (`success`, `failure`, `error` or `pending`); `None` when it has none (issue
    /// 547).
    fn commit_status_state(&self, repo: &str, sha: &str, context: &str) -> Result<Option<String>>;
    /// Every workflow run of commit `sha` (a full commit id), each at its latest attempt (issue
    /// 442).
    fn commit_runs(&self, repo: &str, sha: &str) -> Result<Vec<WorkflowRun>>;
    /// Re-runs the failed jobs of run `id` (`gh run rerun <id> --failed`), which makes its next
    /// attempt (issue 442).
    fn run_rerun_failed(&self, repo: &str, id: u64) -> Result<()>;
    /// The failed jobs of run `id` at its latest attempt, in the run's order, each with its lines
    /// of `gh run view <id> --log-failed`; a failed job that printed none has an empty log (issue
    /// 448).
    fn run_failed_log(&self, repo: &str, id: u64) -> Result<Vec<JobLog>>;
    /// The merge commit of PR `number` once GitHub merged it, a full commit id; `None` while it
    /// isn't merged (issue 442).
    fn pr_merge_commit(&self, repo: &str, number: u32) -> Result<Option<String>>;
    /// PR `number`'s files, every page (`gh api repos/<repo>/pulls/<n>/files --paginate`), and
    /// its `changed_files` count (`gh api repos/<repo>/pulls/<n>`), read before a medium-risk PR
    /// would be armed (issue 549).
    fn pr_files(&self, repo: &str, number: u32) -> Result<PrFiles>;
    /// The labels on PR `number`, every page (`gh api repos/<repo>/issues/<n>/labels
    /// --paginate`), read for its `hold` and `go` labels (issue 548).
    fn pr_labels(&self, repo: &str, number: u32) -> Result<Vec<String>>;
    /// Removes the label named exactly `label` from PR `number` (`gh api --method DELETE
    /// repos/<repo>/issues/<n>/labels/<label>`), as the loop does to `go` before it pushes onto
    /// the PR (issue 548).
    fn pr_remove_label(&self, repo: &str, number: u32, label: &str) -> Result<()>;
    /// Every status posted on commit `sha` (a full commit id), every page, newest first as GitHub
    /// lists them (issue 548).
    fn commit_statuses(&self, repo: &str, sha: &str) -> Result<Vec<PostedStatus>>;
    /// The body of every comment on issue `number`, every page, oldest first (`gh api
    /// repos/<repo>/issues/<n>/comments --paginate`): the medium-risk trial counts its merges
    /// there (issue 550).
    fn issue_comment_bodies(&self, repo: &str, number: u32) -> Result<Vec<String>>;
    /// Every label added to or taken off issue or PR `number`, every page, oldest first (`gh api
    /// repos/<repo>/issues/<n>/events --paginate`), other events left out (issue 550).
    fn label_events(&self, repo: &str, number: u32) -> Result<Vec<LabelEvent>>;
    /// When PR `number` merged, in seconds since the epoch; `None` while it isn't merged (`gh api
    /// repos/<repo>/pulls/<n>`, issue 550).
    fn pr_merged_at(&self, repo: &str, number: u32) -> Result<Option<u64>>;
}
