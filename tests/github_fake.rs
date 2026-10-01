//! M2b slice 2: `FakeGitHub` records calls and is scripted per test (spec §9).

use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{GitHubBackend, Issue, IssueRequest, IssueText, PrInfo, PrRequest, PrState};

fn issue(number: u32) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        labels: vec!["bug".into()],
        body: String::new(),
    }
}

fn pr_info() -> PrInfo {
    PrInfo {
        number: 5,
        head_ref: "feature".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "t".into(),
        body: "b".into(),
        closing_issues: vec![4],
    }
}

#[test]
fn scripted_answers_come_back_and_every_call_is_recorded_in_order() {
    let fake = FakeGitHub::default()
        .with_user("tutros")
        .with_default_branch("main")
        .with_open_issues(vec![issue(1), issue(2)])
        .with_issue_text(IssueText {
            number: 1,
            title: "issue 1".into(),
            state: "OPEN".into(),
            text: "text".into(),
        })
        .with_pr(pr_info())
        .with_labels(&["bug", "must-fix"]);

    assert_eq!(fake.whoami().unwrap(), "tutros");
    assert_eq!(fake.default_branch("o/r").unwrap(), "main");
    assert_eq!(fake.issues_open("o/r").unwrap().len(), 2);
    assert_eq!(fake.issue("o/r", 1).unwrap().text, "text");
    assert_eq!(fake.pr("o/r", 5).unwrap().head_ref, "feature");
    assert_eq!(fake.labels("o/r").unwrap(), ["bug", "must-fix"]);

    assert_eq!(
        fake.calls(),
        [
            GhCall::Whoami,
            GhCall::DefaultBranch("o/r".into()),
            GhCall::IssuesOpen("o/r".into()),
            GhCall::Issue("o/r".into(), 1),
            GhCall::Pr("o/r".into(), 5),
            GhCall::Labels("o/r".into()),
        ]
    );
}

#[test]
fn something_unscripted_is_an_error_naming_it() {
    let fake = FakeGitHub::default();
    let message = format!("{:#}", fake.issue("o/r", 9).unwrap_err());
    assert!(message.contains("issue 9"), "{message}");
    let message = format!("{:#}", fake.pr("o/r", 9).unwrap_err());
    assert!(message.contains("pr 9"), "{message}");
}

#[test]
fn writes_are_recorded_and_answered_with_plausible_values() {
    let fake = FakeGitHub::default().with_next_issue_number(60);
    let request = PrRequest {
        head: "issue-4".into(),
        base: "main".into(),
        title: "t".into(),
        body: "Fixes #4".into(),
    };
    let issue_request = IssueRequest {
        title: "F1".into(),
        body: "b".into(),
        labels: vec!["must-fix".into()],
    };

    let url = fake.pr_create("o/r", &request).unwrap();
    fake.pr_comment("o/r", 5, "review").unwrap();
    assert_eq!(fake.issue_create("o/r", &issue_request).unwrap(), 60);
    assert_eq!(fake.issue_create("o/r", &issue_request).unwrap(), 61);

    assert!(url.starts_with("https://github.com/o/r/pull/"), "{url}");
    assert_eq!(
        fake.calls(),
        [
            GhCall::PrCreate("o/r".into(), request),
            GhCall::PrComment("o/r".into(), 5, "review".into()),
            GhCall::IssueCreate("o/r".into(), issue_request.clone()),
            GhCall::IssueCreate("o/r".into(), issue_request),
        ]
    );
}

#[test]
fn a_failing_fake_fails_every_call_but_still_records_it() {
    let fake = FakeGitHub::default().failing("gh is down");
    let message = format!("{:#}", fake.whoami().unwrap_err());
    assert!(message.contains("gh is down"), "{message}");
    assert_eq!(fake.calls(), [GhCall::Whoami]);
}
