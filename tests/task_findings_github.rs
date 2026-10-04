//! M2b slice 11, step 4: what `task file-findings` needs of `GitHubBackend` beyond the earlier
//! slices (spec §9): every issue of a repo in any state (to find a finding's hidden marker) and
//! editing an issue's body. `GhBackend`'s argv and `FakeGitHub`'s behavior.

use std::sync::Mutex;

use anyhow::Result;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, Issue, IssueRequest};

const ISSUE_LIST: &str = include_str!("../src/github/fixtures/issue-list.json");

struct Scripted {
    calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
    output: String,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), stdin.map(str::to_owned)));
        Ok(self.output.clone())
    }
}

fn backend(output: &str) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        output: output.to_owned(),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn issues_all_lists_every_state_up_to_the_limit() {
    let (gh, script) = backend(ISSUE_LIST);

    let issues = gh.issues_all("o/r", 1000).unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0].0,
        [
            "issue",
            "list",
            "--repo",
            "o/r",
            "--state",
            "all",
            "--limit",
            "1000",
            "--json",
            "number,title,labels,body,state"
        ]
    );
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        [38, 36, 31]
    );
    assert!(issues[2].body.contains("**Acceptance criteria:**"));
    assert!(issues.iter().all(|i| i.open));
}

#[test]
fn a_closed_issue_in_the_list_is_not_open() {
    let (gh, _) = backend(
        r#"[{"number":5,"state":"CLOSED","title":"t","labels":[],"body":""},{"number":6,"state":"OPEN","title":"u","labels":[],"body":""}]"#,
    );

    let issues = gh.issues_all("o/r", 1000).unwrap();

    assert_eq!(
        issues
            .iter()
            .map(|i| (i.number, i.open))
            .collect::<Vec<_>>(),
        [(5, false), (6, true)]
    );
}

#[test]
fn issue_edit_sends_the_body_on_stdin() {
    let (gh, script) = backend("");

    gh.issue_edit("o/r", 12, "new body\n").unwrap();

    let calls = script.calls.lock().unwrap();
    assert_eq!(
        calls[0].0,
        ["issue", "edit", "12", "--repo", "o/r", "--body-file", "-"]
    );
    assert_eq!(calls[0].1.as_deref(), Some("new body\n"));
}

#[test]
fn issue_labels_adds_and_removes_by_name() {
    let (gh, script) = backend("");

    gh.issue_labels("o/r", 12, &["must-fix".into()], &["should-fix".into()])
        .unwrap();

    assert_eq!(
        script.calls.lock().unwrap()[0].0,
        [
            "issue",
            "edit",
            "12",
            "--repo",
            "o/r",
            "--add-label",
            "must-fix",
            "--remove-label",
            "should-fix"
        ]
    );
}

#[test]
fn the_fake_applies_label_changes() {
    let fake = FakeGitHub::default().with_issues(vec![Issue {
        number: 7,
        open: true,
        title: "old".into(),
        labels: vec!["bug".into(), "should-fix".into()],
        body: "b".into(),
    }]);

    fake.issue_labels("o/r", 7, &["must-fix".into()], &["should-fix".into()])
        .unwrap();

    assert_eq!(
        fake.issues_all("o/r", 10).unwrap()[0].labels,
        ["bug", "must-fix"]
    );
    assert!(fake.issue_labels("o/r", 3, &[], &[]).is_err());
}

fn request(title: &str, body: &str) -> IssueRequest {
    IssueRequest {
        title: title.to_owned(),
        body: body.to_owned(),
        labels: vec!["must-fix".to_owned()],
    }
}

#[test]
fn the_fake_lists_scripted_and_created_issues_and_applies_edits() {
    let fake = FakeGitHub::default()
        .with_next_issue_number(40)
        .with_issues(vec![Issue {
            number: 7,
            open: true,
            title: "old".into(),
            labels: vec![],
            body: "b".into(),
        }]);

    let number = fake
        .issue_create("o/r", &request("S-1: t", "body"))
        .unwrap();
    fake.issue_edit("o/r", 7, "edited").unwrap();
    fake.issue_edit("o/r", number, "edited too").unwrap();

    assert_eq!(number, 40);
    let all = fake.issues_all("o/r", 1000).unwrap();
    assert_eq!(
        all.iter()
            .map(|i| (i.number, i.body.as_str()))
            .collect::<Vec<_>>(),
        [(7, "edited"), (40, "edited too")]
    );
    assert_eq!(all[1].labels, ["must-fix"]);
    assert_eq!(
        fake.calls().last(),
        Some(&GhCall::IssuesAll("o/r".into(), 1000))
    );
}

#[test]
fn the_fake_refuses_to_edit_an_unknown_issue() {
    let fake = FakeGitHub::default();
    assert!(fake.issue_edit("o/r", 3, "x").is_err());
}

#[test]
fn the_fake_can_fail_the_nth_create_and_the_nth_edit_and_the_listing() {
    let fake = FakeGitHub::default()
        .with_next_issue_number(1)
        .failing_issue_create_at(2)
        .failing_issue_edit_at(1);

    assert_eq!(fake.issue_create("o/r", &request("a", "b")).unwrap(), 1);
    assert!(fake.issue_create("o/r", &request("c", "d")).is_err());
    assert_eq!(fake.issue_create("o/r", &request("e", "f")).unwrap(), 2);
    assert!(fake.issue_edit("o/r", 1, "x").is_err());
    fake.issue_edit("o/r", 1, "x").unwrap();
    let titles: Vec<_> = fake
        .issues_all("o/r", 10)
        .unwrap()
        .into_iter()
        .map(|i| i.title)
        .collect();
    assert_eq!(titles, ["a", "e"]);

    let down = FakeGitHub::default().failing_issues_all();
    let error = down.issues_all("o/r", 10).unwrap_err();
    assert!(error.to_string().contains("issue list"));
}
