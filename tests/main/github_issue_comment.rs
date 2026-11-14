//! Issue 430: posting a comment on an issue and editing it by the id GitHub gave it, through
//! `gh api` (the bodies on stdin as JSON, never on the command line), and the fake's record.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
    outputs: Mutex<Vec<Result<String, String>>>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), stdin.map(str::to_owned)));
        match self.outputs.lock().unwrap().pop() {
            Some(Ok(out)) => Ok(out),
            Some(Err(message)) => bail!("{message}"),
            None => bail!("no scripted output left"),
        }
    }
}

fn backend(outputs: Vec<Result<String, String>>) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(outputs.into_iter().rev().collect()),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn call(script: &Scripted, n: usize) -> (Vec<String>, Option<String>) {
    script.calls.lock().unwrap()[n].clone()
}

#[test]
fn an_issue_comment_is_posted_through_the_api_and_its_id_returned() {
    // The shape of the REST answer (written by hand: no GitHub access where issue 430 was built).
    let (gh, script) = backend(vec![Ok(
        r#"{"id":2468013579,"html_url":"https://github.com/o/r/issues/12#issuecomment-2468013579","body":"x"}"#
            .to_owned(),
    )]);

    let id = gh.issue_comment("o/r", 12, "hello `x`\n").unwrap();

    assert_eq!(id, 2_468_013_579);
    let (args, stdin) = call(script, 0);
    assert_eq!(
        args,
        [
            "api",
            "repos/o/r/issues/12/comments",
            "--method",
            "POST",
            "--input",
            "-"
        ]
    );
    let sent: serde_json::Value = serde_json::from_str(&stdin.unwrap()).unwrap();
    assert_eq!(sent, serde_json::json!({ "body": "hello `x`\n" }));
}

#[test]
fn an_answer_without_an_id_is_an_error_naming_the_command() {
    let (gh, _) = backend(vec![Ok(r#"{"message":"odd"}"#.to_owned())]);
    let message = format!("{:#}", gh.issue_comment("o/r", 12, "b").unwrap_err());
    assert!(message.contains("gh api"), "{message}");
}

#[test]
fn an_issue_comment_is_edited_by_its_id() {
    let (gh, script) = backend(vec![Ok(r#"{"id":77}"#.to_owned())]);

    gh.issue_comment_edit("o/r", 77, "new body").unwrap();

    let (args, stdin) = call(script, 0);
    assert_eq!(
        args,
        [
            "api",
            "repos/o/r/issues/comments/77",
            "--method",
            "PATCH",
            "--input",
            "-"
        ]
    );
    let sent: serde_json::Value = serde_json::from_str(&stdin.unwrap()).unwrap();
    assert_eq!(sent, serde_json::json!({ "body": "new body" }));
}

#[test]
fn a_failing_edit_is_an_error() {
    let (gh, _) = backend(vec![Err("HTTP 404".into())]);
    let message = format!("{:#}", gh.issue_comment_edit("o/r", 77, "b").unwrap_err());
    assert!(message.contains("HTTP 404"), "{message}");
}

#[test]
fn the_fake_records_posts_and_edits_and_numbers_its_comments() {
    let github = FakeGitHub::default().with_next_comment_id(500);

    assert_eq!(github.issue_comment("o/r", 3, "a").unwrap(), 500);
    assert_eq!(github.issue_comment("o/r", 4, "b").unwrap(), 501);
    github.issue_comment_edit("o/r", 500, "c").unwrap();

    assert_eq!(
        github.calls(),
        [
            GhCall::IssueComment("o/r".into(), 3, "a".into()),
            GhCall::IssueComment("o/r".into(), 4, "b".into()),
            GhCall::IssueCommentEdit("o/r".into(), 500, "c".into()),
        ]
    );
}

#[test]
fn the_fake_fails_the_comments_on_the_issues_it_was_told_to() {
    let github = FakeGitHub::default().failing_issue_comments_on(4);

    assert!(github.issue_comment("o/r", 3, "a").is_ok());
    assert!(github.issue_comment("o/r", 4, "b").is_err());
    assert!(github.issue_comment_edit("o/r", 9, "c").is_ok());
    let edits = FakeGitHub::default().failing_issue_comment_edits();
    assert!(edits.issue_comment_edit("o/r", 9, "c").is_err());
    // Still recorded.
    assert_eq!(github.calls().len(), 3);
    assert_eq!(edits.calls().len(), 1);
}
