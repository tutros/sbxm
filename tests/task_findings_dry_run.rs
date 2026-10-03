//! M2b slice 11, step 5: `sbxm task file-findings --file F` as a dry run (spec §4, §11,
//! decisions 134, 149), and everything it refuses before any write. Ported from
//! `ReviewIssues.Filing.Tests.ps1`: the access checks, the up-front refusals, the dry run.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use sbxm::commands::task_file_findings::{self, Options, Source};
use sbxm::github::Issue;
use sbxm::github::fake::{FakeGitHub, GhCall};
use tempfile::TempDir;

// The fixtures may be checked out with CRLF; the tests build text with LF.
fn small() -> String {
    include_str!("fixtures/review-findings/review-small.md").replace("\r\n", "\n")
}

fn codex() -> String {
    include_str!("fixtures/review-findings/review-m2a-codex.md").replace("\r\n", "\n")
}

struct Setup {
    dir: TempDir,
}

struct Out {
    result: Result<()>,
    out: String,
    warn: String,
    path: PathBuf,
}

impl Out {
    /// The error as one string, or empty when it worked.
    fn error(&self) -> String {
        self.result
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    }
}

impl Setup {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn review(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join("reviews").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn options(&self, path: &Path) -> Options {
        Options {
            source: Source::File(path.to_path_buf()),
            repo_root: self.dir.path().to_path_buf(),
            repo: Some("o/r".into()),
            create: false,
            pr: None,
            standard_criteria: false,
            keep_paths: false,
            only: Vec::new(),
        }
    }

    fn run(
        &self,
        name: &str,
        text: impl AsRef<str>,
        github: &FakeGitHub,
        tweak: impl FnOnce(&mut Options),
    ) -> Out {
        let path = self.review(name, text.as_ref());
        let mut opts = self.options(&path);
        tweak(&mut opts);
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let result = task_file_findings::run(&opts, github, &mut out, &mut warn);
        Out {
            result,
            out: String::from_utf8(out).unwrap(),
            warn: String::from_utf8(warn).unwrap(),
            path,
        }
    }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix", "should-fix", "question"])
}

fn dry(name: &str, text: impl AsRef<str>) -> Out {
    Setup::new().run(name, text, &github(), |_| {})
}

fn no_writes(github: &FakeGitHub) {
    for call in github.calls() {
        assert!(
            !matches!(call, GhCall::IssueCreate(..) | GhCall::IssueEdit(..)),
            "{call:?}"
        );
    }
}

fn secret(prefix: &str, n: usize, c: char) -> String {
    format!("{prefix}{}", c.to_string().repeat(n))
}

#[test]
fn a_dry_run_prints_every_issue_and_the_issues_line_and_writes_nothing() {
    let setup = Setup::new();
    let gh = github();
    let run = setup.run("review-small.md", small(), &gh, |_| {});

    assert!(run.result.is_ok(), "{}", run.error());
    for expected in [
        "would create [must-fix] S-1: Thing breaks",
        "would create [must-fix] S-2: Message is wrong",
        "would create [question] S-Q1: Keep the thing?",
        "decision 7",
        "<!-- review-finding: review-small.md#S-1 -->",
        "not filed: 0 sections, 0 nits",
        "would write: Issues: S-1 #?, S-2 #?, S-Q1 #?",
    ] {
        assert!(run.out.contains(expected), "{expected}\n{}", run.out);
    }
    no_writes(&gh);
    assert_eq!(fs::read_to_string(&run.path).unwrap(), small());
}

#[test]
fn only_files_just_the_findings_named() {
    let setup = Setup::new();
    let run = setup.run("review-small.md", small(), &github(), |o| {
        o.only = vec!["S-2".into()]
    });
    assert!(run.out.contains("S-2: Message is wrong"));
    assert!(!run.out.contains("S-1: Thing breaks"));
}

#[test]
fn an_unknown_id_in_only_is_refused_naming_the_ones_there_are() {
    let setup = Setup::new();
    let gh = github();
    let run = setup.run("review-small.md", small(), &gh, |o| {
        o.only = vec!["S-9".into()]
    });
    let error = run.error();
    assert!(
        error.contains("--only names S-9, which isn't in review-small.md"),
        "{error}"
    );
    assert!(error.contains("S-1, S-2, S-Q1"), "{error}");
    assert!(gh.calls().is_empty());
}

#[test]
fn findings_with_no_acceptance_criteria_are_refused_unless_standard_criteria_is_given() {
    let run = dry("review-m2a-codex.md", codex());
    assert!(run.out.contains("refused: M2A-1 has no acceptance criteria; add them to the review file, or rerun with --standard-criteria"), "{}", run.out);
    assert!(
        run.error().contains("nothing would be filed"),
        "{}",
        run.error()
    );
}

#[test]
fn standard_criteria_files_all_four_codex_findings_and_reports_what_is_not_filed() {
    let setup = Setup::new();
    let run = setup.run("review-m2a-codex.md", codex(), &github(), |o| {
        o.standard_criteria = true
    });
    assert!(run.result.is_ok(), "{}", run.error());
    for id in ["M2A-1", "M2A-2", "M2A-3", "M2A-Q1"] {
        assert!(run.out.contains(id), "{id}");
    }
    assert!(
        run.out.contains("not filed: 2 sections, 1 nit"),
        "{}",
        run.out
    );
}

#[test]
fn a_finding_that_holds_a_secret_is_refused_naming_the_id_and_line_not_the_value() {
    let token = secret("ghp_", 30, 'a');
    let text = small().replace("It breaks.", &format!("It breaks with {token}"));
    let run = dry("review-secret.md", &text);
    assert!(
        run.out.contains(
            "refused: S-1 line 11 looks like a GitHub token; remove it from the review file"
        ),
        "{}",
        run.out
    );
    assert!(!run.out.contains("aaaaaaaa") && !run.error().contains("aaaaaaaa"));
    assert!(
        run.error().contains("nothing would be filed: 1 problem"),
        "{}",
        run.error()
    );
}

#[test]
fn a_missing_review_file_is_refused() {
    let setup = Setup::new();
    let opts = Options {
        source: Source::File(setup.dir.path().join("nope.md")),
        ..setup.options(&PathBuf::new())
    };
    let result = task_file_findings::run(&opts, &github(), &mut Vec::new(), &mut Vec::new());
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("not found") && error.contains("sdlc/reviews"),
        "{error}"
    );
}

