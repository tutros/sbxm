//! M2b slice 11, step 3: rendering a finding as an issue (spec §11, decision 134): permalinks,
//! criteria, links between findings, dependency order and the `Issues:` line. Ported from
//! `ReviewIssues.Render.Tests.ps1` and `ReviewIssues.Criteria.Tests.ps1` (golden bodies in
//! `tests/fixtures/review-findings/`), plus the link and ordering cases of the Filing tests.

use std::collections::BTreeMap;

use sbxm::task::findings::{
    self, Render, Review, acceptance_criteria, filing_order, issue_body, issues_line, link_ids,
    pr_of, update_link_fields, where_links,
};

const SMALL: &str = include_str!("fixtures/review-findings/review-small.md");
const CODEX: &str = include_str!("fixtures/review-findings/review-m2a-codex.md");
const GOLDEN_S1: &str = include_str!("fixtures/review-findings/golden-body-s1.md");
const GOLDEN_S2: &str = include_str!("fixtures/review-findings/golden-body-s2-linked.md");
const GOLDEN_Q1: &str = include_str!("fixtures/review-findings/golden-body-q1.md");
const SHA: &str = "2222222222222222222222222222222222222222";

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn render_with<'a>(map: &'a BTreeMap<String, u32>, standard: bool) -> Render<'a> {
    Render {
        repo: "o/r",
        review_name: "review-small.md",
        review_ref: "sdlc/reviews/review-small.md",
        head_sha: Some(SHA),
        ids: map,
        standard_criteria: standard,
        pr: None,
    }
}

fn body(review: &Review, index: usize, map: &BTreeMap<String, u32>) -> String {
    issue_body(
        review,
        &review.findings[index],
        &render_with(map, false),
        &mut Vec::new(),
    )
    .unwrap()
}

fn links(text: &str, sha: Option<&str>) -> String {
    where_links(text, "o/r", sha, &mut Vec::new())
}

#[test]
fn a_path_with_a_line_range_links_at_the_head_sha() {
    assert_eq!(
        links("`src/git.rs:50-72`", Some(SHA)),
        format!("[`src/git.rs:50-72`](https://github.com/o/r/blob/{SHA}/src/git.rs#L50-L72)")
    );
}

#[test]
fn a_single_line_a_bare_path_and_a_path_outside_backticks_link() {
    assert_eq!(
        links("src/b.rs:5 and `README.md`", Some(SHA)),
        format!(
            "[src/b.rs:5](https://github.com/o/r/blob/{SHA}/src/b.rs#L5) and [`README.md`](https://github.com/o/r/blob/{SHA}/README.md)"
        )
    );
}

#[test]
fn ordinary_words_are_left_alone() {
    assert_eq!(
        links("used by e.g. the parser", Some(SHA)),
        "used by e.g. the parser"
    );
}

#[test]
fn an_extensionless_file_name_with_a_line_suffix_links() {
    assert_eq!(
        links(
            "`justfile:76` and `Dockerfile:1-3` and deploy/Makefile:4",
            Some(SHA)
        ),
        format!(
            "[`justfile:76`](https://github.com/o/r/blob/{SHA}/justfile#L76) and [`Dockerfile:1-3`](https://github.com/o/r/blob/{SHA}/Dockerfile#L1-L3) and [deploy/Makefile:4](https://github.com/o/r/blob/{SHA}/deploy/Makefile#L4)"
        )
    );
}

#[test]
fn plain_words_with_a_colon_and_a_number_stay_text() {
    let text = "at step:3 the ratio 3:1 of profile:2 and e.g. a justfile without a line";
    assert_eq!(links(text, Some(SHA)), text);
}

#[test]
fn with_no_sha_the_text_stays_and_a_warning_says_why() {
    let mut warnings = Vec::new();
    let out = where_links("`src/git.rs:50-72`", "o/r", None, &mut warnings);
    assert_eq!(out, "`src/git.rs:50-72`");
    assert_eq!(
        warnings,
        ["no commit sha found in the Scope line; Where links stay plain text"]
    );
}

