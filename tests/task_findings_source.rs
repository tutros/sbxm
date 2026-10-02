//! M2b slice 11, step 7: `task file-findings --issue N | --pr N` reads the review of that task
//! (`<base>/.sbxm/tasks/<id>/review.md`, spec §3, §4) and files it in the task's repo.

use std::fs;
use std::path::Path;

use sbxm::commands::task_file_findings::{self, Options, Source};
use sbxm::github::GitHubBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Kind, NewTask, Process, Record};
use tempfile::TempDir;

fn small() -> String {
    include_str!("fixtures/review-findings/review-small.md").replace("\r\n", "\n")
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix", "should-fix", "question"])
        .with_next_issue_number(40)
}

/// A task folder with a record for `repo` and, when given, a `review.md`.
fn task(
    base: &Path,
    kind: Kind,
    number: u32,
    repo: &str,
    review: Option<&str>,
) -> std::path::PathBuf {
    let id = match kind {
        Kind::Issue => format!("issue-{number}"),
        Kind::Pr => format!("pr-{number}"),
    };
    let record = Record::new(
        &NewTask {
            kind,
            number,
            repo,
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
        },
        1_790_000_000,
        Process::new(1, 1_790_000_000),
    );
    let dir = record::task_dir(base, &id);
    record::write(&dir, &record).unwrap();
    if let Some(text) = review {
        fs::write(dir.join("review.md"), text).unwrap();
    }
    dir
}

fn run(
    base: &Path,
    kind: Kind,
    number: u32,
    repo: Option<&str>,
    create: bool,
    github: &FakeGitHub,
) -> (anyhow::Result<()>, String) {
    let opts = Options {
        source: Source::Task {
            base_dir: base.to_path_buf(),
            kind,
            number,
        },
        repo_root: base.to_path_buf(),
        repo: repo.map(str::to_owned),
        create,
        standard_criteria: false,
        keep_paths: false,
        only: Vec::new(),
    };
    let mut out = Vec::new();
    let result = task_file_findings::run(&opts, github, &mut out, &mut Vec::new());
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn an_issue_task_is_filed_in_its_own_repo_with_a_marker_that_names_the_task() {
    let base = TempDir::new().unwrap();
    let dir = task(base.path(), Kind::Issue, 12, "o/r", Some(&small()));
    let gh = github();

    let (result, out) = run(base.path(), Kind::Issue, 12, None, true, &gh);

    assert!(result.is_ok(), "{result:?}");
    let all = gh.issues_all("o/r", 1000).unwrap();
    assert_eq!(all.len(), 3);
    assert!(
        all[0]
            .body
            .contains("<!-- review-finding: issue-12-review.md#S-2 -->"),
        "{}",
        all[0].body
    );
    assert!(
        all[0]
            .body
            .contains("**Review:** `sbxm task issue-12: review.md`, finding S-2")
    );
    assert!(out.contains("created"));
    let written = fs::read_to_string(dir.join("review.md")).unwrap();
    assert!(
        written.contains("Issues: S-1 #41, S-2 #40, S-Q1 #42"),
        "{written}"
    );
}

#[test]
fn a_pr_task_reads_its_own_folder() {
    let base = TempDir::new().unwrap();
    task(base.path(), Kind::Pr, 7, "o/r", Some(&small()));
    let gh = github();

    let (result, out) = run(base.path(), Kind::Pr, 7, None, false, &gh);

    assert!(result.is_ok(), "{result:?}");
    assert!(
        out.contains("would create [must-fix] S-1: Thing breaks"),
        "{out}"
    );
    assert!(out.contains("review-finding: pr-7-review.md#S-1"), "{out}");
}

#[test]
fn the_same_finding_ids_of_two_tasks_do_not_collide() {
    let base = TempDir::new().unwrap();
    task(base.path(), Kind::Issue, 12, "o/r", Some(&small()));
    task(base.path(), Kind::Issue, 13, "o/r", Some(&small()));
    let gh = github();

    assert!(run(base.path(), Kind::Issue, 12, None, true, &gh).0.is_ok());
    assert!(run(base.path(), Kind::Issue, 13, None, true, &gh).0.is_ok());

    assert_eq!(gh.issues_all("o/r", 1000).unwrap().len(), 6);
}

#[test]
fn repo_overrides_the_tasks_repo() {
    let base = TempDir::new().unwrap();
    task(base.path(), Kind::Issue, 12, "o/r", Some(&small()));
    let gh = github();

    run(base.path(), Kind::Issue, 12, Some("x/y"), false, &gh)
        .0
        .unwrap();

    assert!(
        gh.calls()
            .iter()
            .all(|c| !format!("{c:?}").contains("\"o/r\""))
    );
}

#[test]
fn a_task_that_does_not_exist_is_refused_before_any_github_call() {
    let base = TempDir::new().unwrap();
    let gh = github();

    let (result, _) = run(base.path(), Kind::Issue, 12, None, false, &gh);

    assert_eq!(
        result.unwrap_err().to_string(),
        "no task issue-12; run `sbxm task status` to list the tasks"
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn a_task_without_a_review_says_how_to_get_one() {
    let base = TempDir::new().unwrap();
    task(base.path(), Kind::Issue, 12, "o/r", None);
    let gh = github();

    let (result, _) = run(base.path(), Kind::Issue, 12, None, false, &gh);

    assert_eq!(
        result.unwrap_err().to_string(),
        "task issue-12 has no review.md yet; run `sbxm task review --issue 12` first"
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn a_pr_task_without_a_review_names_the_pr_flag() {
    let base = TempDir::new().unwrap();
    task(base.path(), Kind::Pr, 7, "o/r", None);

    let (result, _) = run(base.path(), Kind::Pr, 7, None, false, &github());

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("`sbxm task review --pr 7`")
    );
}
