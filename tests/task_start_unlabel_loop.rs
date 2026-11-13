//! Issue 623: a task the loop starts keeps the loop's own label removal; the child `task start`
//! the loop runs doesn't make a second one. This test binary is such a child: it sets the loop
//! for the whole process, as `main` does from the loop's environment variable.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, Play, Probe, backend, fixture, issue_text, ok, open_issue, play_tasks, source,
};
use sbxm::commands::task_start;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::record::{self, Process};
use sbxm::task::repo::Identity;

#[test]
fn a_start_the_loop_runs_leaves_the_label_to_the_loop() {
    record::started_by_loop(Process::new(77, 1_793_613_600));
    let f = fixture();
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let gh = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["ready-for-worker"], "")]);
    let opts = task_start::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(source(&f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    task_start::run(
        &f.env.config_dir(),
        &opts,
        &b,
        &gh,
        &Probe,
        &mut out,
        &mut warn,
    )
    .unwrap();

    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueLabels(..))),
        "{:?}",
        gh.calls()
    );
}
