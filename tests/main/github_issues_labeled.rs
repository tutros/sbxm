//! Review M-1 of issue 463: the open issues with one label, listed by `gh` with the label as a
//! filter (never picked out of the newest 200 open issues), and an error rather than a guess when
//! the listing may have been cut off at its limit.

use std::sync::Mutex;

use anyhow::{Result, bail};
use sbxm::github::GitHubBackend;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner, LABELED_LIMIT};
use sbxm::github::{Issue, IssueRequest};

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    outputs: Mutex<Vec<String>>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match self.outputs.lock().unwrap().pop() {
            Some(out) => Ok(out),
            None => bail!("no scripted output left"),
        }
    }
}

fn backend(output: String) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(vec![output]),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

/// `gh issue list --json number,title,labels,body,state` output for `numbers`, each labeled
/// `loop-log`.
fn listed(numbers: impl Iterator<Item = u32>) -> String {
    let items: Vec<String> = numbers
        .map(|n| {
            format!(
                r#"{{"number":{n},"title":"t{n}","labels":[{{"name":"loop-log"}}],"body":"","state":"OPEN"}}"#
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

#[test]
fn the_label_is_a_filter_of_the_listing_itself() {
    let (gh, script) = backend(listed([52, 41].into_iter()));

    let issues = gh.issues_open_labeled("o/r", "loop-log").unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0],
        [
            "issue",
            "list",
            "--repo",
            "o/r",
            "--state",
            "open",
            "--label",
            "loop-log",
            "--limit",
            &LABELED_LIMIT.to_string(),
            "--json",
            "number,title,labels,body,state"
        ]
    );
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        [52, 41]
    );
    assert!(issues.iter().all(|i| i.open));
}

#[test]
fn one_under_the_limit_is_the_whole_list() {
    let (gh, _) = backend(listed(1..LABELED_LIMIT));

    let issues = gh.issues_open_labeled("o/r", "loop-log").unwrap();

    assert_eq!(issues.len(), LABELED_LIMIT as usize - 1);
}

#[test]
fn a_listing_that_reaches_the_limit_is_an_error_not_a_guess() {
    let (gh, _) = backend(listed(1..=LABELED_LIMIT));

    let err = format!(
        "{:#}",
        gh.issues_open_labeled("o/r", "loop-log").unwrap_err()
    );

    assert!(err.contains(&LABELED_LIMIT.to_string()), "{err}");
    assert!(err.contains("loop-log"), "{err}");
}

// --- Issue 594, review M-4: the issues with one label in every state ---

#[test]
fn the_every_state_listing_filters_by_the_label_with_state_all() {
    let (gh, script) = backend(listed([52, 41].into_iter()));

    let issues = gh.issues_labeled("o/r", "loop-log").unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0],
        [
            "issue",
            "list",
            "--repo",
            "o/r",
            "--state",
            "all",
            "--label",
            "loop-log",
            "--limit",
            &LABELED_LIMIT.to_string(),
            "--json",
            "number,title,labels,body,state"
        ]
    );
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        [52, 41]
    );
}

#[test]
fn an_every_state_listing_one_under_the_limit_is_the_whole_list() {
    let (gh, _) = backend(listed(1..LABELED_LIMIT));

    let issues = gh.issues_labeled("o/r", "loop-log").unwrap();

    assert_eq!(issues.len(), LABELED_LIMIT as usize - 1);
}

#[test]
fn an_every_state_listing_that_reaches_the_limit_is_an_error_not_a_guess() {
    let (gh, _) = backend(listed(1..=LABELED_LIMIT));

    let err = format!("{:#}", gh.issues_labeled("o/r", "loop-log").unwrap_err());

    assert!(err.contains(&LABELED_LIMIT.to_string()), "{err}");
    assert!(err.contains("loop-log"), "{err}");
}

#[test]
fn the_fake_lists_the_open_and_closed_issues_with_the_label() {
    let issue = |number: u32, open: bool, label: &str| Issue {
        number,
        open,
        title: format!("t{number}"),
        labels: vec![label.into()],
        body: String::new(),
    };
    let gh = FakeGitHub::default()
        .with_open_issues(vec![issue(1, true, "loop-log"), issue(2, true, "bug")])
        .with_issues(vec![issue(3, false, "loop-log"), issue(4, false, "bug")]);

    let issues = gh.issues_labeled("o/r", "loop-log").unwrap();

    let numbers: Vec<u32> = issues.iter().map(|i| i.number).collect();
    assert_eq!(numbers, [1, 3]);
    assert!(
        gh.calls()
            .contains(&GhCall::IssuesLabeled("o/r".into(), "loop-log".into()))
    );
}

#[test]
fn the_fake_lists_only_the_open_issues_with_the_label_however_many_there_are() {
    let mut open: Vec<Issue> = (1..=300)
        .map(|n| Issue {
            number: n,
            open: true,
            title: format!("t{n}"),
            labels: vec!["bug".into()],
            body: String::new(),
        })
        .collect();
    open[0].labels.push("loop-log".into());
    let gh = FakeGitHub::default().with_open_issues(open);
    let created = gh
        .issue_create(
            "o/r",
            &IssueRequest {
                title: "x".into(),
                body: String::new(),
                labels: vec!["loop-log".into()],
            },
        )
        .unwrap();

    let issues = gh.issues_open_labeled("o/r", "loop-log").unwrap();

    let numbers: Vec<u32> = issues.iter().map(|i| i.number).collect();
    assert!(numbers.contains(&1), "{numbers:?}");
    assert!(numbers.contains(&created), "{numbers:?}");
    assert_eq!(numbers.len(), 2);
    assert!(
        gh.calls()
            .contains(&GhCall::IssuesOpenLabeled("o/r".into(), "loop-log".into()))
    );
}
