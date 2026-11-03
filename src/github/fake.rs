//! `FakeGitHub`: records calls, answers from what a test scripted.

use std::sync::Mutex;

use anyhow::{Result, anyhow};

use super::{
    CommitStatus, GitHubBackend, Issue, IssueRequest, IssueText, ListedPr, PrComment, PrInfo,
    PrMergeState, PrRequest, PrState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhCall {
    Whoami,
    DefaultBranch(String),
    IssuesOpen(String),
    Issue(String, u32),
    Pr(String, u32),
    PrMergeState(String, u32),
    /// Repo, head, base.
    OpenPrFor(String, String, String),
    PrCreate(String, PrRequest),
    PrComment(String, u32, String),
    PrComments(String, u32),
    IssueCreate(String, IssueRequest),
    Labels(String),
    IssuesAll(String, u32),
    IssueEdit(String, u32, String),
    /// Repo, number, labels added, labels removed.
    IssueLabels(String, u32, Vec<String>, Vec<String>),
    /// Repo, commit, status.
    CommitStatus(String, String, CommitStatus),
}

#[derive(Default)]
pub struct FakeGitHub {
    user: String,
    default_branch: String,
    open_issues: Mutex<Vec<Issue>>,
    issue_texts: Vec<IssueText>,
    prs: Vec<PrInfo>,
    /// The scripted PRs with auto-merge armed.
    auto_merge: Vec<u32>,
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
    status_failure: Option<String>,
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

    pub fn with_pr(mut self, pr: PrInfo) -> Self {
        self.prs.push(pr);
        self
    }

    /// Arms auto-merge on the scripted PR `number`, as `pr_merge_state` reports it.
    pub fn with_auto_merge(mut self, number: u32) -> Self {
        self.auto_merge.push(number);
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

    /// Only `commit_status` fails (and is still recorded).
    pub fn failing_statuses(mut self, message: &str) -> Self {
        self.status_failure = Some(message.to_owned());
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
        Ok(self.open_issues.lock().unwrap().clone())
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

    fn pr_merge_state(&self, repo: &str, number: u32) -> Result<PrMergeState> {
        self.record(GhCall::PrMergeState(repo.to_owned(), number))?;
        let pr = self
            .prs
            .iter()
            .find(|p| p.number == number)
            .ok_or_else(|| anyhow!("FakeGitHub: pr {number} is not scripted"))?;
        Ok(PrMergeState {
            state: pr.state,
            auto_merge: self.auto_merge.contains(&number),
        })
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
            None => Ok(()),
        }
    }
}
