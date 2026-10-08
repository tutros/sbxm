//! Issue 143 (decision 174(e), spec section 5.4): `sbxm task rm --spec <FILE>`. A spec task's
//! result under the `local` sink lives only in the task's `repo.git`, which `rm` deletes, so `rm`
//! refuses it until the branch is fetched into the checkout, or `--force` is given.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    Fixture, Probe, backend, ctx, fixture, ready_spec_task, source, spec_file,
};
use sbxm::commands::task_finish;
use sbxm::commands::task_rm::{Options, Target, run};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::{pipeline, record};

struct Yes(RefCell<Vec<String>>);

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.0.borrow_mut().push(prompt.to_owned());
        Ok(true)
    }
}

fn target_repo(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("target-repo")
}

/// A ready spec task finished through the `local` sink.
fn kept_spec_task(f: &Fixture) -> (PathBuf, String) {
    let (spec, id) = ready_spec_task(f);
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(spec.clone()),
            repo_root: target_repo(f),
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    (spec, id)
}

fn rm(
    f: &Fixture,
    spec: &Path,
    force: bool,
    repo_root: &Path,
) -> (anyhow::Result<()>, String, usize) {
    let confirm = Yes(RefCell::new(Vec::new()));
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Spec(spec.to_path_buf()),
            yes: true,
            force,
            repo_root: repo_root.to_path_buf(),
        },
        &backend(),
        &Probe,
        &confirm,
        &mut out,
    );
    let asked = confirm.0.borrow().len();
    (result, String::from_utf8(out).unwrap(), asked)
}

fn task_dir(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

#[test]
fn an_unfetched_result_is_refused_naming_the_fetch_command_and_force_and_nothing_goes() {
    let f = fixture();
    let (spec, id) = kept_spec_task(&f);

    let (result, out, asked) = rm(&f, &spec, false, &target_repo(&f));

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("git fetch"), "{message}");
    assert!(message.contains(&format!("{id}:{id}")), "{message}");
    assert!(message.contains("--force"), "{message}");
    assert!(task_dir(&f, &id).join("repo.git").is_dir());
    assert!(out.is_empty(), "{out}");
    assert_eq!(asked, 0, "refused before the question");
}

#[test]
fn force_deletes_an_unfetched_result() {
    let f = fixture();
    let (spec, id) = kept_spec_task(&f);

    let (result, _, _) = rm(&f, &spec, true, &target_repo(&f));

    result.unwrap();
    assert!(!task_dir(&f, &id).exists());
}

#[test]
fn a_result_fetched_into_the_checkout_is_removed_without_force() {
    let f = fixture();
    let (spec, id) = kept_spec_task(&f);
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
    let repo_git = task_dir(&f, &id).join("repo.git");
    common::git(
        &checkout,
        &[
            "fetch",
            "-q",
            repo_git.to_str().unwrap(),
            &format!("{id}:{id}"),
        ],
    );

    let (result, _, _) = rm(&f, &spec, false, &checkout);

    result.unwrap();
    assert!(!task_dir(&f, &id).exists());
}

#[test]
fn a_ready_but_unfinished_result_is_protected_too() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);

    let (result, _, _) = rm(&f, &spec, false, &target_repo(&f));

    assert!(format!("{:#}", result.unwrap_err()).contains("--force"));
    assert!(task_dir(&f, &id).join("repo.git").is_dir());
}

#[test]
fn a_spec_task_without_commits_is_removed_without_force() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let b = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = pipeline::prepare_spec(&ctx(&f, &source, &b, &github), &spec).unwrap();
    // Its start failed before the worker committed anything.
    prepared.record.finish(record::Status::Failed).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();

    let (result, _, _) = rm(&f, &spec, false, &target_repo(&f));

    result.unwrap();
    assert!(!task_dir(&f, &prepared.record.id).exists());
}

#[test]
fn no_task_for_the_file_says_so() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");

    let (result, _, _) = rm(&f, &spec, false, &target_repo(&f));

    assert!(format!("{:#}", result.unwrap_err()).contains("no task"));
}
