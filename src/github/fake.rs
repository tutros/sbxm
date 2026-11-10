//! `FakeGitHub`: records calls, answers from what a test scripted.

use std::sync::Mutex;

use anyhow::{Result, anyhow};

use super::{
    CommitStatus, GitHubBackend, Issue, IssueRequest, IssueText, JobLog, ListedPr, Mergeable,
    PostedStatus, PrComment, PrFiles, PrInfo, PrMergeState, PrRequest, PrState, RunStatus,
    WorkflowRun,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Whoami,
    DefaultBranch(String),
    IssuesOpen(String),
    /// Repo, label.
    IssuesOpenLabeled(String, String),
    Issue(String, u32),
    Pr(String, u32),
    PrMergeState(String, u32),
    /// Repo, number, head commit.
    PrArmAutoMerge(String, u32, String),
    PrDisarmAutoMerge(String, u32),
    /// Repo, branch.
    BranchHead(String, String),
    /// Repo, branch.
    BranchExists(String, String),
    /// Repo, head, base.
    OpenPrFor(String, String, String),
    PrCreate(String, PrRequest),
    PrComment(String, u32, String),
    PrComments(String, u32),
    /// Repo, issue, body.
    IssueComment(String, u32, String),
    /// Repo, comment id, body.
    IssueCommentEdit(String, u64, String),
    IssueCreate(String, IssueRequest),
    Labels(String),
    IssuesAll(String, u32),
    IssueEdit(String, u32, String),
    /// Repo, number, labels added, labels removed.
    IssueLabels(String, u32, Vec<String>, Vec<String>),
    /// Repo, commit, status.
    CommitStatus(String, String, CommitStatus),
    /// Repo, commit, context.
    CommitStatusState(String, String, String),
    /// Repo, commit.
    CommitRuns(String, String),
    /// Repo, run id.
    RunRerunFailed(String, u64),
    /// Repo, run id.
    RunFailedLog(String, u64),
    PrMergeCommit(String, u32),
    PrFiles(String, u32),
    PrLabels(String, u32),
    /// Repo, number, label.
    PrRemoveLabel(String, u32, String),
    /// Repo, commit.
    CommitStatuses(String, String),
}

#[derive(Default)]
pub struct FakeGitHub {
    user: String,
    default_branch: String,
    open_issues: Mutex<Vec<Issue>>,
    issue_texts: Vec<IssueText>,
    prs: Mutex<Vec<PrInfo>>,
    /// The scripted PRs with auto-merge armed.
    auto_merge: Mutex<Vec<u32>>,
    /// The scripted draft PRs.
    drafts: Vec<u32>,
    /// What `pr_merge_state` says of a PR's mergeability; mergeable when not scripted.
    mergeable: Mutex<Vec<(u32, Mergeable)>>,
    /// What `branch_head` answers, by branch.
    branch_heads: Mutex<Vec<(String, String)>>,
    disarm_failure: Option<String>,
    /// What `open_pr_for` searches: repo, PR, state. `pr_create` adds the PRs it opens.
    listed: Mutex<Vec<(String, ListedPr, PrState)>>,
    lookups: Mutex<usize>,
    fail_lookup_from: Option<usize>,
    create_failure: Option<(String, bool)>,
    pr_comments: Vec<(u32, Vec<PrComment>)>,
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
    /// The id the next `issue_comment` returns; 1 when not scripted.
    next_comment_id: Mutex<u64>,
    /// The issues whose `issue_comment` fails.
    failing_issue_comments: Vec<u32>,
    failing_comment_edits: bool,
    status_failure: Option<String>,
    /// The statuses `commit_status` posted, by commit, oldest first: what `commit_status_state`
    /// reads.
    statuses: Mutex<Vec<(String, CommitStatus)>>,
    arm_failure: Option<String>,
    /// What `commit_runs` answers, by commit; no runs for a commit not scripted.
    runs: Mutex<Vec<(String, Vec<WorkflowRun>)>>,
    /// What `run_failed_log` answers, by run; no failed jobs for a run not scripted.
    run_logs: Mutex<Vec<(u64, Vec<JobLog>)>>,
    /// What `pr_merge_commit` answers, by PR; not merged when not scripted.
    merge_commits: Mutex<Vec<(u32, String)>>,
    /// What `pr_files` answers, by PR; an error for a PR not scripted.
    pr_files: Vec<(u32, PrFiles)>,
    /// What `pr_labels` answers, by PR; none for a PR not scripted. `pr_remove_label` takes
    /// from it.
    pr_labels: Mutex<Vec<(u32, Vec<String>)>>,
    pr_labels_failure: Option<String>,
    remove_label_failure: Option<String>,
    /// What `commit_statuses` answers, by commit; none for a commit not scripted.
    posted_statuses: Vec<(String, Vec<PostedStatus>)>,
    statuses_failure: Option<String>,
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

