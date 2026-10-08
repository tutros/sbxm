//! M2b slice 11, step 1: parsing a review file into findings (spec §11, decision 134). The cases
//! are ported from the removed Pester tests, on the same fixtures.

use sbxm::task::findings::{self, Review};

const CODEX: &str = include_str!("fixtures/review-findings/review-m2a-codex.md");
const DASH: char = '\u{2014}';

fn review(heading: &str, section: &str) -> String {
    format!(
        "# Review\n\nScope: `aaaaaaa..bbbbbbb` (2 commits)\nIssues: pending (no access)\n\n## {section}\n\n### {heading}\n\n**Where:** `src/a.rs:1-2`\n**What happens:** it breaks\n**Why it matters:** decision 1\n**Fix:** fix it\n**Acceptance criteria:**\n- [ ] it works\n"
    )
}

fn one(text: &str) -> Review {
    findings::parse(text)
}

#[test]
fn the_codex_review_has_four_findings_with_ids_labels_and_titles() {
    let r = one(CODEX);
    let found: Vec<_> = r
        .findings
        .iter()
        .map(|f| format!("{}|{}", f.id, f.label))
        .collect();
    assert_eq!(
        found,
        [
            "M2A-1|must-fix",
            "M2A-2|must-fix",
            "M2A-3|should-fix",
            "M2A-Q1|question"
        ]
    );
    assert_eq!(
        r.findings[0].title,
        "Inherited Git config variables can execute a host command while diffing a contestant workspace"
    );
    assert_eq!(
        r.findings[1].title,
        "`[eval.cosine]` is accepted but silently does nothing"
    );
}

#[test]
fn fields_are_read_under_canonical_names() {
    let r = one(CODEX);
    let m1 = &r.findings[0];
    assert!(
        m1.field("where")
            .unwrap()
            .starts_with("`src/git.rs:50-72`, used by")
    );
    assert!(
        m1.field("what happens")
            .unwrap()
            .contains("GIT_CONFIG_COUNT")
    );
    assert!(
        m1.field("why it matters")
            .unwrap()
            .starts_with("decision 127")
    );
    assert!(
        m1.field("fix")
            .unwrap()
            .starts_with("construct the Git child")
    );
    assert!(m1.field("depends on").is_none());
}

#[test]
fn smallest_fix_and_recommendation_map_to_fix() {
    let r = one(CODEX);
    assert!(
        r.findings[2]
            .field("fix")
            .unwrap()
            .starts_with("before merge")
    );
    assert!(
        r.findings[3]
            .field("fix")
            .unwrap()
            .starts_with("reject Antigravity")
    );
}

#[test]
fn summary_verification_and_nit_text_is_reported_as_not_filed() {
    let r = one(CODEX);
    assert_eq!(r.not_filed_sections, 2);
    assert_eq!(r.not_filed_nits, 1);
}

#[test]
fn the_scope_head_sha_and_the_issues_line_are_read() {
    let r = one(CODEX);
    assert_eq!(
        r.head_sha.as_deref(),
        Some("82659a26614c58da671641ffa6e62264cb9038ce")
    );
    assert!(
        r.issues_line
            .as_deref()
            .unwrap()
            .starts_with("Issues: pending.")
    );
    assert_eq!(r.issues_line_count, 1);
}

#[test]
fn the_codex_review_has_nothing_missing() {
    for f in &one(CODEX).findings {
        assert!(f.missing.is_empty(), "{}: {:?}", f.id, f.missing);
    }
}

#[test]
fn id_and_title_split_on_a_hyphen_a_colon_and_an_em_dash() {
    for heading in [
        "C-1 - Title here".to_owned(),
        "C-1: Title here".to_owned(),
        format!("C-1 {DASH} Title here"),
    ] {
        let r = one(&review(&heading, "Must fix"));
        assert_eq!(r.findings.len(), 1, "{heading}");
        assert_eq!(r.findings[0].id, "C-1");
        assert_eq!(r.findings[0].title, "Title here");
    }
}

#[test]
fn smallest_fix_gives_the_same_structure_as_fix() {
    let codex_style = one(&review(&format!("C-1 {DASH} Title here"), "Must fix"));
    let canonical =
        one(&review("C-1 - Title here", "Must fix").replace("**Fix:**", "**Smallest fix:**"));
    let names = |r: &Review| -> Vec<String> {
        r.findings[0]
            .fields
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    };
    assert_eq!(names(&canonical), names(&codex_style));
    assert_eq!(canonical.findings[0].criteria, ["it works"]);
}

#[test]
fn section_headings_map_to_labels_ignoring_case_and_colons() {
    for (section, label) in [
        ("Must fix", "must-fix"),
        ("Should fix:", "should-fix"),
        ("QUESTION", "question"),
        ("Questions", "question"),
    ] {
        assert_eq!(
            one(&review("C-1 - T", section)).findings[0].label,
            label,
            "{section}"
        );
    }
}

#[test]
fn field_names_match_case_insensitively() {
    let text = review("C-1 - T", "Must fix").replace("**What happens:**", "**WHAT HAPPENS:**");
    assert!(one(&text).findings[0].missing.is_empty());
}

#[test]
fn a_missing_required_field_is_named() {
    let text = review("C-1 - T", "Must fix").replace("**Why it matters:** decision 1\n", "");
    assert_eq!(one(&text).findings[0].missing, ["why it matters"]);
}

#[test]
fn a_question_needs_fix_too() {
    let text = review("C-1 - T", "Questions").replace("**Fix:** fix it\n", "");
    assert_eq!(one(&text).findings[0].missing, ["fix"]);
}

#[test]
fn a_field_value_may_sit_below_the_label_line() {
    let text = review("C-1 - T", "Must fix").replace(
        "**What happens:** it breaks",
        "**What happens:**\n\nit breaks\nin two lines",
    );
    assert_eq!(
        one(&text).findings[0].field("what happens"),
        Some("it breaks\nin two lines")
    );
}

#[test]
fn crlf_files_read_the_same_way() {
    let crlf = review("C-1 - T", "Must fix").replace('\n', "\r\n");
    assert_eq!(one(&crlf).findings[0].field("fix"), Some("fix it"));
}

#[test]
fn start_and_end_lines_cover_the_heading_and_the_body() {
    let f = &one(&review("C-1 - T", "Must fix")).findings[0];
    assert_eq!(f.start_line, 8);
    assert_eq!(f.end_line, 16);
}

#[test]
fn issues_lines_are_counted() {
    let text = review("C-1 - T", "Must fix").replace(
        "Issues: pending (no access)",
        "Issues: pending (no access)\nIssues: pending (again)",
    );
    let r = one(&text);
    assert_eq!(r.issues_line_count, 2);
    assert_eq!(
        r.issues_line.as_deref(),
        Some("Issues: pending (no access)")
    );
}

#[test]
fn a_finding_under_outside_this_change_is_not_a_finding() {
    // Issue 152: what the change did not cause is reported, never filed and never counted.
    let r = one(&review("O-1 - an older gap", "Outside this change"));
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert_eq!(r.not_filed_sections, 1);
}
