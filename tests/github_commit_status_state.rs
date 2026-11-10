//! Issue 547: `GhBackend::commit_status_state` reads a commit's statuses (`gh api
//! repos/<repo>/commits/<sha>/statuses --paginate`, newest first) and answers the state of the
//! latest one of a context, for the loop's real-check arming check.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::gh::{GhBackend, GhRunner};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    /// What the statuses call prints.
    statuses: String,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match args.get(1).map(String::as_str) {
            Some(path) if path.starts_with(&format!("repos/o/r/commits/{SHA}/statuses")) => {
                Ok(self.statuses.clone())
            }
            _ => bail!("unexpected gh call {args:?}"),
        }
    }
}

fn backend(statuses: &str) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        statuses: statuses.to_owned(),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn status(state: &str, context: &str) -> String {
    format!(r#"{{"state": "{state}", "context": "{context}", "description": "d", "id": 1}}"#)
}

#[test]
fn the_latest_status_of_the_context_is_read() {
    // Newest first: success after failure.
    let page = format!(
        "[{}, {}, {}]",
        status("success", "ci"),
        status("success", "sbxm/real-check"),
        status("failure", "sbxm/real-check"),
    );
    let (gh, script) = backend(&page);
    assert_eq!(
        gh.commit_status_state("o/r", SHA, "sbxm/real-check")
            .unwrap()
            .as_deref(),
        Some("success")
    );
    let calls = script.calls.lock().unwrap();
    assert!(calls[0].contains(&"--paginate".to_owned()), "{calls:?}");
}

#[test]
fn a_later_page_is_read_too() {
    let text = format!(
        "[{}][{}]",
        status("success", "ci"),
        status("failure", "sbxm/real-check")
    );
    let (gh, _) = backend(&text);
    assert_eq!(
        gh.commit_status_state("o/r", SHA, "sbxm/real-check")
            .unwrap()
            .as_deref(),
        Some("failure")
    );
}

#[test]
fn no_status_of_the_context_is_none() {
    let (gh, _) = backend(&format!("[{}]", status("success", "sbxm/review")));
    assert_eq!(
        gh.commit_status_state("o/r", SHA, "sbxm/real-check")
            .unwrap(),
        None
    );
    let (gh, _) = backend("[]");
    assert_eq!(
        gh.commit_status_state("o/r", SHA, "sbxm/real-check")
            .unwrap(),
        None
    );
}

#[test]
fn a_sha_that_isnt_a_full_commit_id_is_refused_without_a_call() {
    let (gh, script) = backend("[]");
    assert!(gh.commit_status_state("o/r", "0123456", "x").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn output_that_isnt_json_is_an_error() {
    let (gh, _) = backend("oops");
    assert!(gh.commit_status_state("o/r", SHA, "x").is_err());
}
