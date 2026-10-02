//! M2b slice 2: `GhBackend` against output captured from the real `gh`
//! (version 2.83.2, 2026-10-01, `tutros/sbxm`), and the argv it builds (spec §9).

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, IssueRequest, PrRequest, PrState};

const ISSUE_LIST: &str = include_str!("../src/github/fixtures/issue-list.json");
const ISSUE_VIEW: &str = include_str!("../src/github/fixtures/issue-view-31.txt");
const PR_CLOSING: &str = include_str!("../src/github/fixtures/pr-view-closing.json");
const PR_MERGED: &str = include_str!("../src/github/fixtures/pr-view-merged.json");
const DEFAULT_BRANCH: &str = include_str!("../src/github/fixtures/repo-default-branch.json");
const LABELS: &str = include_str!("../src/github/fixtures/label-list.json");

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
    outputs: Mutex<Vec<Result<String, String>>>,
}

impl Scripted {
    fn answering(output: &str) -> Self {
        Self::with(vec![Ok(output.to_owned())])
    }

    fn with(outputs: Vec<Result<String, String>>) -> Self {
        Self {
            calls: Mutex::default(),
            outputs: Mutex::new(outputs.into_iter().rev().collect()),
        }
    }
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

/// A backend over a leaked script (tests are short-lived), plus the script to inspect.
fn backend(script: Scripted) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(script));
    (GhBackend::with_runner(Box::new(script)), script)
}

fn args(script: &Scripted, call: usize) -> Vec<String> {
    script.calls.lock().unwrap()[call].0.clone()
}

#[test]
fn open_issues_are_parsed_with_label_names() {
    let (gh, script) = backend(Scripted::answering(ISSUE_LIST));

    let issues = gh.issues_open("tutros/sbxm").unwrap();

    assert_eq!(
        args(script, 0),
        [
            "issue",
            "list",
            "--repo",
            "tutros/sbxm",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title,labels,body"
        ]
    );
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        [38, 36, 31]
    );
    assert_eq!(issues[0].labels, ["should-fix"]);
    assert!(issues[2].body.contains("**Acceptance criteria:**"));
    assert_eq!(
        issues[2].title,
        "M2 plan: tests for run-id validation and run-root collision rollback (P2)"
    );
}

#[test]
fn an_issue_view_keeps_the_text_and_reads_title_and_state_from_its_header() {
    let (gh, script) = backend(Scripted::answering(ISSUE_VIEW));

    let issue = gh.issue("tutros/sbxm", 31).unwrap();

    assert_eq!(
        args(script, 0),
        ["issue", "view", "31", "--repo", "tutros/sbxm"]
    );
    assert_eq!(issue.number, 31);
    assert_eq!(
        issue.title,
        "M2 plan: tests for run-id validation and run-root collision rollback (P2)"
    );
    assert_eq!(issue.state, "OPEN");
    assert_eq!(issue.text, ISSUE_VIEW);
}

#[test]
fn an_issue_view_without_a_header_is_an_error() {
    let (gh, _) = backend(Scripted::answering("not what gh prints"));
    let message = format!("{:#}", gh.issue("o/r", 1).unwrap_err());
    assert!(message.contains("gh issue view 1"), "{message}");
}

#[test]
fn a_pr_with_closing_issues_is_parsed() {
    let (gh, script) = backend(Scripted::answering(PR_CLOSING));

    let pr = gh.pr("tutros/sbxm", 34).unwrap();

    assert_eq!(
        args(script, 0),
        [
            "pr",
            "view",
            "34",
            "--repo",
            "tutros/sbxm",
            "--json",
            "headRefName,isCrossRepository,state,title,body,closingIssuesReferences"
        ]
    );
    assert_eq!(pr.number, 34);
    assert_eq!(pr.head_ref, "fix-33-mandatory-harness");
    assert!(!pr.is_cross_repository);
    assert_eq!(pr.state, PrState::Merged);
    assert_eq!(pr.title, "Require --harness on open, stop and rm");
    assert!(pr.body.starts_with("Fixes #33"));
    assert_eq!(pr.closing_issues, [33]);
}

#[test]
fn another_merged_pr_parses_too() {
    let (gh, _) = backend(Scripted::answering(PR_MERGED));
    let pr = gh.pr("tutros/sbxm", 54).unwrap();
    assert_eq!(pr.state, PrState::Merged);
    assert!(
        pr.title.contains("2a") || pr.title.contains("comparisons"),
        "{}",
        pr.title
    );
}

