//! Issue 727, decision 331(3): a loop task whose changed files overlap other open work once its
//! gates pass is held before its review, then released, merges the base, runs its gates again
//! and is reviewed once. One test per boundary case of the issue.

use crate::common;

use std::fs;
use std::path::Path;

use common::git;
use common::task_fixture::Probe;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrFile, PrFiles, PrInfo, PrState};
use sbxm::run::results::format_timestamp;
use sbxm::task::hold::{self, Check, Checker, Wait};
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Stamp, Status};

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;

/// The process the fake probe says is alive.
fn alive() -> Process {
    Process::new(1, 0)
}

/// A task `issue-<n>` in `base` whose branch changes `files` past `main`, at `stage` and
/// `status`, its gates started at `gates_at`.
fn task(
    base: &Path,
    n: u32,
    files: &[&str],
    stage: Stage,
    status: Status,
    gates_at: u64,
) -> Record {
    let id = format!("issue-{n}");
    let dir = record::task_dir(base, &id);
    let repo_git = dir.join("repo.git");
    fs::create_dir_all(&repo_git).unwrap();
    let scratch = base.join(format!("scratch-{n}"));
    fs::create_dir_all(&scratch).unwrap();
    git(&scratch, &["init", "-q", "-b", "main"]);
    fs::write(scratch.join("README.md"), "base\n").unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "base"]);
    git(&scratch, &["checkout", "-q", "-b", &id]);
    for file in files {
        let path = scratch.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{id}\n")).unwrap();
    }
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "work"]);
    git(&repo_git, &["init", "-q", "--bare"]);
    git(
        &scratch,
        &[
            "push",
            "-q",
            repo_git.to_str().unwrap(),
            "main:refs/heads/main",
            &format!("{id}:refs/heads/{id}"),
        ],
    );
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: n,
            repo: REPO,
            title: "",
            base: "main",
            branch: &id,
            config_hash: "h",
            id: None,
        },
        START,
        alive(),
    );
    rec.stage = stage;
    rec.status = status;
    rec.stages.push(Stamp {
        stage: Stage::Gating,
        at: format_timestamp(gates_at),
    });
    if stage != Stage::Gating {
        rec.stages.push(Stamp {
            stage,
            at: format_timestamp(gates_at + 60),
        });
    }
    record::write(&dir, &rec).unwrap();
    rec
}

/// A finished loop task `issue-<n>` that opened PR `pr`.
fn loop_pr(base: &Path, n: u32, pr: u32) {
    let id = format!("issue-{n}");
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: n,
            repo: REPO,
            title: "",
            base: "main",
            branch: &id,
            config_hash: "h",
            id: None,
        },
        START,
        alive(),
    );
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.process = None;
    rec.pr = Some(format!("https://github.com/o/r/pull/{pr}"));
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    record::write(&record::task_dir(base, &id), &rec).unwrap();
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("issue-{number}"),
        head_oid: "0123456789abcdef0123456789abcdef01234567".into(),
        is_cross_repository: false,
        head_repo: REPO.into(),
        state,
        title: format!("PR {number}"),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    }
}

fn files(paths: &[&str]) -> PrFiles {
    PrFiles {
        changed_files: paths.len() as u32,
        files: paths
            .iter()
            .map(|p| PrFile {
                filename: (*p).to_owned(),
                previous_filename: None,
            })
            .collect(),
    }
}

fn waits(base: &Path, github: &FakeGitHub, own: &Record) -> Vec<Wait> {
    let repo_git = record::task_dir(base, &own.id).join("repo.git");
    let paths = own.changed_paths(&repo_git).unwrap();
    Checker {
        base_dir: base,
        repo: REPO,
        github,
        probe: &Probe,
    }
    .waits(own, &paths)
    .unwrap()
}

#[test]
fn task_loop_hold_no_overlap_is_not_held() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Open))
        .with_pr_files(30, files(&["src/other.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    assert_eq!(waits(base, &github, &own), Vec::<Wait>::new());
}

#[test]
fn task_loop_hold_one_shared_file_with_an_open_loop_pr_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Open))
        .with_pr_files(30, files(&["docs/x.md", "src/a.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    let on = waits(base, &github, &own);
    assert_eq!(
        on,
        vec![Wait::Pr {
            number: 30,
            armed: false
        }]
    );
    assert_eq!(
        hold::waits_line(&on),
        "waits on #30 (PR, awaits your merge)"
    );
}

#[test]
fn task_loop_hold_a_shared_file_with_a_task_in_review_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    task(
        base,
        8,
        &["src/a.rs"],
        Stage::Reviewing,
        Status::Running,
        START,
    );
    let github = FakeGitHub::default();
    let own = task(
        base,
        9,
        &["src/a.rs"],
        Stage::Gating,
        Status::Passed,
        START + 60,
    );
    let on = waits(base, &github, &own);
    assert_eq!(
        on,
        vec![Wait::Task {
            task: "issue-8".into(),
            stage: Stage::Reviewing
        }]
    );
    assert_eq!(hold::waits_line(&on), "waits on issue-8 (in review)");
}

#[test]
fn task_loop_hold_a_shared_file_only_with_a_merged_pr_is_not_held() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Merged))
        .with_pr_files(30, files(&["src/a.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    assert_eq!(waits(base, &github, &own), Vec::<Wait>::new());
}

#[test]
fn task_loop_hold_two_tasks_whose_gates_pass_together_the_first_holds_the_second_only() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let github = FakeGitHub::default();
    let first = task(base, 8, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    let second = task(
        base,
        9,
        &["src/a.rs"],
        Stage::Gating,
        Status::Passed,
        START + 1,
    );
    // Both see the other with its gates passed: only the later one waits.
    assert_eq!(waits(base, &github, &first), Vec::<Wait>::new());
    assert_eq!(
        waits(base, &github, &second),
        vec![Wait::Task {
            task: "issue-8".into(),
            stage: Stage::Gating
        }]
    );
    // Once the second is held, it holds nothing: the first goes on to its review.
    let mut held = second.clone();
    held.stage = Stage::Held;
    held.status = Status::Ok;
    hold::start(&mut held, START + 2, vec![]);
    record::write(&record::task_dir(base, &held.id), &held).unwrap();
    assert_eq!(waits(base, &github, &first), Vec::<Wait>::new());
}
