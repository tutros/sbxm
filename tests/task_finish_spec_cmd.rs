//! `sbxm task finish --spec <FILE>` as a command. Issue 142 refused it outright; issue 143
//! (decision 174(e)) adds the `local` sink: the branch stays in the task's host-owned `repo.git`
//! and `finish` prints how to fetch it, opening no PR and pushing nothing.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    Fixture, Probe, backend, ctx, fixture, ready_spec_task, source, spec_file,
};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;
use sbxm::task::record::{self, Record, Stage, Status};

fn repo_root(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("target-repo")
}

fn finish(f: &Fixture, spec: &Path, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec.to_path_buf()),
            repo_root: repo_root(f),
        },
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn reread(f: &Fixture, id: &str) -> Record {
    record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap()
}

fn origin_branches(f: &Fixture) -> String {
    common::git(&f.origin, &["branch", "--list"])
}

#[test]
fn a_ready_spec_task_with_the_local_sink_is_finished_and_finish_says_how_to_fetch_it() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    let github = FakeGitHub::default();

    let (result, out) = finish(&f, &spec, &github);

    result.unwrap();
    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    assert!(out.contains("git fetch"), "{out}");
    assert!(out.contains(&repo_git.display().to_string()), "{out}");
    assert!(out.contains(&format!("{id}:{id}")), "{out}");
    let record = reread(&f, &id);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr, None);
    // No PR, no push.
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert!(
        !origin_branches(&f).contains(&id),
        "{}",
        origin_branches(&f)
    );
}

#[test]
fn the_fetch_command_it_prints_works() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    let (result, _) = finish(&f, &spec, &FakeGitHub::default());
    result.unwrap();

    let repo_git = record::task_dir(&f.env.base_dir(), &id).join("repo.git");
    let checkout = f.env.tmp.path().join("checkout");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    common::git(
        &checkout,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );
    assert_eq!(
        common::git(&checkout, &["rev-parse", &id]),
        common::git(&repo_git, &["rev-parse", &id])
    );
}

#[test]
fn finishing_twice_is_refused_the_second_time_and_changes_nothing() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    finish(&f, &spec, &FakeGitHub::default()).0.unwrap();
    let task_json = record::task_dir(&f.env.base_dir(), &id).join("task.json");
    let before = fs::read(&task_json).unwrap();

    let (result, _) = finish(&f, &spec, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("already finished"), "{message}");
    assert_eq!(fs::read(&task_json).unwrap(), before);
}

#[test]
fn the_push_sink_is_accepted_by_the_config_but_finish_says_it_is_not_available_yet() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    let toml = repo_root(&f).join("sbxm-task.toml");
    let text = fs::read_to_string(&toml).unwrap();
    fs::write(&toml, format!("{text}\n[finish]\nsink = \"push\"\n")).unwrap();
    let github = FakeGitHub::default();

    let (result, _) = finish(&f, &spec, &github);

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("push") && message.contains("not available yet"),
        "{message}"
    );
    let record = reread(&f, &id);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(github.calls().is_empty());
    assert!(!origin_branches(&f).contains(&id));
}

#[test]
fn a_spec_task_that_is_not_ready_is_refused_naming_the_review_command() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let b = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    pipeline::prepare_spec(&ctx(&f, &source, &b, &github), &spec).unwrap();

    let (result, _) = finish(&f, &spec, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("spec-idea-") && message.contains("not ready"),
        "{message}"
    );
    assert!(message.contains("task review --spec"), "{message}");
}

#[test]
fn finish_names_the_missing_task_when_no_task_was_started() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");

    let (result, _) = finish(&f, &spec, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("no task"), "{message}");
}

#[test]
fn a_ready_spec_task_without_commits_is_refused_and_stays_ready() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let b = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = pipeline::prepare_spec(&ctx(&f, &source, &b, &github), &spec).unwrap();
    // Walked to `ready` by hand: the worker committed nothing.
    let r = &mut prepared.record;
    let p = || record::Process::new(1, 0);
    for (stage, done) in [
        (Stage::Working, Status::Completed),
        (Stage::Gating, Status::Passed),
        (Stage::Reviewing, Status::Completed),
    ] {
        r.advance(stage, 0, p()).unwrap();
        r.finish(done).unwrap();
    }
    r.advance(Stage::Ready, 0, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();

    let (result, _) = finish(&f, &spec, &FakeGitHub::default());

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("nothing to keep"), "{message}");
    let record = reread(&f, &prepared.record.id);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}