    pub fn with_open_issues(self, issues: Vec<Issue>) -> Self {
        Self {
            open_issues: Mutex::new(issues),
            ..self
        }
    }

    /// Changes what `issues_open` returns from now on, e.g. to model an issue closing or becoming
    /// blocked while a `--restart` confirmation is pending (issue #207): a test's
    /// `Confirm::confirm` can call this to change the answer `pipeline::choose_issues`' later call
    /// gives, after `check_restartable`'s own call already ran.
    pub fn set_open_issues(&self, issues: Vec<Issue>) {
        *self.open_issues.lock().unwrap() = issues;
    }

    pub fn with_issue_text(mut self, text: IssueText) -> Self {
        self.issue_texts.push(text);
        self
    }

    pub fn with_pr_files(mut self, number: u32, files: PrFiles) -> Self {
        self.pr_files.push((number, files));
        self
    }

    /// The labels PR `number` has (issue 548).
    pub fn with_pr_labels(self, number: u32, labels: &[&str]) -> Self {
        self.set_pr_labels(number, labels);
        self
    }

    /// The labels PR `number` has from now on, as the user labelling it does.
    pub fn set_pr_labels(&self, number: u32, labels: &[&str]) {
        let mut all = self.pr_labels.lock().unwrap();
        all.retain(|(n, _)| *n != number);
        all.push((number, labels.iter().map(|l| (*l).to_owned()).collect()));
    }

    /// The labels PR `number` has now.
    pub fn pr_labels_now(&self, number: u32) -> Vec<String> {
        self.pr_labels
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default()
    }

    /// Only `pr_labels` fails (and is still recorded).
    pub fn failing_pr_labels(mut self, message: &str) -> Self {
        self.pr_labels_failure = Some(message.to_owned());
        self
    }

    /// Only `pr_remove_label` fails (and is still recorded).
    pub fn failing_label_removal(mut self, message: &str) -> Self {
        self.remove_label_failure = Some(message.to_owned());
        self
    }

    /// The statuses posted on commit `sha`, newest first (issue 548).
    pub fn with_statuses(mut self, sha: &str, statuses: Vec<PostedStatus>) -> Self {
        self.posted_statuses.push((sha.to_owned(), statuses));
        self
    }

    /// Only `commit_statuses` fails (and is still recorded).
    pub fn failing_commit_statuses(mut self, message: &str) -> Self {
        self.statuses_failure = Some(message.to_owned());
        self
    }

    pub fn with_pr(self, pr: PrInfo) -> Self {
        self.prs.lock().unwrap().push(pr);
        self
    }

    /// Moves the scripted PR `number`'s head to `head`, as a push onto its branch does.
    pub fn set_pr_head(&self, number: u32, head: &str) {
        for pr in self.prs.lock().unwrap().iter_mut() {
            if pr.number == number {
                pr.head_oid = head.to_owned();
            }
        }
    }

    /// Marks the scripted PR `number` a draft.
    pub fn with_draft(mut self, number: u32) -> Self {
        self.drafts.push(number);
        self
    }

    /// What `pr_merge_state` says of PR `number`'s mergeability from now on.
    pub fn set_mergeable(&self, number: u32, mergeable: Mergeable) {
        let mut all = self.mergeable.lock().unwrap();
        all.retain(|(n, _)| *n != number);
        all.push((number, mergeable));
    }

