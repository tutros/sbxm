//! M2b slice 11, step 2: refusing secrets and replacing personal paths in a review's text
//! (decision 134). Ported from `scripts/tests/ReviewIssues.Scrub.Tests.ps1`. Secret-shaped
//! values are built here, so the repo holds no literal that looks like one.

use sbxm::task::findings::{self, find_secrets, protect_finding, protect_text, secret_kind};

fn review_with(line: &str) -> String {
    format!(
        "# Review\n\nScope: `aaaaaaa..bbbbbbb` (1 commit)\nIssues: pending (test)\n\n## Must fix\n\n### S-1 - Thing\n\n**Where:** `src/a.rs:1`\n**What happens:** first line\n{line}\n**Why it matters:** x\n**Fix:** y\n**Acceptance criteria:**\n- [ ] z\n"
    )
}

fn hits(text: &str) -> Vec<findings::SecretHit> {
    let review = findings::parse(text);
    find_secrets(text, &review.findings[0])
}

fn protect(text: &str) -> (String, Vec<String>) {
    let mut warnings = Vec::new();
    let out = protect_text(text, "S-1", false, &mut warnings);
    (out, warnings)
}

#[test]
fn secret_shaped_values_are_refused_naming_the_line_but_not_the_value() {
    let cases = [
        ("a GitHub token", format!("ghp_{}", "a".repeat(30))),
        ("a GitHub token", format!("gho_{}", "b".repeat(30))),
        ("a GitHub token", format!("ghs_{}", "c".repeat(30))),
        ("a GitHub token", format!("github_pat_{}", "d".repeat(30))),
        ("an sk- key", format!("sk-{}", "e".repeat(24))),
        ("an AWS key", format!("AKIA{}", "F".repeat(16))),
        ("a bearer token", format!("Bearer {}", "g".repeat(30))),
        (
            "a private key",
            "-----BEGIN RSA PRIVATE KEY-----".to_owned(),
        ),
        (
            "a password or token assignment",
            "password = hunter2hunter".to_owned(),
        ),
        (
            "a password or token assignment",
            "token = \"abcd1234efgh\"".to_owned(),
        ),
    ];
    for (kind, value) in cases {
        let found = hits(&review_with(&format!("output: {value}")));
        assert_eq!(found.len(), 1, "{kind}");
        assert_eq!(found[0].id, "S-1");
        assert_eq!(found[0].line, 12);
        assert_eq!(found[0].kind, kind);
        assert!(!format!("{:?}", found[0]).contains(&value[6..]), "{kind}");
    }
}

#[test]
fn short_assignments_are_refused() {
    for text in [
        "token = a",
        "token = ab",
        "token = abc",
        "password = x",
        "password=xy",
        "secret = xyz",
        "api_token = abc",
        "token = \"a\"",
        "password = 'ab'",
        "token = \"abc\"",
    ] {
        let found = hits(&review_with(&format!("output: {text}")));
        assert_eq!(found.len(), 1, "{text}");
        assert_eq!(found[0].kind, "a password or token assignment");
    }
}

#[test]
fn empty_assignments_comparisons_and_prose_pass() {
    for text in [
        "token =",
        "password = ",
        "token = \"\"",
        "if token == other then",
        "the token is missing",
        "tokens = 5 of them",
        "A token is required; the password field is empty.",
    ] {
        assert!(
            hits(&review_with(&format!("output: {text}"))).is_empty(),
            "{text}"
        );
    }
}

#[test]
fn the_codex_review_has_no_secrets() {
    let text = include_str!("fixtures/review-findings/review-m2a-codex.md");
    for finding in &findings::parse(text).findings {
        assert!(find_secrets(text, finding).is_empty(), "{}", finding.id);
    }
}

#[test]
fn secret_kind_works_on_any_text() {
    assert_eq!(
        secret_kind("sk-eeeeeeeeeeeeeeeeeeeeeeee.md"),
        Some("an sk- key")
    );
    assert_eq!(secret_kind("review-small.md"), None);
}

#[test]
fn a_personal_windows_path_becomes_a_tilde_with_a_warning() {
    let (out, warnings) = protect(r"see C:\Users\james\proj\x.rs");
    assert_eq!(out, r"see ~\proj\x.rs");
    assert_eq!(
        warnings,
        ["S-1: replaced 1 personal path(s) with ~; use --keep-paths to keep them"]
    );
}

#[test]
fn a_personal_linux_path_becomes_a_tilde() {
    assert_eq!(protect("in /home/james/src/a.rs").0, "in ~/src/a.rs");
}

#[test]
fn a_profile_name_with_spaces_goes_whole() {
    assert_eq!(
        protect(r"see C:\Users\Jane Doe\project\x.rs").0,
        r"see ~\project\x.rs"
    );
    assert_eq!(
        protect("in /home/Jane Doe/project/x.rs").0,
        "in ~/project/x.rs"
    );
    assert_eq!(
        protect(r"C:\Users\Mary Jane Watson Parker\repo").0,
        r"~\repo"
    );
    assert_eq!(
        protect("at /home/Mary Jane Watson Parker/repo/x").0,
        "at ~/repo/x"
    );
    assert_eq!(
        protect("in /Users/Mary Jane Watson Parker Smith/repo").0,
        "in ~/repo"
    );
}

#[test]
fn a_spaced_profile_name_stops_at_a_quote_a_backtick_or_a_newline() {
    assert_eq!(protect(r#"a "C:\Users\Jane Doe" b"#).0, r#"a "~" b"#);
    assert_eq!(protect("x /home/Jane Doe`y/z").0, "x ~`y/z");
    assert_eq!(
        protect("C:\\Users\\james\nsee D:\\x\\y").0,
        "~\nsee D:\\x\\y"
    );
}

#[test]
fn prose_after_a_bare_path_is_left_alone() {
    assert_eq!(
        protect(r"C:\Users\james is the folder").0,
        "~ is the folder"
    );
    assert_eq!(
        protect("under /home/james and then /etc/x").0,
        "under ~ and then /etc/x"
    );
}

#[test]
fn keep_paths_keeps_them() {
    let mut warnings = Vec::new();
    let out = protect_text(r"see C:\Users\james\x", "S-1", true, &mut warnings);
    assert_eq!(out, r"see C:\Users\james\x");
    assert!(warnings.is_empty());
}

#[test]
fn project_paths_are_left_alone() {
    let (out, warnings) = protect(r"see E:\sbxm-projects\x");
    assert_eq!(out, r"see E:\sbxm-projects\x");
    assert!(warnings.is_empty());
}

#[test]
fn an_email_address_is_warned_about_but_not_changed() {
    let (out, warnings) = protect("ask a@b.example");
    assert_eq!(out, "ask a@b.example");
    assert_eq!(warnings, ["S-1: contains an e-mail address (left as is)"]);
}

#[test]
fn protect_finding_covers_the_title_every_field_and_the_criteria() {
    let text =
        review_with(r"path C:\Users\james\x").replace("- [ ] z", r"- [ ] check C:\Users\james\y");
    let finding = &findings::parse(&text).findings[0];
    let mut warnings = Vec::new();
    let safe = protect_finding(finding, false, &mut warnings);
    assert_eq!(safe.field("what happens"), Some("first line\npath ~\\x"));
    assert_eq!(safe.criteria, [r"check ~\y"]);
    assert_eq!(warnings.len(), 2);
}
