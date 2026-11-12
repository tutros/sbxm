//! Issue 485: fix tasks of PR 7 in the states `task rm --pr` and `task resume --pr` meet, and the
//! two commands run with `--pr 7`.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use super::git;
use super::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, add_pr_head, ctx, fixture_with, ok, play, play_reviews,
    source,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_rm;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrComment, PrInfo, PrState};
use sbxm::task::pipeline;
use sbxm::task::pr_fix::review_to_fix;
use sbxm::task::record::{self, ProcessProbe, Stage, Status};
use sbxm::task::review::pr_comment;

/// Every recorded sbxm process is gone: an interrupted task.
pub struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Every recorded sbxm process still runs.
pub struct Alive;

impl ProcessProbe for Alive {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(0)
    }
}

pub struct FakeConfirm {
    pub interactive: bool,
    pub answer: bool,
    pub prompts: RefCell<Vec<String>>,
}

impl Confirm for FakeConfirm {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.prompts.borrow_mut().push(prompt.to_owned());
        Ok(self.answer)
    }
}

pub fn yes_on_a_terminal() -> FakeConfirm {
    FakeConfirm {
        interactive: true,
        answer: true,
        prompts: RefCell::new(Vec::new()),
    }
}

pub fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

pub fn pr7(head: &str) -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "This adds x.".into(),
        closing_issues: vec![],
        base_moved: false,
    }
}

pub fn posted_review(head: &str) -> PrComment {
    let review = format!(
        "Must-fix findings: 1\nRisk: medium\n\nScope: `0000000..{head}`\nIssues: pending\n\n\
         ## Must fix\n\n### M-1 - The lock is never released\n\n**Where:** `src/lock.rs:10`\n\
         **What happens:** it stays held\n**Why it matters:** decision 1\n**Fix:** release it\n"
    );
    PrComment {
        author: "tutros".into(),
        body: pr_comment(7, "", &review),
    }
}

/// A new head of PR 7 on the remote's `feature-x`.
pub fn new_head(f: &Fixture) -> String {
    let head = add_pr_head(f, 7);
    git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    head
}

pub const CLEAN: &str =
    "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing.\n";

/// The worker commits `src/lock.rs` on `feature-x`; the in-task review is clean.
pub fn worker_backend(f: &Fixture, id: &str) -> FakeBackend {
    let worker = play(
        &f.env.base_dir().join("tasks").join(id),
        "main",
        "feature-x",
        Play {
            commits: vec!["src/lock.rs".into()],
            result_md: Some(b"M-1 fixed\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), id, vec![CLEAN.to_owned()]);
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        })
}

/// A fix task of PR 7 at `head`, prepared and then cut off at its worker with 0 commits (the
/// state of issue 485), its sandbox left in `backend`. Returns its id.
pub fn interrupted_fix(f: &Fixture, head: &str, backend: &FakeBackend) -> String {
    let pr = pr7(head);
    let review = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(head)]).unwrap();
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, backend, &github);
    let mut prepared = pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap();
    prepared.record.stage = Stage::Working;
    prepared.record.status = Status::Running;
    // Its process was cut off mid-run: the lock goes without the run's end (issue 597), so the
    // record keeps naming it.
    prepared.lock = None;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared.record.id.clone()
}

/// A fix task of PR 7 at `head` whose worker made a commit and whose review passed: `ready`,
/// its result only in its `repo.git`.
pub fn ready_fix(f: &Fixture, head: &str) -> String {
    let pr = pr7(head);
    let review = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(head)]).unwrap();
    let id = record::fix_id(7, head).unwrap();
    let backend = worker_backend(f, &id);
    let github = FakeGitHub::default();
    let source = source(f);
    let context = ctx(f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap();
    pipeline::run_worker(&context, &mut prepared).unwrap();
    pipeline::review_issue(&context.env(), &mut prepared).unwrap();
    assert_eq!(prepared.record.stage, Stage::Ready);
    id
}

/// The review task `pr-7`, as `task review --pr 7` leaves it (only its folders matter here).
pub fn review_task(f: &Fixture) -> PathBuf {
    let workspace = f.env.base_dir().join("tasks").join("pr-7");
    fs::create_dir_all(&workspace).unwrap();
    workspace
}

pub fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

pub fn workspace(f: &Fixture, id: &str) -> PathBuf {
    f.env.base_dir().join("tasks").join(id)
}

pub struct Out {
    pub result: anyhow::Result<()>,
    pub out: String,
}

pub fn rm(f: &Fixture, force: bool, backend: &FakeBackend, confirm: &FakeConfirm) -> Out {
    let mut out = Vec::new();
    let result = task_rm::run(
        &f.env.config_dir(),
        &task_rm::Options {
            target: task_rm::Target::Pr(7),
            yes: false,
            force,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
            forget_ambiguous: false,
        },
        backend,
        &FakeGitHub::default(),
        &Gone,
        confirm,
        &mut out,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

pub fn github_at(head: &str) -> FakeGitHub {
    FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_pr(pr7(head))
}

pub fn origin_head(f: &Fixture) -> String {
    git(&f.origin, &["rev-parse", "refs/heads/feature-x"])
}
