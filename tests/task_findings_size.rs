//! Issue 233: a finding's `**Size:** S | M | L` line goes into its issue's body and becomes a
//! `size:*` label. A finding without one, or with another value, is filed as before and the
//! report says so; an adopted issue gains a size label only when it has none; a missing
//! `size:*` label in the repo is refused like a missing severity label.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use sbxm::commands::task_file_findings::{self, Options, Source};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{GitHubBackend, Issue};
use tempfile::TempDir;

/// `review-small.md` with S-1 sized `M` and S-2 given `size` as its size line, or none.
fn review(s2_size: Option<&str>) -> String {
    let text = include_str!("fixtures/review-findings/review-small.md")
        .replace("\r\n", "\n")
        .replace(
            "**Fix:** Fix it.\n",
            "**Fix:** Fix it.\n**Size:** M (one command, tests and README)\n",
        );
    match s2_size {
        Some(size) => text.replace(
            "**Fix:** Say the right thing.\n",
            &format!("**Fix:** Say the right thing.\n**Size:** {size}\n"),
        ),
        None => text,
    }
}

struct Out {
    result: Result<()>,
    out: String,
}

impl Out {
    fn error(&self) -> String {
        self.result
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    }
}

fn run(text: &str, github: &FakeGitHub, create: bool, pr: Option<u32>) -> Out {
    let dir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("review-small.md");
    fs::write(&path, text).unwrap();
    let opts = Options {
        source: Source::File(path),
        repo_root: dir.path().to_path_buf(),
        repo: Some("o/r".into()),
        create,
        pr,
        standard_criteria: false,
        keep_paths: false,
        only: Vec::new(),
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_file_findings::run(&opts, github, &mut out, &mut warn);
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_labels(&[
            "must-fix",
            "should-fix",
            "question",
            "size:S",
            "size:M",
            "size:L",
        ])
        .with_next_issue_number(40)
}

fn issue(github: &FakeGitHub, title_prefix: &str) -> Issue {
    github
        .issues_all("o/r", 1000)
        .unwrap()
        .into_iter()
        .find(|i| i.title.starts_with(title_prefix))
        .unwrap()
}

fn writes(github: &FakeGitHub) -> Vec<GhCall> {
    github
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                GhCall::IssueCreate(..) | GhCall::IssueEdit(..) | GhCall::IssueLabels(..)
            )
        })
        .collect()
}

/// An open issue for S-1 found by its title and PR, with these labels.
fn adopted(labels: &[&str]) -> Issue {
    Issue {
        number: 77,
        open: true,
        title: "S-1: Thing breaks".into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        body: "PR: #7\nold body, no marker\n".into(),
    }
}

