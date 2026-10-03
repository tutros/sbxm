//! M2b slice 11, step 6: `task file-findings --create` (spec §11, decision 134): issues are
//! filed in dependency order, a finding with a marker already in an issue is skipped, links
//! between findings are completed on both sides, the review's `Issues:` line is rewritten.
//! Ported from `ReviewIssues.Filing.Tests.ps1` (Create, Write-back), on `FakeGitHub`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sbxm::commands::task_file_findings::{self, Options, Source};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{GitHubBackend, Issue};
use tempfile::TempDir;

fn small() -> String {
    include_str!("fixtures/review-findings/review-small.md").replace("\r\n", "\n")
}

fn codex() -> String {
    include_str!("fixtures/review-findings/review-m2a-codex.md").replace("\r\n", "\n")
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

struct Setup {
    dir: TempDir,
}

impl Setup {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Files the review at `path` (create or dry run) with the other options tweaked.
    fn file(
        &self,
        path: &Path,
        github: &FakeGitHub,
        create: bool,
        tweak: impl FnOnce(&mut Options),
    ) -> Out {
        let mut opts = Options {
            source: Source::File(path.to_path_buf()),
            repo_root: self.dir.path().to_path_buf(),
            repo: Some("o/r".into()),
            create,
            pr: None,
            standard_criteria: false,
            keep_paths: false,
            only: Vec::new(),
        };
        tweak(&mut opts);
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let result = task_file_findings::run(&opts, github, &mut out, &mut warn);
        Out {
            result,
            out: String::from_utf8(out).unwrap(),
        }
    }

    fn create(&self, name: &str, text: &str, github: &FakeGitHub) -> (Out, PathBuf) {
        let path = self.write(name, text.as_bytes());
        (self.file(&path, github, true, |_| {}), path)
    }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix", "should-fix", "question"])
        .with_next_issue_number(40)
}

fn writes(github: &FakeGitHub) -> Vec<String> {
    github
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::IssueCreate(..) => Some("create".to_owned()),
            GhCall::IssueEdit(_, number, _) => Some(format!("edit {number}")),
            GhCall::IssueLabels(_, number, ..) => Some(format!("label {number}")),
            _ => None,
        })
        .collect()
}

fn issues(github: &FakeGitHub) -> Vec<Issue> {
    github.issues_all("o/r", 1000).unwrap()
}

fn body_of(github: &FakeGitHub, title_prefix: &str) -> String {
    issues(github)
        .into_iter()
        .find(|i| i.title.starts_with(title_prefix))
        .unwrap()
        .body
}

fn issues_line(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .find(|l| l.starts_with("Issues:"))
        .unwrap()
        .to_owned()
}

#[test]
fn issues_are_created_in_dependency_order_with_label_and_title() {
    let setup = Setup::new();
    let gh = github();
    let (run, _) = setup.create("review-small.md", &small(), &gh);
    assert!(run.result.is_ok(), "{}", run.error());
    let all = issues(&gh);
    assert_eq!(
        all.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
        [
            "S-2: Message is wrong",
            "S-1: Thing breaks",
            "S-Q1: Keep the thing?"
        ]
    );
    assert_eq!(
        all.iter().map(|i| i.labels[0].as_str()).collect::<Vec<_>>(),
        ["must-fix", "must-fix", "question"]
    );
    assert_eq!(
        all.iter().map(|i| i.number).collect::<Vec<_>>(),
        [40, 41, 42]
    );
}

#[test]
fn depends_on_becomes_a_number_in_the_dependent_and_the_reverse_related_in_the_dependency() {
    let setup = Setup::new();
    let gh = github();
    setup.create("review-small.md", &small(), &gh);
    assert!(
        body_of(&gh, "S-1").contains("Depends on:** #40 (the message must match)"),
        "{}",
        body_of(&gh, "S-1")
    );
    assert!(body_of(&gh, "S-2").contains("Related:** #41 depends on this one"));
    assert_eq!(writes(&gh), ["create", "create", "create", "edit 40"]);
}

