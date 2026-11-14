//! Issue 548: what the medium-risk veto window reads of GitHub through `GhBackend`: a PR's labels
//! (`gh api repos/<repo>/issues/<n>/labels --paginate`), the removal of one
//! (`gh api --method DELETE repos/<repo>/issues/<n>/labels/<name>`), and the statuses posted on a
//! commit with their times (`gh api repos/<repo>/commits/<sha>/statuses --paginate`).

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, PostedStatus};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    /// What every call prints.
    out: String,
    failure: Option<&'static str>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        if let Some(message) = self.failure {
            bail!("{message}");
        }
        Ok(self.out.clone())
    }
}

fn backend(out: &str, failure: Option<&'static str>) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        out: out.to_owned(),
        failure,
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}

// --- A PR's labels ---

#[test]
fn a_prs_labels_are_read_from_every_page() {
    let (gh, script) = backend(
        r#"[{"id": 1, "name": "size:M"}, {"id": 2, "name": "Hold"}][{"id": 3, "name": "go"}]"#,
        None,
    );

    let labels = gh.pr_labels("o/r", 12).unwrap();

    assert_eq!(labels, ["size:M", "Hold", "go"]);
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&["api", "repos/o/r/issues/12/labels", "--paginate"])]
    );
}

#[test]
fn a_pr_without_labels_has_none() {
    let (gh, _) = backend("[]", None);

    assert!(gh.pr_labels("o/r", 12).unwrap().is_empty());
}

#[test]
fn labels_that_cant_be_read_are_an_error() {
    let (gh, _) = backend("", Some("HTTP 502"));

    let e = gh.pr_labels("o/r", 12).unwrap_err();

    assert!(format!("{e:#}").contains("HTTP 502"), "{e:#}");
}

// --- Removing a label ---

#[test]
fn a_label_is_removed_by_its_own_name() {
    let (gh, script) = backend("[]", None);

    gh.pr_remove_label("o/r", 12, "GO").unwrap();

    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&[
            "api",
            "repos/o/r/issues/12/labels/GO",
            "--method",
            "DELETE",
            "--silent",
        ])]
    );
}

#[test]
fn a_label_name_is_percent_encoded_in_the_path() {
    let (gh, script) = backend("[]", None);

    gh.pr_remove_label("o/r", 12, "size:M a/b").unwrap();

    let calls = script.calls.lock().unwrap();
    assert_eq!(calls[0][1], "repos/o/r/issues/12/labels/size%3AM%20a%2Fb");
}

#[test]
fn a_removal_that_fails_is_an_error() {
    let (gh, _) = backend("", Some("HTTP 403"));

    assert!(gh.pr_remove_label("o/r", 12, "go").is_err());
}

// --- A commit's statuses ---

#[test]
fn a_commits_statuses_are_read_with_their_times_from_every_page() {
    let (gh, script) = backend(
        r#"[{"state": "failure", "context": "sbxm/review", "created_at": "2026-11-10T19:40:00Z", "description": "x"}]
[{"state": "success", "context": "ci", "created_at": "2026-11-10T17:00:05Z"}]"#,
        None,
    );

    let statuses = gh.commit_statuses("o/r", SHA).unwrap();

    assert_eq!(
        statuses,
        [
            PostedStatus {
                context: "sbxm/review".into(),
                state: "failure".into(),
                at: 1_794_339_600,
            },
            PostedStatus {
                context: "ci".into(),
                state: "success".into(),
                at: 1_794_330_005,
            },
        ]
    );
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&[
            "api",
            &format!("repos/o/r/commits/{SHA}/statuses"),
            "--paginate",
        ])]
    );
}

#[test]
fn a_status_whose_time_cant_be_read_is_an_error() {
    let (gh, _) = backend(
        r#"[{"state": "success", "context": "sbxm/review", "created_at": "yesterday"}]"#,
        None,
    );

    let e = gh.commit_statuses("o/r", SHA).unwrap_err();

    assert!(format!("{e:#}").contains("yesterday"), "{e:#}");
}

#[test]
fn statuses_are_only_read_for_a_full_commit_id() {
    let (gh, script) = backend("[]", None);

    assert!(gh.commit_statuses("o/r", "../x").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}
