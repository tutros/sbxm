//! Issue 375: `GhBackend::pr_arm_auto_merge` builds `gh pr merge --auto --merge
//! --match-head-commit` and refuses anything but a full commit id before calling `gh`.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::gh::{GhBackend, GhRunner};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    failure: Option<&'static str>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match self.failure {
            Some(message) => bail!("{message}"),
            None => Ok(String::new()),
        }
    }
}

fn backend(failure: Option<&'static str>) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        failure,
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn auto_merge_is_armed_with_the_reviewed_commit() {
    let (gh, script) = backend(None);

    gh.pr_arm_auto_merge("o/r", 12, SHA).unwrap();

    assert_eq!(
        script.calls.lock().unwrap().as_slice(),
        [[
            "pr",
            "merge",
            "12",
            "--repo",
            "o/r",
            "--auto",
            "--merge",
            "--match-head-commit",
            SHA
        ]]
    );
}

#[test]
fn anything_but_a_full_commit_id_is_refused_without_calling_gh() {
    let (gh, script) = backend(None);

    for sha in ["abc1234", "--admin", &SHA.replace('0', "g"), ""] {
        let message = format!("{:#}", gh.pr_arm_auto_merge("o/r", 12, sha).unwrap_err());
        assert!(message.contains("not a full commit id"), "{message}");
    }

    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn a_gh_failure_is_an_error_naming_the_command() {
    let (gh, _) = backend(Some("Pull request is in clean status"));

    let message = format!("{:#}", gh.pr_arm_auto_merge("o/r", 12, SHA).unwrap_err());

    assert!(message.contains("gh pr merge"), "{message}");
    assert!(message.contains("clean status"), "{message}");
}