#[test]
fn the_report_has_id_label_number_url_and_what_happened() {
    let setup = Setup::new();
    let (run, _) = setup.create("review-small.md", &small(), &github());
    assert!(
        run.out
            .contains("S-1      must-fix    #41   https://github.com/o/r/issues/41 created"),
        "{}",
        run.out
    );
    assert!(run.out.contains("not filed: 0 sections, 0 nits"));
}

#[test]
fn a_second_run_creates_nothing_and_reuses_the_numbers() {
    let setup = Setup::new();
    let gh = github();
    let (_, path) = setup.create("review-small.md", &small(), &gh);
    let before = writes(&gh).len();
    let run = setup.file(&path, &gh, true, |_| {});
    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(writes(&gh).len(), before);
    assert_eq!(issues(&gh).len(), 3);
    assert!(run.out.contains("S-1      must-fix    #41"));
    assert!(run.out.contains("skipped (exists)"), "{}", run.out);
}

fn marked(number: u32, id: &str) -> Issue {
    Issue {
        number,
        open: true,
        title: format!("{id}: old"),
        labels: vec![],
        body: format!("x\n<!-- review-finding: review-small.md#{id} -->\n"),
    }
}

#[test]
fn the_number_of_a_finding_filed_earlier_is_used_in_links() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![marked(99, "S-2")]);
    setup.create("review-small.md", &small(), &gh);
    assert!(body_of(&gh, "S-1").contains("Depends on:** #99 (the message must match)"));
    assert_eq!(issues(&gh).len(), 3, "the old one, S-1 and S-Q1");
}

#[test]
fn a_marker_matched_issue_gains_its_sections_label_once() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![marked(99, "S-1")]);
    let path = setup.write("review-small.md", small().as_bytes());
    let first = setup.file(&path, &gh, true, |o| o.pr = Some(7));
    assert!(first.result.is_ok(), "{}", first.error());
    let issue = issues(&gh).into_iter().find(|i| i.number == 99).unwrap();
    assert_eq!(issue.labels, ["must-fix"]);

    let before = writes(&gh).len();
    let second = setup.file(&path, &gh, true, |o| o.pr = Some(7));
    assert!(second.result.is_ok(), "{}", second.error());
    assert_eq!(writes(&gh).len(), before, "{:?}", writes(&gh));
}

#[test]
fn a_run_resumes_after_an_interruption_with_only_the_missing_findings() {
    let setup = Setup::new();
    let gh = github().failing_issue_create_at(3);
    let path = setup.write("review-m2a-codex.md", codex().as_bytes());
    let first = setup.file(&path, &gh, true, |o| o.standard_criteria = true);
    assert!(first.error().contains("stopped early"), "{}", first.error());
    assert_eq!(issues(&gh).len(), 2);
    assert_eq!(
        issues_line(&path).matches('#').count(),
        2,
        "{}",
        issues_line(&path)
    );

    let second = setup.file(&path, &gh, true, |o| o.standard_criteria = true);
    assert!(second.result.is_ok(), "{}", second.error());
    let titles: Vec<String> = issues(&gh).into_iter().map(|i| i.title).collect();
    assert_eq!(titles.len(), 4);
    let mut unique = titles.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 4);
}

#[test]
fn a_duplicate_by_title_and_pr_is_skipped_without_creating_a_new_issue() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![Issue {
        number: 77,
        open: true,
        title: "S-1: Thing breaks".into(),
        labels: vec![],
        body: "PR: #7\nold body, no marker\n".into(),
    }]);
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.pr = Some(7));

    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(
        issues(&gh)
            .iter()
            .filter(|i| i.title.starts_with("S-1"))
            .count(),
        1,
        "{:?}",
        issues(&gh)
    );
    assert!(run.out.contains("S-1      must-fix    #77"), "{}", run.out);
    assert_eq!(
        writes(&gh)
            .iter()
            .filter(|w| w.as_str() == "create")
            .count(),
        2,
        "{:?}",
        writes(&gh)
    );

    // A rerun with the same options files nothing new for S-1.
    let before = issues(&gh).len();
    let second = setup.file(&path, &gh, true, |o| o.pr = Some(7));
    assert!(second.result.is_ok(), "{}", second.error());
    assert_eq!(issues(&gh).len(), before);
}

