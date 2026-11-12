//! Milestone 5 slice 2 (issue 600, decisions 317 and 321): the loop picks each task's location
//! from the issue's `run-cloud` / `run-local` labels, and `task fix --pr N` runs where the PR's
//! task ran unless `--cloud` or `--local` says otherwise.

mod common;

use std::fs;

use sbxm::backend::Location;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::location::{self, NEEDS_CLOUD_SLOTS, RUN_CLOUD_LABEL, RUN_LOCAL_LABEL};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

use common::task_fixture::open_issue;

const REPO: &str = "o/r";

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

// --- the labels ---

#[test]
fn no_label_leaves_the_location_to_the_default() {
    assert_eq!(location::for_loop(&labels(&["size:S"]), None), Ok(None));
    assert_eq!(location::for_loop(&labels(&[]), Some(2)), Ok(None));
}

#[test]
fn run_local_is_local_even_with_cloud_slots_set() {
    assert_eq!(
        location::for_loop(&labels(&[RUN_LOCAL_LABEL]), Some(2)),
        Ok(Some(Location::Local))
    );
    assert_eq!(
        location::for_loop(&labels(&[RUN_LOCAL_LABEL]), None),
        Ok(Some(Location::Local))
    );
}

#[test]
fn run_cloud_with_cloud_slots_set_is_cloud() {
    assert_eq!(
        location::for_loop(&labels(&["size:S", RUN_CLOUD_LABEL]), Some(2)),
        Ok(Some(Location::Cloud))
    );
    assert_eq!(
        location::for_loop(&labels(&[RUN_CLOUD_LABEL]), Some(1)),
        Ok(Some(Location::Cloud))
    );
}

#[test]
fn run_cloud_with_cloud_slots_unset_is_a_skip_with_its_reason() {
    assert_eq!(
        location::for_loop(&labels(&[RUN_CLOUD_LABEL]), None),
        Err(NEEDS_CLOUD_SLOTS.to_owned())
    );
    assert_eq!(NEEDS_CLOUD_SLOTS, "run-cloud needs [resources] cloud_slots");
}

#[test]
fn both_labels_are_a_skip_naming_both() {
    for slots in [None, Some(2)] {
        let why =
            location::for_loop(&labels(&[RUN_LOCAL_LABEL, RUN_CLOUD_LABEL]), slots).unwrap_err();
        assert!(
            why.contains("run-cloud") && why.contains("run-local"),
            "{why}"
        );
    }
}

// --- a fix's location without a flag ---

/// Writes the record of `issue-41`, whose task opened PR 7 of `repo`, run at `at`.
fn opened_pr7(base: &std::path::Path, repo: &str, at: Location) {
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo,
            title: "",
            base: "main",
            branch: "issue-41",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 1),
    );
    rec.location = at;
    rec.pr = Some(format!("https://github.com/{repo}/pull/7"));
    let dir = record::task_dir(base, "issue-41");
    fs::create_dir_all(&dir).unwrap();
    record::write(&dir, &rec).unwrap();
}

fn pr7_closing(issues: Vec<u32>) -> sbxm::github::PrInfo {
    let mut pr = common::fix_task::pr7(&"a".repeat(40));
    pr.closing_issues = issues;
    pr
}

#[test]
fn a_fix_follows_the_location_its_pr_task_recorded() {
    for at in [Location::Cloud, Location::Local] {
        let env = common::Env::new();
        opened_pr7(&env.base_dir(), REPO, at);
        // The record decides: GitHub isn't asked.
        let gh = FakeGitHub::default();

        let found = location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap();

        assert_eq!(found, Some(at));
        assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    }
}

#[test]
fn a_record_of_another_repo_or_pr_does_not_count() {
    let env = common::Env::new();
    opened_pr7(&env.base_dir(), "o/other", Location::Cloud);
    let mut pr8 = pr7_closing(vec![]);
    pr8.number = 8;
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![]))
        .with_pr(pr8)
        .with_open_issues(vec![]);

    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        None
    );
    assert_eq!(
        location::of_pr(&env.base_dir(), "o/other", 8, &gh).unwrap(),
        None
    );
}

#[test]
fn with_no_record_a_fix_follows_its_issues_label() {
    let env = common::Env::new();
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![41]))
        .with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);

    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        Some(Location::Cloud)
    );

    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![41]))
        .with_open_issues(vec![open_issue(41, &[RUN_LOCAL_LABEL], "")]);
    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        Some(Location::Local)
    );
}

#[test]
fn with_no_record_and_no_label_a_fix_has_no_location_of_its_own() {
    let env = common::Env::new();
    for issue in [
        open_issue(41, &["size:S"], ""),
        // Both labels pin nothing: the default stands.
        open_issue(41, &[RUN_CLOUD_LABEL, RUN_LOCAL_LABEL], ""),
    ] {
        let gh = FakeGitHub::default()
            .with_pr(pr7_closing(vec![41]))
            .with_open_issues(vec![issue]);
        assert_eq!(
            location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
            None
        );
    }
    // A hand-made PR that closes no issue.
    let gh = FakeGitHub::default()
        .with_pr(pr7_closing(vec![]))
        .with_open_issues(vec![open_issue(41, &[RUN_CLOUD_LABEL], "")]);
    assert_eq!(
        location::of_pr(&env.base_dir(), REPO, 7, &gh).unwrap(),
        None
    );
}
