//! M2b slice 10, steps 4-5: `finish` pushes the branch from `repo.git` to the remote (a local
//! bare repo standing in for GitHub) and opens the PR (spec §4, decisions 153, 158).

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::finish::{SECTION_CAP, check_can_finish, finish, pr_body};
use sbxm::task::pipeline::Prepared;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Status};

const REVIEW: &str = "Reviewer: codex (default)\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done by hand: `ready`, with a result and a review.
fn ready(f: &Fixture) -> Prepared {
    ready_with(f, &["a.txt"])
}

/// [`ready`], where the worker commits `files` (paths inside the repo).
fn ready_with(f: &Fixture, files: &[&str]) -> Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: files.iter().map(|s| (*s).to_owned()).collect(),
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = worked_task(f, &b);
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

fn origin_has(f: &Fixture, branch: &str) -> bool {
    std::process::Command::new("git")
        .current_dir(&f.origin)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .unwrap()
        .success()
}

fn reread(f: &Fixture) -> record::Record {
    Prepared::open(&f.env.base_dir(), "issue-41")
        .unwrap()
        .record
}

fn run(f: &Fixture, github: &FakeGitHub) -> anyhow::Result<sbxm::task::finish::Finished> {
    finish(&f.env.base_dir(), "issue-41", github, &Probe)
}

#[test]
fn a_ready_task_is_pushed_and_its_pr_is_opened_with_the_result_and_review() {
    let f = fixture();
    let prepared = ready(&f);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    // The branch really went to the remote, and it is the commits of repo.git.
    assert!(origin_has(&f, "issue-41"));
    assert_eq!(
        git(&f.origin, &["rev-parse", "issue-41"]),
        git(&prepared.meta.join("repo.git"), &["rev-parse", "issue-41"])
    );
    let calls = github.calls();
    let [GhCall::PrCreate(repo, request)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert_eq!(repo, "o/r");
    assert_eq!(
        (request.head.as_str(), request.base.as_str()),
        ("issue-41", "main")
    );
    assert_eq!(request.title, "Fix 41");
    assert!(request.body.starts_with("Fixes #41\n"), "{}", request.body);
    assert!(
        request.body.contains("## Result\n\ndone\n"),
        "{}",
        request.body
    );
    assert!(
        request.body.contains("## Review\n\nReviewer: codex"),
        "{}",
        request.body
    );
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
}

#[test]
fn a_task_that_is_not_ready_is_refused_before_anything_is_pushed_or_opened() {
    let f = fixture();
    let b = backend().with_exec_output_matching("claude", ok(CLAUDE_DONE));
    worked_task(&f, &b);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("not ready; run `sbxm task review --issue 41`"),
        "{message}"
    );
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_task_with_a_pr_is_refused_and_nothing_is_pushed_again() {
    let f = fixture();
    ready(&f);
    let github = FakeGitHub::default();
    run(&f, &github).unwrap();
    let before = git(&f.origin, &["rev-parse", "issue-41"]);

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("already has a PR"), "{err:#}");
    assert_eq!(github.calls().len(), 1);
    assert_eq!(git(&f.origin, &["rev-parse", "issue-41"]), before);
}

#[test]
fn a_pr_that_cannot_be_opened_leaves_a_ready_task_with_a_note_and_a_rerun_opens_it() {
    let f = fixture();
    ready(&f);
    let down = FakeGitHub::default().failing("gh: HTTP 502");

    let err = run(&f, &down).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("pushed issue-41 but could not open the PR"),
        "{message}"
    );
    assert!(message.contains("gh: HTTP 502"), "{message}");
    assert!(origin_has(&f, "issue-41"));
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(record.pr.is_none());
    assert_eq!(record.notes.len(), 1, "{:?}", record.notes);

    let up = FakeGitHub::default();
    let done = run(&f, &up).unwrap();

    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
    assert!(record.notes.is_empty(), "{:?}", record.notes);
}

#[test]
fn a_failed_push_opens_no_pr_and_leaves_the_record_as_it_was() {
    let f = fixture();
    let prepared = ready(&f);
    let before = fs::read_to_string(prepared.meta.join("task.json")).unwrap();
    // The remote is gone.
    fs::remove_dir_all(&f.origin).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(
        format!("{err:#}").contains("cannot push issue-41"),
        "{err:#}"
    );
    assert!(github.calls().is_empty());
    assert_eq!(
        fs::read_to_string(prepared.meta.join("task.json")).unwrap(),
        before
    );
}