#[test]
fn a_dry_run_shows_the_size_label_and_the_size_line() {
    let gh = github();
    let run = run(&review(None), &gh, false, None);

    assert!(run.result.is_ok(), "{}", run.error());
    assert!(
        run.out
            .contains("would create [must-fix, size:M] S-1: Thing breaks"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("**Size:** M (one command, tests and README)"),
        "{}",
        run.out
    );
    assert!(writes(&gh).is_empty(), "{:?}", writes(&gh));
}

#[test]
fn create_writes_the_size_line_and_adds_the_size_label() {
    let gh = github();
    let run = run(&review(None), &gh, true, None);

    assert!(run.result.is_ok(), "{}", run.error());
    let s1 = issue(&gh, "S-1");
    assert_eq!(s1.labels, ["must-fix", "size:M"]);
    assert!(
        s1.body
            .starts_with("**Size:** M (one command, tests and README)\n**Where:**"),
        "{}",
        s1.body
    );
}

#[test]
fn a_finding_without_a_size_line_is_filed_without_one_and_the_report_says_so() {
    for create in [false, true] {
        let gh = github();
        let run = run(&review(None), &gh, create, None);

        assert!(run.result.is_ok(), "{}", run.error());
        assert!(
            run.out
                .contains("S-2 has no size (no **Size:** line), so it gets no size:* label"),
            "create {create}: {}",
            run.out
        );
        if create {
            let s2 = issue(&gh, "S-2");
            assert_eq!(s2.labels, ["must-fix"]);
            assert!(!s2.body.contains("**Size:**"), "{}", s2.body);
        } else {
            assert!(
                run.out
                    .contains("would create [must-fix] S-2: Message is wrong"),
                "{}",
                run.out
            );
        }
    }
}

#[test]
fn a_finding_whose_size_is_not_s_m_or_l_is_filed_without_one_and_the_report_says_so() {
    for size in ["huge", "XL", "S | M | L", "m", "Medium"] {
        for create in [false, true] {
            let gh = github();
            let run = run(&review(Some(size)), &gh, create, None);

            assert!(run.result.is_ok(), "{size}: {}", run.error());
            assert!(
                run.out.contains(&format!(
                    "S-2 has no size ('{size}' is not S, M or L), so it gets no size:* label"
                )),
                "{size}, create {create}: {}",
                run.out
            );
            if create {
                let s2 = issue(&gh, "S-2");
                assert_eq!(s2.labels, ["must-fix"], "{size}");
                assert!(!s2.body.contains("**Size:**"), "{size}: {}", s2.body);
            }
        }
    }
}

#[test]
fn a_size_followed_by_its_reason_in_any_common_shape_is_read() {
    for (line, label) in [
        ("S", "size:S"),
        ("L: the state format", "size:L"),
        ("S, one bullet", "size:S"),
        ("M - one command", "size:M"),
        ("**L** (three open points)", "size:L"),
    ] {
        let gh = github();
        let run = run(&review(Some(line)), &gh, true, None);
        assert!(run.result.is_ok(), "{line}: {}", run.error());
        assert_eq!(issue(&gh, "S-2").labels, ["must-fix", label], "{line}");
    }
}

#[test]
fn an_adopted_issue_without_a_size_label_gets_the_findings() {
    let gh = github().with_issues(vec![adopted(&["must-fix"])]);
    let dry = run(&review(None), &gh, false, Some(7));
    assert!(
        dry.out
            .contains("S-1 skipped (exists #77), would label it size:M"),
        "{}",
        dry.out
    );
    assert!(writes(&gh).is_empty(), "{:?}", writes(&gh));

    let done = run(&review(None), &gh, true, Some(7));
    assert!(done.result.is_ok(), "{}", done.error());
    assert_eq!(issue(&gh, "S-1").labels, ["must-fix", "size:M"]);
    assert!(
        done.out.contains("skipped (exists, labeled size:M)"),
        "{}",
        done.out
    );
}

#[test]
fn an_adopted_issue_with_a_size_label_keeps_it() {
    let gh = github().with_issues(vec![adopted(&["should-fix", "size:L"])]);
    let dry = run(&review(None), &gh, false, Some(7));
    assert!(
        dry.out
            .contains("S-1 skipped (exists #77), would label it must-fix (removing should-fix)\n"),
        "{}",
        dry.out
    );

    let done = run(&review(None), &gh, true, Some(7));
    assert!(done.result.is_ok(), "{}", done.error());
    assert_eq!(issue(&gh, "S-1").labels, ["size:L", "must-fix"]);
    assert!(
        gh.calls().contains(&GhCall::IssueLabels(
            "o/r".into(),
            77,
            vec!["must-fix".into()],
            vec!["should-fix".into()]
        )),
        "{:?}",
        gh.calls()
    );
}

#[test]
fn a_missing_size_label_in_the_repo_is_refused_before_anything_is_filed() {
    for create in [false, true] {
        let gh = FakeGitHub::default().with_user("me").with_labels(&[
            "must-fix",
            "should-fix",
            "question",
            "size:S",
            "size:L",
        ]);
        let run = run(&review(None), &gh, create, None);
        assert!(
            run.error().contains(
                "label 'size:M' doesn't exist in o/r; create it (this command creates no labels;"
            ),
            "{}",
            run.error()
        );
        assert!(writes(&gh).is_empty(), "{:?}", writes(&gh));
        assert!(
            !gh.calls()
                .iter()
                .any(|c| matches!(c, GhCall::IssuesAll(..))),
            "refused before the issue list is read: {:?}",
            gh.calls()
        );
    }
}

// Issue 99 (PR #86 review round 4, S-1): the dry run names the label an adopted issue ends up
// with, even when the only change is removing a conflicting severity. Boundary cases first.
#[test]
fn a_dry_run_names_the_target_label_in_every_kind_of_label_change() {
    for (labels, line) in [
        // Nothing to add, a conflicting severity to remove: the desired severity is named.
        (
            &["must-fix", "should-fix", "size:L"][..],
            "S-1 skipped (exists #77), would label it must-fix (removing should-fix)\n",
        ),
        // Only a severity to add.
        (
            &["size:L"][..],
            "S-1 skipped (exists #77), would label it must-fix\n",
        ),
        // Only a size to add.
        (
            &["must-fix"][..],
            "S-1 skipped (exists #77), would label it size:M\n",
        ),
        // A severity and a size to add.
        (
            &[][..],
            "S-1 skipped (exists #77), would label it must-fix, size:M\n",
        ),
        // No label change at all.
        (&["must-fix", "size:L"][..], "S-1 skipped (exists #77)\n"),
    ] {
        let gh = github().with_issues(vec![adopted(labels)]);
        let dry = run(&review(None), &gh, false, Some(7));
        assert!(dry.result.is_ok(), "{labels:?}: {}", dry.error());
        assert!(dry.out.contains(line), "{labels:?}: {}", dry.out);
        assert!(writes(&gh).is_empty(), "{labels:?}: {:?}", writes(&gh));
    }
}
