//! Issue 202, decision 195: `task::merged::plan` finds the tasks whose work is merged.

mod common;

use std::fs;

use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::finish::finish;
use sbxm::task::merged::plan;
use sbxm::task::record::{self, Process, Stage, Status};

/// As `review::with_header` saves it: the reviewer's line, a blank line, then the review.
const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done by hand: `ready`, with a result and a review.
fn ready(f: &Fixture) -> sbxm::task::pipeline::Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = common::task_fixture::worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    prepared
}

/// `ready`, then finished (pushed and a PR opened): `task.pr` names the PR `finish` opened.
fn finished(f: &Fixture) -> String {
    ready(f);
    let github = FakeGitHub::default();
    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();
    done.url
}

fn pr_number(url: &str) -> u32 {
    sbxm::task::finish::pr_number(url).unwrap()
}

fn repo_root(f: &Fixture) -> std::path::PathBuf {
    f.env.tmp.path().join("target-repo")
}

#[test]
fn an_issue_task_with_no_pr_yet_is_not_a_candidate() {
    let f = fixture();
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    common::task_fixture::worked_task(&f, &b);
    let github = FakeGitHub::default();

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn an_issue_task_whose_pr_is_merged_is_a_candidate() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert_eq!(found.candidates.len(), 1, "{found:?}");
    assert_eq!(found.candidates[0].id, "issue-41");
    assert!(
        found.candidates[0].reason.contains(&format!("#{number}")),
        "{}",
        found.candidates[0].reason
    );
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn an_issue_task_whose_pr_is_open_is_left_alone() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}

#[test]
fn an_issue_task_whose_pr_closed_without_merging_is_left_alone() {
    let f = fixture();
    let url = finished(&f);
    let number = pr_number(&url);
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        state: PrState::Closed,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
    });

    let found = plan(&f.env.base_dir(), &repo_root(&f), &github, &Probe).unwrap();

    assert!(found.is_empty(), "{found:?}");
    assert!(found.left_alone.is_empty(), "{found:?}");
}