#[test]
fn a_task_without_commits_beyond_the_base_is_refused() {
    let f = fixture();
    let prepared = ready(&f);
    let repo_git = prepared.meta.join("repo.git");
    git(&repo_git, &["branch", "-f", "issue-41", "main"]);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("no commits"), "{err:#}");
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_secret_in_review_or_result_stops_the_publish_without_echoing_it() {
    let f = fixture();
    let prepared = ready(&f);
    fs::write(
        prepared.meta.join("review.md"),
        "Must-fix findings: 0\nkey: ghp_abcdefghijklmnopqrstuvwxyz0123456789\n",
    )
    .unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("review.md line 2 looks like a secret"),
        "{message}"
    );
    assert!(!message.contains("ghp_abc"), "{message}");
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_huge_file_is_cut_to_the_cap_and_the_body_says_so() {
    let big = "x".repeat(SECTION_CAP * 2);

    let (body, cuts) = pr_body(7, Some(&big), None);

    assert!(body.chars().count() < 65_536, "{}", body.len());
    assert!(
        body.contains("(cut: result.md has 50000 characters"),
        "{body}"
    );
    assert!(body.contains("(no review.md)"), "{body}");
    assert_eq!(cuts.len(), 1);
}

#[test]
fn a_record_that_names_a_hostile_branch_is_refused() {
    let f = fixture();
    let mut prepared = ready(&f);
    prepared.record.branch = "--upload-pack=x".into();
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(
        format!("{err:#}").contains("isn't a usable branch name"),
        "{err:#}"
    );
    assert!(github.calls().is_empty());
}

// ---- Slice 10 review, must-fix 2: the agent's commits must not start CI before anyone looks ----

#[test]
fn a_branch_that_changes_a_workflow_is_refused_before_anything_is_pushed() {
    for path in [
        ".github/workflows/ci.yml",
        ".github/actions/setup/action.yml",
        // GitHub reads these paths as written; sbxm must not be fooled by letter case.
        ".GitHub/Workflows/ci.yml",
    ] {
        let f = fixture();
        ready_with(&f, &["a.txt", path]);
        let before =
            fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json"))
                .unwrap();
        let github = FakeGitHub::default();

        let message = format!("{:#}", run(&f, &github).unwrap_err());

        assert!(message.contains(path), "{path}: {message}");
        assert!(
            message.contains("secrets") || message.contains("run"),
            "{message}"
        );
        assert!(
            !origin_has(&f, "issue-41"),
            "{path}: the branch must not have been pushed"
        );
        assert!(github.calls().is_empty(), "{path}: no PR may be opened");
        let after =
            fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json"))
                .unwrap();
        assert_eq!(before, after, "{path}: the task stays ready and unchanged");
    }
}

#[test]
fn only_the_workflow_files_are_named_in_the_refusal() {
    let f = fixture();
    ready_with(&f, &["src/a.rs", ".github/workflows/ci.yml", "README.md"]);

    let message = format!("{:#}", run(&f, &FakeGitHub::default()).unwrap_err());

    assert!(message.contains(".github/workflows/ci.yml"), "{message}");
    assert!(
        !message.contains("src/a.rs") && !message.contains("README.md"),
        "{message}"
    );
}

/// Issue 143, decision 174(e): a spec task can be finished once `ready` (through its sink, not a
/// PR); before that the refusal names the spec form of the review command. The issue 142 version
/// of this test refused it at every stage, since no sink existed yet.
#[test]
fn a_spec_task_is_refused_until_ready_and_may_be_finished_once_ready() {
    let mut record = Record::new(
        &NewTask {
            kind: Kind::Spec,
            number: 0,
            repo: "o/r",
            title: "idea.md",
            base: "main",
            branch: "spec-idea-abc123",
            config_hash: "h",
            id: Some("spec-idea-abc123"),
        },
        0,
        Process::new(1, 0),
    );

    let message = format!("{:#}", check_can_finish(&record).unwrap_err());
    assert!(
        message.contains("spec-idea-abc123") && message.contains("not ready"),
        "{message}"
    );
    assert!(message.contains("task review --spec"), "{message}");
    assert!(!message.contains("--issue"), "{message}");

    for (stage, done) in [
        (Stage::Working, Status::Completed),
        (Stage::Gating, Status::Passed),
        (Stage::Reviewing, Status::Completed),
    ] {
        record.advance(stage, 0, Process::new(1, 0)).unwrap();
        record.finish(done).unwrap();
    }
    record.advance(Stage::Ready, 0, Process::new(1, 0)).unwrap();
    check_can_finish(&record).unwrap();
}

/// The issue path never publishes a spec task as a PR, even a ready one.
#[test]
fn the_pr_path_refuses_a_spec_task() {
    let f = fixture();
    let spec = f.env.tmp.path().join("idea.md");
    fs::write(&spec, "Build a thing.\n").unwrap();
    let b = backend();
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(&f);
    let prepared = sbxm::task::pipeline::prepare_spec(
        &common::task_fixture::ctx(&f, &source, &b, &github),
        &spec,
    )
    .unwrap();

    let result = finish(&f.env.base_dir(), &prepared.record.id, &github, &Probe);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("never a PR"), "{message}");
    assert!(github.calls().is_empty());
}

#[test]
fn other_files_under_dot_github_and_look_alike_paths_are_fine() {
    let f = fixture();
    ready_with(
        &f,
        &[
            ".github/ISSUE_TEMPLATE/bug.md",
            ".github/workflows-notes.md",
            "docs/.github/workflows/example.yml",
        ],
    );
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    assert!(origin_has(&f, "issue-41"));
}