#[test]
fn an_adopted_duplicate_gets_its_sections_label() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![Issue {
        number: 77,
        open: true,
        title: "S-1: Thing breaks".into(),
        labels: vec![],
        body: "PR: #7\nold body, no marker\n".into(),
    }]);
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.pr = Some(7));

    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(issues(&gh)[0].labels, ["must-fix"]);
    assert!(
        run.out.contains("#77") && run.out.contains("skipped (exists, labeled must-fix)"),
        "{}",
        run.out
    );

    // A rerun finds the label in place and changes nothing.
    let before = writes(&gh).len();
    let second = setup.file(&path, &gh, true, |o| o.pr = Some(7));
    assert!(second.result.is_ok(), "{}", second.error());
    assert_eq!(writes(&gh).len(), before, "{:?}", writes(&gh));
}

#[test]
fn an_adopted_duplicates_other_severity_label_is_replaced_and_other_labels_stay() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![Issue {
        number: 77,
        open: true,
        title: "S-1: Thing breaks".into(),
        labels: vec!["bug".into(), "should-fix".into()],
        body: "PR: #7\nold body, no marker\n".into(),
    }]);
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.pr = Some(7));

    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(issues(&gh)[0].labels, ["bug", "must-fix"]);
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
fn a_closed_issue_with_the_same_title_and_pr_is_not_a_duplicate() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![Issue {
        number: 77,
        open: false,
        title: "S-1: Thing breaks".into(),
        labels: vec!["must-fix".into()],
        body: "PR: #7\nold body, no marker\n".into(),
    }]);
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.pr = Some(7));

    assert!(run.result.is_ok(), "{}", run.error());
    let s1: Vec<Issue> = issues(&gh)
        .into_iter()
        .filter(|i| i.title.starts_with("S-1"))
        .collect();
    assert_eq!(s1.len(), 2, "a new S-1 issue next to the closed one");
    assert!(s1[1].open);
    assert_eq!(s1[1].labels, ["must-fix"]);
    assert!(!run.out.contains("#77"), "{}", run.out);
}

#[test]
fn a_title_match_with_a_different_pr_is_not_a_duplicate() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![Issue {
        number: 77,
        open: true,
        title: "S-1: Thing breaks".into(),
        labels: vec![],
        body: "PR: #9\nold body, no marker\n".into(),
    }]);
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.pr = Some(7));

    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(
        issues(&gh)
            .iter()
            .filter(|i| i.title.starts_with("S-1"))
            .count(),
        2,
        "a new S-1 issue is created alongside the old one: {:?}",
        issues(&gh)
    );
}

#[test]
fn an_unreadable_issue_list_files_nothing() {
    let setup = Setup::new();
    let gh = github().failing_issues_all();
    let (run, path) = setup.create("review-small.md", &small(), &gh);
    assert!(run.error().contains("nothing was filed"), "{}", run.error());
    assert!(writes(&gh).is_empty());
    assert_eq!(fs::read_to_string(path).unwrap(), small());
}

#[test]
fn only_files_just_the_named_findings() {
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-small.md", small().as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.only = vec!["S-1".into()]);
    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(
        issues(&gh).into_iter().map(|i| i.title).collect::<Vec<_>>(),
        ["S-1: Thing breaks"]
    );
}

#[test]
fn split_only_batches_complete_the_links_of_both_issues() {
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-small.md", small().as_bytes());
    setup.file(&path, &gh, true, |o| o.only = vec!["S-1".into()]);
    let second = setup.file(&path, &gh, true, |o| o.only = vec!["S-2".into()]);
    assert!(second.result.is_ok(), "{}", second.error());
    assert_eq!(writes(&gh), ["create", "create", "edit 40"]);
    assert!(body_of(&gh, "S-1").contains("Depends on:** #41 (the message must match)"));
    assert!(body_of(&gh, "S-2").contains("Related:** #40 depends on this one"));
    assert_eq!(issues(&gh).len(), 2);
}

