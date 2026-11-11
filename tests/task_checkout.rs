//! Issue 574: a task remembers the checkout it was started in, and a `task` command run where
//! there is no `sbxm-task.toml` says so plainly, naming that checkout when the task's record has
//! one (instead of a later, misleading error).

mod common;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::Env;
use common::task_fixture::{backend, ctx, fixture, issue_text, source};
use sbxm::github::fake::FakeGitHub;
use sbxm::hint::Shell;
use sbxm::task::record::{self, Kind, NewTask, Process};

#[test]
fn a_prepared_task_records_the_checkout_it_was_started_in() {
    let f = fixture();
    let backend = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    let ctx = ctx(&f, &source, &backend, &github);
    let prepared = sbxm::task::pipeline::prepare(&ctx, &issue_text(41)).unwrap();
    let checkout = f.env.tmp.path().join("target-repo");
    assert_eq!(
        prepared.record.checkout.as_deref(),
        Some(checkout.to_str().unwrap())
    );
}

#[test]
fn an_older_record_without_a_checkout_still_reads() {
    let env = Env::new();
    let dir = save_task(&env, None);
    let text = std::fs::read_to_string(dir.join("task.json")).unwrap();
    assert!(!text.contains("checkout"), "{text}");
    assert_eq!(record::read(&dir.join("task.json")).unwrap().checkout, None);
}

/// Saves task issue-41, with `checkout` recorded when given; returns its folder.
fn save_task(env: &Env, checkout: Option<&Path>) -> PathBuf {
    let mut task = record::Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        1_790_000_000,
        Process::new(777_777, 1_790_000_000),
    );
    task.checkout = checkout.map(|p| p.to_string_lossy().into_owned());
    let dir = record::task_dir(&env.base_dir(), &task.id);
    record::write(&dir, &task).unwrap();
    dir
}

/// Runs `sbxm task <args>` in an empty folder of `env`; returns that folder and the stderr.
fn run_elsewhere(env: &Env, args: &[&str]) -> (PathBuf, String) {
    let here = env.tmp.path().join("elsewhere");
    std::fs::create_dir_all(&here).unwrap();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(&here)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .arg("task")
        .args(args)
        .assert()
        .failure()
        .get_output()
        .clone();
    (here, String::from_utf8(output.stderr).unwrap())
}

#[test]
fn a_task_command_with_no_sbxm_task_toml_says_so() {
    let env = Env::new();
    save_task(&env, None);
    let (here, stderr) = run_elsewhere(&env, &["gates", "--issue", "41"]);
    assert!(
        stderr.starts_with(&format!(
            "Error: there is no sbxm-task.toml in {}; run this from the root of the task's repo, \
             or create one with `",
            here.display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains(" task init`\n"), "{stderr}");
}

#[test]
fn the_error_names_the_checkout_the_task_was_started_in() {
    let env = Env::new();
    let checkout = env.tmp.path().join("the repo");
    save_task(&env, Some(&checkout));
    for args in [
        &["gates", "--issue", "41"][..],
        &["resume", "--issue", "41"][..],
        &["review", "--issue", "41"][..],
    ] {
        let (here, stderr) = run_elsewhere(&env, args);
        assert!(
            stderr.starts_with(&format!(
                "Error: there is no sbxm-task.toml in {}; task issue-41 was started in {}: run \
                 this from there\n",
                here.display(),
                checkout.display()
            )),
            "{args:?}: {stderr}"
        );
        // The block says where to run it: the checkout, not the folder it was run in.
        assert!(
            stderr.contains(&format!(
                "  {}\n",
                Shell::HOST.cd(&checkout.to_string_lossy())
            )),
            "{args:?}: {stderr}"
        );
    }
}