#[test]
fn the_template_parts_are_in_order_and_end_with_the_marker_golden() {
    let review = findings::parse(SMALL);
    let out = body(&review, 0, &BTreeMap::new());
    assert_eq!(out.trim_end(), lf(GOLDEN_S1).trim_end());
}

#[test]
fn a_missing_depends_on_says_none_known_and_the_reverse_related_is_added_golden() {
    let review = findings::parse(SMALL);
    let out = body(&review, 1, &BTreeMap::new());
    assert_eq!(out.trim_end(), lf(GOLDEN_S2).trim_end());
}

#[test]
fn a_question_uses_options_and_recommendation_and_has_no_criteria_golden() {
    let review = findings::parse(SMALL);
    let out = body(&review, 2, &BTreeMap::new());
    assert_eq!(out.trim_end(), lf(GOLDEN_Q1).trim_end());
}

#[test]
fn finding_ids_become_issue_numbers_where_they_are_known() {
    let review = findings::parse(SMALL);
    let map = BTreeMap::from([("S-2".to_owned(), 41)]);
    assert!(body(&review, 0, &map).contains("Depends on:** #41 (the message must match)"));
}

#[test]
fn link_ids_only_touches_whole_ids() {
    let map = BTreeMap::from([("S-1".to_owned(), 7)]);
    assert_eq!(
        link_ids("S-1 and S-10 and XS-1 and S-1-b", &map),
        "#7 and S-10 and XS-1 and S-1-b"
    );
}

const STANDARD: [&str; 3] = [
    "A test covering it fails before the fix and passes after (name it, or say which file it goes in)",
    "`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass",
    "Docs updated where behavior users see changed (`README.md`), or \"no user-visible change\"",
];

#[test]
fn a_finding_with_no_criteria_is_refused_with_the_exact_message() {
    let codex = findings::parse(CODEX);
    assert_eq!(
        acceptance_criteria(&codex.findings[0], false).unwrap_err(),
        "M2A-1 has no acceptance criteria; add them to the review file, or rerun with --standard-criteria to file it with only the standard ones"
    );
}

#[test]
fn standard_criteria_give_the_placeholder_then_the_standard_checklist() {
    let codex = findings::parse(CODEX);
    let items = acceptance_criteria(&codex.findings[0], true).unwrap();
    assert_eq!(
        items[0],
        "<the specific check is missing from the review: add one before working this issue>"
    );
    assert_eq!(items[1..], STANDARD);
}

#[test]
fn a_findings_own_criteria_stay_and_only_the_missing_standard_ones_are_appended() {
    let small = findings::parse(SMALL);
    let finding = &small.findings[0];
    assert_eq!(
        acceptance_criteria(finding, false).unwrap(),
        finding.criteria
    );

    let mut own = finding.clone();
    own.criteria = vec![
        "it works".to_owned(),
        "A test covering it fails before the fix and passes after (name it)".to_owned(),
    ];
    let items = acceptance_criteria(&own, false).unwrap();
    assert_eq!(items[..2], own.criteria[..]);
    assert_eq!(items[2..], STANDARD[1..]);
}