    pub fn with_mergeable(self, number: u32, mergeable: Mergeable) -> Self {
        self.set_mergeable(number, mergeable);
        self
    }

    /// What `branch_head` answers for `branch` from now on.
    pub fn set_branch_head(&self, branch: &str, head: &str) {
        let mut all = self.branch_heads.lock().unwrap();
        all.retain(|(b, _)| b != branch);
        all.push((branch.to_owned(), head.to_owned()));
    }

    pub fn with_branch_head(self, branch: &str, head: &str) -> Self {
        self.set_branch_head(branch, head);
        self
    }

    /// Only `pr_disarm_auto_merge` fails (and is still recorded).
    pub fn failing_disarm(mut self, message: &str) -> Self {
        self.disarm_failure = Some(message.to_owned());
        self
    }

    /// Arms auto-merge on the scripted PR `number`, as `pr_merge_state` reports it.
    pub fn with_auto_merge(self, number: u32) -> Self {
        self.auto_merge.lock().unwrap().push(number);
        self
    }

    /// A PR of `repo` that `open_pr_for` finds while `state` is open.
    pub fn with_listed_pr(self, repo: &str, pr: ListedPr, state: PrState) -> Self {
        self.listed
            .lock()
            .unwrap()
            .push((repo.to_owned(), pr, state));
        self
    }

    /// `pr_create` fails with `message` (and is still recorded); with `opened`, GitHub opened the
    /// PR all the same, as when only its answer is lost, so `open_pr_for` finds it afterwards.
    pub fn failing_pr_create(mut self, message: &str, opened: bool) -> Self {
        self.create_failure = Some((message.to_owned(), opened));
        self
    }

    /// The nth `open_pr_for` (1-based) and every later one fail.
    pub fn failing_pr_lookup_from(mut self, n: usize) -> Self {
        self.fail_lookup_from = Some(n);
        self
    }

    /// What `pr_comments` returns for PR `number` (no comments for a PR not scripted).
    pub fn with_pr_comments(mut self, number: u32, comments: Vec<PrComment>) -> Self {
        self.pr_comments.push((number, comments));
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

    /// The id the next `issue_comment` returns; each one after it is one higher.
    pub fn with_next_comment_id(self, id: u64) -> Self {
        *self.next_comment_id.lock().unwrap() = id;
        self
    }

    /// `issue_comment` on issue `number` fails (and is still recorded).
    pub fn failing_issue_comments_on(mut self, number: u32) -> Self {
        self.failing_issue_comments.push(number);
        self
    }

    /// Every `issue_comment_edit` fails (and is still recorded).
    pub fn failing_issue_comment_edits(mut self) -> Self {
        self.failing_comment_edits = true;
        self
    }

    /// Only `commit_status` fails (and is still recorded).
    pub fn failing_statuses(mut self, message: &str) -> Self {
        self.status_failure = Some(message.to_owned());
        self
    }

    /// Only `pr_arm_auto_merge` fails (and is still recorded).
    pub fn failing_arm(mut self, message: &str) -> Self {
        self.arm_failure = Some(message.to_owned());
        self
    }

    /// What `commit_runs` answers for `sha` from now on.
    pub fn set_runs(&self, sha: &str, runs: Vec<WorkflowRun>) {
        let mut all = self.runs.lock().unwrap();
        all.retain(|(s, _)| s != sha);
        all.push((sha.to_owned(), runs));
    }

    pub fn with_runs(self, sha: &str, runs: Vec<WorkflowRun>) -> Self {
        self.set_runs(sha, runs);
        self
    }

    /// What `run_failed_log` answers for run `id`.
    pub fn with_run_log(self, id: u64, jobs: Vec<JobLog>) -> Self {
        self.run_logs.lock().unwrap().push((id, jobs));
        self
    }

    /// Merges the scripted PR `number` into `sha`: `pr_merge_commit` names it, and `pr` and
    /// `pr_merge_state` say merged.
    pub fn set_merged(&self, number: u32, sha: &str) {
        let mut all = self.merge_commits.lock().unwrap();
        all.retain(|(n, _)| *n != number);
        all.push((number, sha.to_owned()));
        for pr in self.prs.lock().unwrap().iter_mut() {
            if pr.number == number {
                pr.state = PrState::Merged;
            }
        }
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
        Ok(self.open_issues.lock().unwrap().clone())
    }

    /// The scripted open issues and the open ones created through the fake, with the label.
    fn issues_open_labeled(&self, repo: &str, label: &str) -> Result<Vec<Issue>> {
        self.record(GhCall::IssuesOpenLabeled(repo.to_owned(), label.to_owned()))?;
        let mut found: Vec<Issue> = Vec::new();
        let open = self.open_issues.lock().unwrap().clone();
        let created = self.all_issues.lock().unwrap().clone();
        for issue in open
            .into_iter()
            .chain(created.into_iter().filter(|i| i.open))
        {
            if issue.labels.iter().any(|l| l == label)
                && !found.iter().any(|f| f.number == issue.number)
            {
                found.push(issue);
            }
        }
        Ok(found)
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
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.number == number)
            .cloned()
            .ok_or_else(|| anyhow!("FakeGitHub: pr {number} is not scripted"))
    }

