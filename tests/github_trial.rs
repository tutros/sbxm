//! Issue 550: what the medium-risk trial reads of GitHub through `GhBackend`: an issue's comments
//! (`gh api repos/<repo>/issues/<n>/comments --paginate`), a PR's label history (`gh api
//! repos/<repo>/issues/<n>/events --paginate`) and when a PR merged (`gh api
//! repos/<repo>/pulls/<n>`).

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, LabelEvent};

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

// --- An issue's comments ---

#[test]
fn an_issues_comments_are_read_from_every_page_oldest_first() {
    let (gh, script) = backend(
        r#"[{"id": 1, "body": "first", "user": {"login": "a"}}][{"id": 2, "body": "second"}]"#,
        None,
    );

    let bodies = gh.issue_comment_bodies("o/r", 568).unwrap();

    assert_eq!(bodies, ["first", "second"]);
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&[
            "api",
            "repos/o/r/issues/568/comments",
            "--paginate"
        ])]
    );
}

#[test]
fn comments_that_cant_be_read_are_an_error() {
    let (gh, _) = backend("", Some("HTTP 502"));
    assert!(gh.issue_comment_bodies("o/r", 568).is_err());

    let (gh, _) = backend("not json", None);
    assert!(gh.issue_comment_bodies("o/r", 568).is_err());
}

// --- A PR's label history ---

#[test]
fn a_prs_label_events_are_read_with_their_times_and_other_events_left_out() {
    let (gh, script) = backend(
        r#"[{"event": "labeled", "label": {"name": "hold"}, "created_at": "2026-11-10T19:40:00Z"},
{"event": "commented", "created_at": "2026-11-10T19:41:00Z"},
{"event": "unlabeled", "label": {"name": "hold"}, "created_at": "2026-11-10T19:42:00Z"}]
[{"event": "labeled", "label": {"name": "go"}, "created_at": "2026-11-10T19:43:00Z"}]"#,
        None,
    );

    let events = gh.label_events("o/r", 120).unwrap();

    let at = 1_794_339_600;
    assert_eq!(
        events,
        [
            LabelEvent {
                label: "hold".into(),
                added: true,
                at,
            },
            LabelEvent {
                label: "hold".into(),
                added: false,
                at: at + 120,
            },
            LabelEvent {
                label: "go".into(),
                added: true,
                at: at + 180,
            },
        ]
    );
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&["api", "repos/o/r/issues/120/events", "--paginate"])]
    );
}

#[test]
fn a_label_event_whose_time_cant_be_read_is_an_error() {
    let (gh, _) = backend(
        r#"[{"event": "labeled", "label": {"name": "hold"}, "created_at": "soon"}]"#,
        None,
    );
    let e = gh.label_events("o/r", 120).unwrap_err();
    assert!(format!("{e:#}").contains("soon"), "{e:#}");
}

// --- When a PR merged ---

#[test]
fn a_merged_prs_time_is_read() {
    let (gh, script) = backend(
        r#"{"number": 120, "merged_at": "2026-11-10T19:40:00Z"}"#,
        None,
    );

    assert_eq!(gh.pr_merged_at("o/r", 120).unwrap(), Some(1_794_339_600));
    assert_eq!(
        *script.calls.lock().unwrap(),
        [args(&["api", "repos/o/r/pulls/120"])]
    );
}

#[test]
fn an_unmerged_pr_has_no_merge_time() {
    let (gh, _) = backend(r#"{"number": 120, "merged_at": null}"#, None);
    assert_eq!(gh.pr_merged_at("o/r", 120).unwrap(), None);
}

#[test]
fn a_merge_time_that_cant_be_read_is_an_error() {
    let (gh, _) = backend(r#"{"number": 120, "merged_at": "later"}"#, None);
    assert!(gh.pr_merged_at("o/r", 120).is_err());
}
