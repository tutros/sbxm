//! Issue 649: a queued `review` job reviews the PR's head even when the in-task
//! review already covered it, as `task review --pr N --force` does, on the loop's one-at-a-time
//! path too (`cloud_slots` unset); a closed, merged or fork PR is still refused as such. The
//! child-process path's `--force` is in `task_loop_job_children`'s commands; `task review --pr N`
//! run by hand still refuses an already-reviewed head (`task_review_cmd`).

use crate::common;
use crate::task_review_cmd::{REVIEWED, config, finished_issue_task, pr_github_at, pr_github_with};

use anyhow::Result;
use common::task_fixture::{Fixture, Probe, add_pr_head};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_loop::child::{Child, Exit, Spawner};
use sbxm::commands::task_loop::{Steps, TaskSteps};
use sbxm::github::PrState;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;

/// The one-at-a-time loop starts no child for a review.
struct NoSpawn;

impl Spawner for NoSpawn {
    fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
        panic!("a one-at-a-time review starts no child: {child:?}")
    }

    fn stop(&self) {}
}

/// Issue 4's finished task, whose in-task review covered and posted `REVIEWED` on PR 7.
fn reviewed_fixture() -> Fixture {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    f
}

/// The loop's own step for a queued `review --pr 7` job, run here. The backend holds no
/// secrets, so a review that runs stops at the reviewer's secret check, before any clone.
fn review_job(f: &Fixture, github: &FakeGitHub) -> String {
    let (backend, host) = (FakeBackend::default(), FakeHostRunner::default());
    let config_dir = f.env.config_dir();
    let steps = TaskSteps {
        config_dir: &config_dir,
        repo_root: f.env.tmp.path().join("target-repo"),
        repo: Some("o/r".into()),
        backend: &backend,
        github,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
        spawner: &NoSpawn,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = steps.review_pr(7, &mut out, &mut warn);
    format!("{:#}", result.expect_err("the backend has no secrets"))
}

#[test]
fn task_loop_review_force_a_queued_review_of_an_already_reviewed_head_runs() {
    let f = reviewed_fixture();

    let message = review_job(&f, &pr_github_at(REVIEWED));

    assert!(!message.contains("already reviewed"), "{message}");
    assert!(message.contains("secret"), "{message}");
}

#[test]
fn task_loop_review_force_a_queued_review_of_a_new_head_runs() {
    let f = reviewed_fixture();

    let message = review_job(
        &f,
        &pr_github_at("3333333333333333333333333333333333333333"),
    );

    assert!(!message.contains("already reviewed"), "{message}");
    assert!(message.contains("secret"), "{message}");
}

#[test]
fn task_loop_review_force_a_queued_review_of_a_merged_pr_is_refused_as_merged() {
    let f = reviewed_fixture();

    let message = review_job(&f, &pr_github_with(REVIEWED, PrState::Merged, false));

    assert!(
        message.contains("PR #7 is merged; only open PRs are reviewed"),
        "{message}"
    );
    assert!(!message.contains("already reviewed"), "{message}");
}