#[test]
fn a_failed_link_edit_is_reported_and_a_rerun_finishes_it() {
    let setup = Setup::new();
    let gh = github().failing_issue_edit_at(1);
    let (first, path) = setup.create("review-small.md", &small(), &gh);
    assert!(first.error().contains("links"), "{}", first.error());
    assert!(body_of(&gh, "S-2").contains("S-1 depends on this one"));

    let second = setup.file(&path, &gh, true, |_| {});
    assert!(second.result.is_ok(), "{}", second.error());
    assert!(body_of(&gh, "S-2").contains("Related:** #41 depends on this one"));
}

#[test]
fn only_the_link_fields_of_an_existing_issue_are_patched_so_hand_edits_survive() {
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-small.md", small().as_bytes());
    setup.file(&path, &gh, true, |o| o.only = vec!["S-1".into()]);
    let s1 = body_of(&gh, "S-1").replace("It breaks.", "It breaks, see S-2 in my notes.")
        + "\nHand note about S-2\n";
    gh.issue_edit("o/r", 40, &s1).unwrap();

    setup.file(&path, &gh, true, |o| o.only = vec!["S-2".into()]);

    let after = body_of(&gh, "S-1");
    assert!(after.contains("It breaks, see S-2 in my notes."));
    assert!(after.contains("Hand note about S-2"));
    assert!(after.contains("Depends on:** #41 (the message must match)"));
}

#[test]
fn an_existing_issue_that_already_has_the_numbers_gets_no_edit() {
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-small.md", small().as_bytes());
    setup.file(&path, &gh, true, |o| o.only = vec!["S-1".into()]);
    setup.file(&path, &gh, true, |o| o.only = vec!["S-2".into()]);
    let before = writes(&gh).len();
    setup.file(&path, &gh, true, |_| {});
    assert!(
        writes(&gh)[before..].iter().all(|w| !w.starts_with("edit")),
        "{:?}",
        writes(&gh)
    );
}

#[test]
fn a_failed_edit_of_an_existing_issue_is_reported_and_a_rerun_fixes_it() {
    let setup = Setup::new();
    let gh = github().failing_issue_edit_at(1);
    let path = setup.write("review-small.md", small().as_bytes());
    setup.file(&path, &gh, true, |o| o.only = vec!["S-1".into()]);
    let bad = setup.file(&path, &gh, true, |o| o.only = vec!["S-2".into()]);
    assert!(bad.error().contains("#40 links"), "{}", bad.error());
    assert!(body_of(&gh, "S-1").contains("Depends on:** S-2"));

    let fixed = setup.file(&path, &gh, true, |_| {});
    assert!(fixed.result.is_ok(), "{}", fixed.error());
    assert!(body_of(&gh, "S-1").contains("Depends on:** #41"));
}

#[test]
fn unsafe_text_of_an_excluded_finding_stays_out_of_the_backlink() {
    let token = format!("ghp_{}", "a".repeat(30));
    let text = small()
        .replace(
            "### S-1 - Thing breaks",
            &format!("### {token} - Thing breaks"),
        )
        .replace(
            "**Why it matters:** decision 7",
            "**Why it matters:** decision 7 token = hunter2hunter",
        );
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-unsafe.md", text.as_bytes());
    let run = setup.file(&path, &gh, true, |o| o.only = vec!["S-2".into()]);
    assert!(run.result.is_ok(), "{}", run.error());
    let body = body_of(&gh, "S-2");
    assert!(
        !body.contains("ghp_") && !body.contains("hunter2"),
        "{body}"
    );
}

