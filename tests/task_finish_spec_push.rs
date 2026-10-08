//! Issue 144 (decision 174(e), spec section 5.4): `task finish --spec` with `[finish] sink =
//! "push"` pushes the branch to `origin` and opens no PR. It never force-pushes, refuses a branch
//! name already taken on `origin`, refuses unresolved work unless `--push-unresolved`, and a rerun
//! after a push whose record was lost only records it (spec section 5.4a).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{Fixture, Probe, fixture, ready_spec_task, ready_spec_task_with};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Record, Stage, Status, Stopped};

fn repo_root(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("target-repo")
}

fn use_push_sink(f: &Fixture) {
    let toml = repo_root(f).join("sbxm-task.toml");
    let text = fs::read_to_string(&toml).unwrap();
    fs::write(&toml, format!("{text}\n[finish]\nsink = \"push\"\n")).unwrap();
}

fn finish(
    f: &Fixture,
    spec: &Path,
    push_unresolved: bool,
    github: &FakeGitHub,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec.to_path_buf()),
            repo_root: repo_root(f),
            push_unresolved,
        },
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

fn reread(f: &Fixture, id: &str) -> Record {
    record::read(&meta(f, id).join("task.json")).unwrap()
}

/// The commit `branch` is at in the stand-in origin, if it has the branch.
fn origin_head(f: &Fixture, branch: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(&f.origin)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

fn task_tip(f: &Fixture, id: &str) -> String {
    common::git(&meta(f, id).join("repo.git"), &["rev-parse", id])
}

/// Puts `branch` on the stand-in origin at a commit of its own (someone else's branch).
fn someone_elses_branch(f: &Fixture, branch: &str) -> String {
    let scratch = f.env.tmp.path().join("someone");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    fs::write(scratch.join("theirs.txt"), "theirs\n").unwrap();
    common::git(&scratch, &["add", "-A"]);
    common::git(&scratch, &["commit", "-q", "-m", "their work"]);
    common::git(
        &scratch,
        &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
    );
    common::git(&scratch, &["rev-parse", "HEAD"])
}

#[test]
fn a_ready_spec_task_is_pushed_to_origin_and_no_pr_is_opened() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    let github = FakeGitHub::default();

    let (result, out) = finish(&f, &spec, false, &github);

    result.unwrap();
    assert_eq!(origin_head(&f, &id), Some(task_tip(&f, &id)));
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    let record = reread(&f, &id);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr, None);
    assert!(out.contains(&format!("pushed {id} to origin")), "{out}");
    assert!(out.contains("no PR"), "{out}");
    assert!(out.contains(&format!("git fetch origin {id}")), "{out}");
}

#[test]
fn a_branch_name_taken_on_origin_is_refused_and_left_alone() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    let theirs = someone_elses_branch(&f, &id);

    let (result, _) = finish(&f, &spec, false, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("already exists on origin"), "{message}");
    assert_eq!(origin_head(&f, &id), Some(theirs), "never forced");
    let record = reread(&f, &id);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_rerun_after_a_push_whose_record_was_lost_only_records_it() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    // The push landed, then the record write was lost: origin is already at the task's tip.
    let repo_git = meta(&f, &id).join("repo.git");
    common::git(
        &repo_git,
        &[
            "push",
            "-q",
            "origin",
            &format!("refs/heads/{id}:refs/heads/{id}"),
        ],
    );

    let (result, _) = finish(&f, &spec, false, &FakeGitHub::default());

    result.unwrap();
    assert_eq!(origin_head(&f, &id), Some(task_tip(&f, &id)));
    assert_eq!(reread(&f, &id).stage, Stage::Finished);
}

#[test]
fn finishing_twice_is_refused_the_second_time_and_pushes_nothing() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    finish(&f, &spec, false, &FakeGitHub::default()).0.unwrap();
    let task_json = meta(&f, &id).join("task.json");
    let before = fs::read(&task_json).unwrap();

    let (result, _) = finish(&f, &spec, false, &FakeGitHub::default());

    assert!(format!("{:#}", result.unwrap_err()).contains("already finished"));
    assert_eq!(fs::read(&task_json).unwrap(), before);
}

#[test]
fn a_stopped_task_is_refused_unless_push_unresolved() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    let mut record = reread(&f, &id);
    record.stopped = Some(Stopped::RoundsExhausted);
    record::write(&meta(&f, &id), &record).unwrap();

    let (result, _) = finish(&f, &spec, false, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("rounds-exhausted") && message.contains("--push-unresolved"),
        "{message}"
    );
    assert_eq!(origin_head(&f, &id), None);

    finish(&f, &spec, true, &FakeGitHub::default()).0.unwrap();
    assert_eq!(origin_head(&f, &id), Some(task_tip(&f, &id)));
}

#[test]
fn open_must_fix_findings_are_refused_unless_push_unresolved() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    use_push_sink(&f);
    fs::write(
        meta(&f, &id).join("review.md"),
        "Must-fix findings: 2\n\n## Must fix\n",
    )
    .unwrap();

    let (result, _) = finish(&f, &spec, false, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("2 must-fix") && message.contains("--push-unresolved"),
        "{message}"
    );
    assert_eq!(origin_head(&f, &id), None);

    finish(&f, &spec, true, &FakeGitHub::default()).0.unwrap();
    assert_eq!(origin_head(&f, &id), Some(task_tip(&f, &id)));
}

#[test]
fn a_branch_that_changes_a_workflow_is_refused_before_the_push() {
    let f = fixture();
    let (spec, id) = ready_spec_task_with(&f, &["a.txt", ".github/workflows/ci.yml"]);
    use_push_sink(&f);

    let (result, _) = finish(&f, &spec, true, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains(".github/workflows/ci.yml"), "{message}");
    assert_eq!(origin_head(&f, &id), None);
}