#[test]
fn a_question_gets_no_criteria() {
    let codex = findings::parse(CODEX);
    assert!(
        acceptance_criteria(&codex.findings[3], true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_body_renders_the_standard_criteria_for_a_finding_with_none_when_asked() {
    let codex = findings::parse(CODEX);
    let map = BTreeMap::new();
    let sha = "a".repeat(40);
    let render = Render {
        review_name: "x.md",
        review_ref: "sdlc/reviews/x.md",
        head_sha: Some(&sha),
        ..render_with(&map, true)
    };
    let out = issue_body(&codex, &codex.findings[1], &render, &mut Vec::new()).unwrap();
    assert!(out.contains("Acceptance criteria:**\n- [ ] <the specific check is missing"));
    assert!(out.contains("- [ ] `cargo fmt --check`"));

    let refused = issue_body(
        &codex,
        &codex.findings[1],
        &render_with(&map, false),
        &mut Vec::new(),
    );
    assert!(
        refused
            .unwrap_err()
            .starts_with("M2A-2 has no acceptance criteria")
    );
}

#[test]
fn findings_are_filed_after_the_ones_they_depend_on() {
    let review = findings::parse(SMALL);
    let order: Vec<_> = filing_order(&review.findings)
        .iter()
        .map(|f| f.id.as_str())
        .collect();
    assert_eq!(order, ["S-2", "S-1", "S-Q1"]);
}

#[test]
fn a_dependency_cycle_keeps_file_order_among_its_members() {
    let text = SMALL.replace(
        "**Where:** `src/b.rs:5`",
        "**Depends on:** S-1\n**Where:** `src/b.rs:5`",
    );
    let review = findings::parse(&text);
    let order: Vec<_> = filing_order(&review.findings)
        .iter()
        .map(|f| f.id.as_str())
        .collect();
    assert_eq!(order, ["S-Q1", "S-1", "S-2"]);
}

#[test]
fn the_issues_line_has_numbers_would_file_markers_and_pending() {
    let review = findings::parse(SMALL);
    let numbers = BTreeMap::from([("S-1".to_owned(), 41)]);
    let would = ["S-2".to_owned()];
    assert_eq!(
        issues_line(&review.findings, &numbers, &would, "#?"),
        "Issues: S-1 #41, S-2 #?, S-Q1 pending"
    );
}

#[test]
fn a_pr_is_named_on_the_bodys_first_line() {
    let review = findings::parse(SMALL);
    let map = BTreeMap::new();
    let render = Render {
        pr: Some(57),
        ..render_with(&map, false)
    };
    let out = issue_body(&review, &review.findings[0], &render, &mut Vec::new()).unwrap();
    assert_eq!(out.lines().next(), Some("PR: #57"));
}

#[test]
fn with_no_pr_the_body_has_no_pr_line() {
    let review = findings::parse(SMALL);
    let out = body(&review, 0, &BTreeMap::new());
    assert!(!out.lines().any(|l| l == "PR: #57"));
    assert!(out.lines().next().unwrap().starts_with("**Where:**"));
}

#[test]
fn pr_of_reads_the_first_line_only() {
    assert_eq!(pr_of("PR: #57\nmore text\n"), Some(57));
    assert_eq!(pr_of("PR: #57"), Some(57));
}

#[test]
fn pr_of_ignores_the_line_anywhere_but_the_first() {
    assert_eq!(pr_of("something else\nPR: #57\n"), None);
    assert_eq!(pr_of("PR: #57 extra\n"), None);
    assert_eq!(pr_of(""), None);
}

#[test]
fn set_pr_line_replaces_a_stale_first_line_instead_of_adding_another() {
    assert_eq!(
        findings::set_pr_line("PR: #9\nold body\n", 7),
        "PR: #7\nold body\n"
    );
    assert_eq!(findings::set_pr_line("PR: #9", 7), "PR: #7");
    assert_eq!(findings::set_pr_line("PR: #7\nbody\n", 7), "PR: #7\nbody\n");
    assert_eq!(
        findings::set_pr_line("body\n<!-- marker -->\n", 7),
        "PR: #7\nbody\n<!-- marker -->\n"
    );
    assert_eq!(pr_of("no pr here"), None);
}

#[test]
fn only_the_depends_on_and_related_fields_of_an_existing_body_are_patched() {
    let map = BTreeMap::from([("S-2".to_owned(), 41)]);
    let body = "x\n**Why:** see S-2 in my notes\n**Depends on:** S-2 (the message\nS-2 again)\n**Related:** S-2\n\n**Fix:** S-2 stays\nHand note about S-2\n<!-- review-finding: r.md#S-1 -->\n";
    assert_eq!(
        update_link_fields(body, &map),
        "x\n**Why:** see S-2 in my notes\n**Depends on:** #41 (the message\n#41 again)\n**Related:** #41\n\n**Fix:** S-2 stays\nHand note about S-2\n<!-- review-finding: r.md#S-1 -->\n"
    );
}

#[test]
fn a_link_field_ends_at_a_comment_and_keeps_crlf() {
    let map = BTreeMap::from([("S-2".to_owned(), 41)]);
    let body = "**Related:** S-2\r\n<!-- S-2 -->\r\n";
    assert_eq!(
        update_link_fields(body, &map),
        "**Related:** #41\r\n<!-- S-2 -->\r\n"
    );
}