#[test]
fn nothing_is_filed_when_any_finding_is_refused() {
    let setup = Setup::new();
    let gh = github();
    let (run, _) = setup.create("review-m2a-codex.md", &codex(), &gh);
    assert!(run.error().contains("nothing was filed"), "{}", run.error());
    assert!(writes(&gh).is_empty());
}

// The file is made writable again so the temp dir can be removed; it's ours, not a shared one.
#[allow(clippy::permissions_set_readonly_false)]
#[test]
fn a_review_file_that_cannot_be_updated_is_refused_before_any_issue_is_filed() {
    let setup = Setup::new();
    let gh = github();
    let path = setup.write("review-readonly.md", small().as_bytes());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();

    let run = setup.file(&path, &gh, true, |_| {});

    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    assert!(
        run.error()
            .contains("can't update review-readonly.md to record issue numbers"),
        "{}",
        run.error()
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn the_issues_line_is_replaced_and_nothing_else_changes() {
    let setup = Setup::new();
    let original = small();
    let (_, path) = setup.create("r-lf.md", &original, &github());
    let expected = original.replace(
        "Issues: pending (test)",
        "Issues: S-1 #41, S-2 #40, S-Q1 #42",
    );
    assert_eq!(fs::read(&path).unwrap(), expected.as_bytes());
}

#[test]
fn crlf_line_endings_are_kept() {
    let setup = Setup::new();
    let original = small().replace('\n', "\r\n");
    let (_, path) = setup.create("r-crlf.md", &original, &github());
    let expected = original.replace(
        "Issues: pending (test)",
        "Issues: S-1 #41, S-2 #40, S-Q1 #42",
    );
    assert_eq!(fs::read(&path).unwrap(), expected.as_bytes());
}

#[test]
fn a_byte_order_mark_is_kept() {
    let setup = Setup::new();
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(small().as_bytes());
    let path = setup.write("r-bom.md", &bytes);
    let run = setup.file(&path, &github(), true, |_| {});
    assert!(run.result.is_ok(), "{}", run.error());
    let after = fs::read(&path).unwrap();
    assert_eq!(&after[..3], [0xEF, 0xBB, 0xBF]);
    assert!(
        String::from_utf8(after[3..].to_vec())
            .unwrap()
            .contains("Issues: S-1 #41, S-2 #40, S-Q1 #42")
    );
}

#[test]
fn findings_that_were_not_filed_stay_pending() {
    let setup = Setup::new();
    let path = setup.write("r.md", small().as_bytes());
    setup.file(&path, &github(), true, |o| o.only = vec!["S-1".into()]);
    assert_eq!(
        issues_line(&path),
        "Issues: S-1 #40, S-2 pending, S-Q1 pending"
    );
}

#[test]
fn an_interrupted_run_writes_the_numbers_that_exist() {
    let setup = Setup::new();
    let gh = github().failing_issue_create_at(2);
    let (run, path) = setup.create("r.md", &small(), &gh);
    assert!(run.error().contains("stopped early"), "{}", run.error());
    assert_eq!(
        issues_line(&path),
        "Issues: S-1 pending, S-2 #40, S-Q1 pending"
    );
}

#[test]
fn the_codex_reviews_long_issues_line_becomes_the_short_form() {
    let setup = Setup::new();
    let path = setup.write("review-m2a-codex.md", codex().as_bytes());
    let run = setup.file(&path, &github(), true, |o| o.standard_criteria = true);
    assert!(run.result.is_ok(), "{}", run.error());
    let line = issues_line(&path);
    assert!(
        line.starts_with("Issues: M2A-1 #") && line.contains(", M2A-Q1 #"),
        "{line}"
    );
    assert_eq!(
        fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("Issues:"))
            .count(),
        1
    );
}

#[test]
fn a_dry_run_does_not_touch_the_file() {
    let setup = Setup::new();
    let path = setup.write("r.md", small().as_bytes());
    let run = setup.file(&path, &github(), false, |_| {});
    assert!(run.result.is_ok(), "{}", run.error());
    assert_eq!(fs::read_to_string(&path).unwrap(), small());
}