#[test]
fn an_unknown_pr_state_is_an_error() {
    let (gh, _) = backend(Scripted::answering(
        r#"{"headRefName":"b","isCrossRepository":false,"state":"WEIRD","title":"t","body":"","closingIssuesReferences":[]}"#,
    ));
    let message = format!("{:#}", gh.pr("o/r", 2).unwrap_err());
    assert!(message.contains("WEIRD"), "{message}");
}

#[test]
fn the_default_branch_labels_and_login_are_parsed() {
    let (gh, script) = backend(Scripted::with(vec![
        Ok(DEFAULT_BRANCH.to_owned()),
        Ok(LABELS.to_owned()),
        Ok("tutros\n".to_owned()),
    ]));

    assert_eq!(gh.default_branch("tutros/sbxm").unwrap(), "main");
    let labels = gh.labels("tutros/sbxm").unwrap();
    assert!(labels.contains(&"must-fix".to_owned()) && labels.contains(&"question".to_owned()));
    assert_eq!(gh.whoami().unwrap(), "tutros");

    assert_eq!(
        args(script, 0),
        ["repo", "view", "tutros/sbxm", "--json", "defaultBranchRef"]
    );
    assert_eq!(
        args(script, 1),
        [
            "label",
            "list",
            "--repo",
            "tutros/sbxm",
            "--json",
            "name",
            "--limit",
            "200"
        ]
    );
    assert_eq!(args(script, 2), ["api", "user", "--jq", ".login"]);
}

#[test]
fn a_comment_goes_through_stdin_not_the_command_line() {
    let (gh, script) = backend(Scripted::answering(""));

    gh.pr_comment("o/r", 7, "line one\nline \"two\"").unwrap();

    assert_eq!(
        args(script, 0),
        ["pr", "comment", "7", "--repo", "o/r", "--body-file", "-"]
    );
    let stdin = script.calls.lock().unwrap()[0].1.clone();
    assert_eq!(stdin.as_deref(), Some("line one\nline \"two\""));
}

#[test]
fn a_pr_is_created_from_the_head_branch_and_returns_the_url() {
    let (gh, script) = backend(Scripted::answering("https://github.com/o/r/pull/9\n"));

    let url = gh
        .pr_create(
            "o/r",
            &PrRequest {
                head: "issue-4".into(),
                base: "main".into(),
                title: "Fix it".into(),
                body: "Fixes #4".into(),
            },
        )
        .unwrap();

    assert_eq!(url, "https://github.com/o/r/pull/9");
    assert_eq!(
        args(script, 0),
        [
            "pr",
            "create",
            "--repo",
            "o/r",
            "--head",
            "issue-4",
            "--base",
            "main",
            "--title",
            "Fix it",
            "--body-file",
            "-"
        ]
    );
    let stdin = script.calls.lock().unwrap()[0].1.clone();
    assert_eq!(stdin.as_deref(), Some("Fixes #4"));
}

#[test]
fn an_issue_is_created_with_its_labels_and_returns_its_number() {
    let (gh, script) = backend(Scripted::answering("https://github.com/o/r/issues/58\n"));

    let number = gh
        .issue_create(
            "o/r",
            &IssueRequest {
                title: "F1: x".into(),
                body: "body".into(),
                labels: vec!["must-fix".into(), "bug".into()],
            },
        )
        .unwrap();

    assert_eq!(number, 58);
    assert_eq!(
        args(script, 0),
        [
            "issue",
            "create",
            "--repo",
            "o/r",
            "--title",
            "F1: x",
            "--label",
            "must-fix",
            "--label",
            "bug",
            "--body-file",
            "-"
        ]
    );
}

#[test]
fn an_issue_url_that_gh_prints_oddly_is_an_error() {
    let (gh, _) = backend(Scripted::answering("created!\n"));
    let request = IssueRequest {
        title: "t".into(),
        body: "b".into(),
        labels: vec![],
    };
    let message = format!("{:#}", gh.issue_create("o/r", &request).unwrap_err());
    assert!(message.contains("issue number"), "{message}");
}

#[test]
fn a_failing_gh_is_reported_with_the_command_and_a_fix() {
    let (gh, _) = backend(Scripted::with(vec![Err("boom".into())]));
    let message = format!("{:#}", gh.issues_open("o/r").unwrap_err());
    assert!(message.contains("boom"), "{message}");
}

#[test]
fn bad_json_names_the_command() {
    let (gh, _) = backend(Scripted::answering("[{"));
    let message = format!("{:#}", gh.issues_open("o/r").unwrap_err());
    assert!(message.contains("gh issue list"), "{message}");
}