    fn pr_merge_state(&self, repo: &str, number: u32) -> Result<PrMergeState> {
        self.record(GhCall::PrMergeState(repo.to_owned(), number))?;
        let state = self
            .prs
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.number == number)
            .map(|p| p.state)
            .ok_or_else(|| anyhow!("FakeGitHub: pr {number} is not scripted"))?;
        Ok(PrMergeState {
            state,
            auto_merge: self.auto_merge.lock().unwrap().contains(&number),
            draft: self.drafts.contains(&number),
            mergeable: self
                .mergeable
                .lock()
                .unwrap()
                .iter()
                .find(|(n, _)| *n == number)
                .map_or(Mergeable::Mergeable, |(_, m)| *m),
        })
    }

    fn pr_disarm_auto_merge(&self, repo: &str, number: u32) -> Result<()> {
        self.record(GhCall::PrDisarmAutoMerge(repo.to_owned(), number))?;
        if let Some(message) = &self.disarm_failure {
            return Err(anyhow!("{message}"));
        }
        self.auto_merge.lock().unwrap().retain(|n| *n != number);
        Ok(())
    }

    fn branch_head(&self, repo: &str, branch: &str) -> Result<String> {
        self.record(GhCall::BranchHead(repo.to_owned(), branch.to_owned()))?;
        self.branch_heads
            .lock()
            .unwrap()
            .iter()
            .find(|(b, _)| b == branch)
            .map(|(_, head)| head.clone())
            .ok_or_else(|| anyhow!("FakeGitHub: branch {branch} is not scripted"))
    }

    /// The branches `branch_head` answers for are the ones that exist.
    fn branch_exists(&self, repo: &str, branch: &str) -> Result<bool> {
        self.record(GhCall::BranchExists(repo.to_owned(), branch.to_owned()))?;
        Ok(self
            .branch_heads
            .lock()
            .unwrap()
            .iter()
            .any(|(b, _)| b == branch))
    }

    fn pr_arm_auto_merge(&self, repo: &str, number: u32, head_commit: &str) -> Result<()> {
        self.record(GhCall::PrArmAutoMerge(
            repo.to_owned(),
            number,
            head_commit.to_owned(),
        ))?;
        if let Some(message) = &self.arm_failure {
            return Err(anyhow!("{message}"));
        }
        self.auto_merge.lock().unwrap().push(number);
        Ok(())
    }

    fn open_pr_for(&self, repo: &str, head: &str, base: &str) -> Result<Option<ListedPr>> {
        self.record(GhCall::OpenPrFor(
            repo.to_owned(),
            head.to_owned(),
            base.to_owned(),
        ))?;
        let attempt = {
            let mut lookups = self.lookups.lock().unwrap();
            *lookups += 1;
            *lookups
        };
        if self.fail_lookup_from.is_some_and(|n| attempt >= n) {
            return Err(anyhow!("FakeGitHub: `gh pr list` {attempt} fails"));
        }
        let listed = self.listed.lock().unwrap();
        let candidates: Vec<&ListedPr> = listed
            .iter()
            .filter(|(r, p, state)| {
                r == repo && *state == PrState::Open && p.head_ref == head && !p.is_cross_repository
            })
            .map(|(_, p, _)| p)
            .collect();
        Ok(candidates
            .iter()
            .find(|p| p.base_ref == base)
            .or(candidates.first())
            .map(|p| (*p).clone()))
    }

    fn pr_create(&self, repo: &str, request: &PrRequest) -> Result<String> {
        self.record(GhCall::PrCreate(repo.to_owned(), request.clone()))?;
        let number = u32::try_from(request.head.len()).unwrap_or(u32::MAX - 1) + 1;
        let url = format!("https://github.com/{repo}/pull/{number}");
        let opened = self
            .create_failure
            .as_ref()
            .is_none_or(|(_, opened)| *opened);
        if opened {
            let pr = ListedPr {
                number,
                url: url.clone(),
                head_ref: request.head.clone(),
                base_ref: request.base.clone(),
                is_cross_repository: false,
            };
            self.listed
                .lock()
                .unwrap()
                .push((repo.to_owned(), pr, PrState::Open));
        }
        match &self.create_failure {
            Some((message, _)) => Err(anyhow!("{message}")),
            None => Ok(url),
        }
    }

    fn pr_comment(&self, repo: &str, number: u32, body: &str) -> Result<()> {
        self.record(GhCall::PrComment(repo.to_owned(), number, body.to_owned()))?;
        match &self.comment_failure {
            Some(message) => Err(anyhow!("{message}")),
            None => Ok(()),
        }
    }

    fn pr_comments(&self, repo: &str, number: u32) -> Result<Vec<PrComment>> {
        self.record(GhCall::PrComments(repo.to_owned(), number))?;
        Ok(self
            .pr_comments
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, comments)| comments.clone())
            .unwrap_or_default())
    }

    fn issue_comment(&self, repo: &str, number: u32, body: &str) -> Result<u64> {
        self.record(GhCall::IssueComment(
            repo.to_owned(),
            number,
            body.to_owned(),
        ))?;
        if self.failing_issue_comments.contains(&number) {
            return Err(anyhow!("FakeGitHub: the comment on issue {number} fails"));
        }
        let mut next = self.next_comment_id.lock().unwrap();
        let id = (*next).max(1);
        *next = id + 1;
        Ok(id)
    }

    fn issue_comment_edit(&self, repo: &str, id: u64, body: &str) -> Result<()> {
        self.record(GhCall::IssueCommentEdit(
            repo.to_owned(),
            id,
            body.to_owned(),
        ))?;
        if self.failing_comment_edits {
            return Err(anyhow!("FakeGitHub: the edit of comment {id} fails"));
        }
        Ok(())
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
            open: true,
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

    fn issue_labels(
        &self,
        repo: &str,
        number: u32,
        add: &[String],
        remove: &[String],
    ) -> Result<()> {
        self.record(GhCall::IssueLabels(
            repo.to_owned(),
            number,
            add.to_vec(),
            remove.to_vec(),
        ))?;
        // The same issue in both lists, as GitHub has it once: `issues_open` sees the change too.
        let mut all = self.all_issues.lock().unwrap();
        let mut open = self.open_issues.lock().unwrap();
        let mut found = false;
        for issue in all
            .iter_mut()
            .chain(open.iter_mut())
            .filter(|i| i.number == number)
        {
            found = true;
            issue.labels.retain(|l| !remove.contains(l));
            for label in add {
                if !issue.labels.contains(label) {
                    issue.labels.push(label.clone());
                }
            }
        }
        if !found {
            return Err(anyhow!("FakeGitHub: issue {number} does not exist"));
        }
        Ok(())
    }

    fn commit_status(&self, repo: &str, sha: &str, status: &CommitStatus) -> Result<()> {
        self.record(GhCall::CommitStatus(
            repo.to_owned(),
            sha.to_owned(),
            status.clone(),
        ))?;
        match &self.status_failure {
            Some(message) => Err(anyhow!("{message}")),
            None => {
                self.statuses
                    .lock()
                    .unwrap()
                    .push((sha.to_owned(), status.clone()));
                Ok(())
            }
        }
    }

    fn commit_status_state(&self, repo: &str, sha: &str, context: &str) -> Result<Option<String>> {
        self.record(GhCall::CommitStatusState(
            repo.to_owned(),
            sha.to_owned(),
            context.to_owned(),
        ))?;
        Ok(self
            .statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(s, status)| s == sha && status.context == context)
            .map(|(_, status)| status.state.word().to_owned()))
    }

    fn commit_runs(&self, repo: &str, sha: &str) -> Result<Vec<WorkflowRun>> {
        self.record(GhCall::CommitRuns(repo.to_owned(), sha.to_owned()))?;
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|(s, _)| s == sha)
            .map(|(_, runs)| runs.clone())
            .unwrap_or_default())
    }

    /// As GitHub does: the run's next attempt, queued.
    fn run_rerun_failed(&self, repo: &str, id: u64) -> Result<()> {
        self.record(GhCall::RunRerunFailed(repo.to_owned(), id))?;
        for (_, runs) in self.runs.lock().unwrap().iter_mut() {
            for run in runs.iter_mut().filter(|r| r.id == id) {
                run.attempt += 1;
                run.status = RunStatus::Going;
            }
        }
        Ok(())
    }

    fn run_failed_log(&self, repo: &str, id: u64) -> Result<Vec<JobLog>> {
        self.record(GhCall::RunFailedLog(repo.to_owned(), id))?;
        Ok(self
            .run_logs
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, jobs)| jobs.clone())
            .unwrap_or_default())
    }

    fn pr_merge_commit(&self, repo: &str, number: u32) -> Result<Option<String>> {
        self.record(GhCall::PrMergeCommit(repo.to_owned(), number))?;
        Ok(self
            .merge_commits
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, sha)| sha.clone()))
    }

    fn pr_files(&self, repo: &str, number: u32) -> Result<PrFiles> {
        self.record(GhCall::PrFiles(repo.to_owned(), number))?;
        self.pr_files
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, files)| files.clone())
            .ok_or_else(|| anyhow!("FakeGitHub: the files of pr {number} are not scripted"))
    }

    fn pr_labels(&self, repo: &str, number: u32) -> Result<Vec<String>> {
        self.record(GhCall::PrLabels(repo.to_owned(), number))?;
        if let Some(message) = &self.pr_labels_failure {
            return Err(anyhow!("{message}"));
        }
        Ok(self.pr_labels_now(number))
    }

    fn pr_remove_label(&self, repo: &str, number: u32, label: &str) -> Result<()> {
        self.record(GhCall::PrRemoveLabel(
            repo.to_owned(),
            number,
            label.to_owned(),
        ))?;
        if let Some(message) = &self.remove_label_failure {
            return Err(anyhow!("{message}"));
        }
        let mut all = self.pr_labels.lock().unwrap();
        let labels = all
            .iter_mut()
            .find(|(n, _)| *n == number)
            .map(|(_, labels)| labels)
            .filter(|labels| labels.iter().any(|l| l == label))
            .ok_or_else(|| anyhow!("FakeGitHub: PR {number} has no label {label}"))?;
        labels.retain(|l| l != label);
        Ok(())
    }

    fn commit_statuses(&self, repo: &str, sha: &str) -> Result<Vec<PostedStatus>> {
        self.record(GhCall::CommitStatuses(repo.to_owned(), sha.to_owned()))?;
        if let Some(message) = &self.statuses_failure {
            return Err(anyhow!("{message}"));
        }
        Ok(self
            .posted_statuses
            .iter()
            .find(|(s, _)| s == sha)
            .map(|(_, statuses)| statuses.clone())
            .unwrap_or_default())
    }
}