#[test]
fn duplicate_finding_ids_are_refused_before_any_github_call() {
    let text = small().replace("### S-2 - Message is wrong", "### S-1 - Message is wrong");
    let setup = Setup::new();
    let gh = github();
    let run = setup.run("review-dup.md", &text, &gh, |_| {});
    assert!(
        run.out.contains(
            "refused: S-1 is used by more than one finding in review-dup.md, at lines 8 and 21"
        ),
        "{}",
        run.out
    );
    assert!(
        run.error().contains("make finding ids unique"),
        "{}",
        run.error()
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn an_id_with_characters_outside_the_allowed_set_is_refused_before_any_github_call() {
    let text = small().replace(
        "### S-2 - Message is wrong",
        r"### C:\Users\alice - Message is wrong",
    );
    let setup = Setup::new();
    let gh = github();
    let run = setup.run("review-badid.md", &text, &gh, |o| {
        o.only = vec!["S-1".into()]
    });
    assert!(
        run.out.contains(r"refused: finding id C:\Users\alice") && run.out.contains("A-Za-z0-9._-"),
        "{}",
        run.out
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn the_review_needs_exactly_one_issues_line_before_any_github_call() {
    let setup = Setup::new();
    let gh = github();
    let none = setup.run(
        "none.md",
        small().replace("Issues: pending (test)\n", ""),
        &gh,
        |_| {},
    );
    assert!(
        none.error()
            .contains("has no 'Issues:' line(s); keep exactly one"),
        "{}",
        none.error()
    );
    let two = setup.run(
        "two.md",
        small().replace(
            "Issues: pending (test)",
            "Issues: pending (test)\nIssues: pending (again)",
        ),
        &gh,
        |_| {},
    );
    assert!(
        two.error()
            .contains("has 2 'Issues:' line(s); keep exactly one"),
        "{}",
        two.error()
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn a_review_with_no_findings_is_refused() {
    let run = dry(
        "empty.md",
        "# Review\nScope: x\nIssues: pending\n\n## Summary\n\nfine\n",
    );
    assert!(
        run.error().contains("no findings found in empty.md"),
        "{}",
        run.error()
    );
}

#[test]
fn access_checks_stop_before_any_write() {
    let setup = Setup::new();
    // Not logged in.
    let gh = github().failing("not logged in");
    let run = setup.run("a.md", small(), &gh, |_| {});
    assert!(
        run.error()
            .contains("gh isn't logged in; run `gh auth login`"),
        "{}",
        run.error()
    );
    assert!(run.error().contains("Issues: pending"), "{}", run.error());
    no_writes(&gh);
    // A label that doesn't exist: nothing is created.
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix", "should-fix"]);
    let run = setup.run("b.md", small(), &gh, |_| {});
    assert!(
        run.error()
            .contains("label 'question' doesn't exist in o/r; create it"),
        "{}",
        run.error()
    );
    no_writes(&gh);
}

#[test]
fn a_label_no_selected_finding_uses_is_not_needed() {
    let setup = Setup::new();
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_labels(&["must-fix"]);
    let run = setup.run("a.md", small(), &gh, |o| {
        o.only = vec!["S-1".into(), "S-2".into()]
    });
    assert!(run.result.is_ok(), "{}", run.error());
}

#[test]
fn without_a_repo_the_origin_must_be_a_github_repo() {
    let setup = Setup::new();
    let run = setup.run("a.md", small(), &github(), |o| o.repo = None);
    assert!(
        run.error().contains("pass --repo owner/name"),
        "{}",
        run.error()
    );
}

#[test]
fn a_no_sha_scope_gets_the_same_protection_as_a_field() {
    let scope = |line: &str| {
        small()
            .lines()
            .map(|l| if l.starts_with("Scope:") { line } else { l })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    // A personal path is replaced and warned about.
    let run = dry(
        "p.md",
        scope("Scope: reviewed by hand, notes in /home/alice/project"),
    );
    assert!(
        run.out
            .contains("reviewed commits `reviewed by hand, notes in ~/project`"),
        "{}",
        run.out
    );
    assert!(
        run.warn.contains("replaced 1 personal path"),
        "{}",
        run.warn
    );
    assert!(!run.out.contains("/home/alice"));
    // --keep-paths keeps it.
    let kept = Setup::new().run(
        "k.md",
        scope("Scope: reviewed by hand, notes in /home/alice/project"),
        &github(),
        |o| o.keep_paths = true,
    );
    assert!(
        kept.out
            .contains("`reviewed by hand, notes in /home/alice/project`"),
        "{}",
        kept.out
    );
    // An e-mail address is warned about, not changed.
    let mail = dry("m.md", scope("Scope: reviewed by alice@example.com"));
    assert!(
        mail.out.contains("`reviewed by alice@example.com`")
            && mail.warn.contains("e-mail address")
    );
    // A secret is refused, line named, value not.
    let token = secret("sk-", 24, 'e');
    let bad = dry(
        "s.md",
        scope(&format!("Scope: reviewed by hand, output: {token}")),
    );
    assert!(
        bad.out
            .contains("refused: Scope line 3 looks like an sk- key"),
        "{}",
        bad.out
    );
    assert!(!bad.out.contains("eeeeeeee"));
}

#[test]
fn a_review_file_name_that_looks_like_a_secret_is_refused() {
    let name = format!("{}.md", secret("sk-", 24, 'e'));
    let run = dry(&name, small());
    assert!(
        run.out
            .contains("the review file name looks like an sk- key; rename the file"),
        "{}",
        run.out
    );
    assert!(!run.out.contains("eeeeeeee"));
}

#[test]
fn a_short_sha_is_resolved_by_git_or_the_links_stay_text() {
    let text = small().replace(
        "`1111111111111111111111111111111111111111..2222222222222222222222222222222222222222`",
        "`1111111..2222222`",
    );
    let run = dry("short.md", &text);
    assert!(
        run.warn.contains(
            "git can't resolve the reviewed commit 2222222 here; Where links stay plain text"
        ),
        "{}",
        run.warn
    );
    assert!(
        run.out.contains("**Where:** `src/a.rs:10-20`, `README.md`"),
        "{}",
        run.out
    );
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
fn a_finding_that_already_has_an_issue_is_skipped_in_the_dry_run_and_its_number_is_used() {
    let setup = Setup::new();
    let gh = github().with_issues(vec![marked(99, "S-2")]);
    let run = setup.run("review-small.md", small(), &gh, |_| {});
    assert!(run.out.contains("S-2 skipped (exists #99)"), "{}", run.out);
    assert!(
        run.out
            .contains("**Depends on:** #99 (the message must match)"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("would write: Issues: S-1 #?, S-2 #99, S-Q1 #?"),
        "{}",
        run.out
    );
}

#[test]
fn an_unreadable_issue_list_stops_the_run() {
    let setup = Setup::new();
    let run = setup.run("a.md", small(), &github().failing_issues_all(), |_| {});
    assert!(
        run.error().contains("can't read the issue list of o/r"),
        "{}",
        run.error()
    );
    assert!(run.error().contains("nothing was filed"), "{}", run.error());
}

#[test]
fn an_issue_list_as_long_as_the_limit_stops_the_run() {
    let setup = Setup::new();
    let issues = (1..=1000)
        .map(|n| Issue {
            number: n,
            open: true,
            title: format!("t{n}"),
            labels: vec![],
            body: String::new(),
        })
        .collect();
    let run = setup.run("a.md", small(), &github().with_issues(issues), |_| {});
    assert!(
        run.error().contains("1000") && run.error().contains("nothing was filed"),
        "{}",
        run.error()
    );
}

#[test]
fn the_fake_is_asked_for_the_login_labels_and_issues_in_that_order() {
    let setup = Setup::new();
    let gh = github();
    setup.run("a.md", small(), &gh, |_| {});
    assert_eq!(
        gh.calls(),
        [
            GhCall::Whoami,
            GhCall::Labels("o/r".into()),
            GhCall::IssuesAll("o/r".into(), 1000)
        ]
    );
}
