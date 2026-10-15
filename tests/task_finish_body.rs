//! Issue 59 (review finding R2-S3): the agent-written text `pr_body` embeds (`result.md`,
//! `review.md`, the risk section and a draft's unresolved section) can't close other issues or
//! PRs, or notify anyone; sbxm's own `Fixes #N` stays active.

use sbxm::task::finish::{SECTION_CAP, pr_body};

const ZWSP: char = '\u{200b}';
const RISK: &str = "## Risk: \u{26aa} unknown\n";

/// Each piece of agent text and what it must read like once embedded (`|` marks the zero-width
/// space).
const CASES: &[(&str, &str)] = &[
    ("close #10", "close| #10"),
    ("closes #10", "closes| #10"),
    ("closed #10", "closed| #10"),
    ("fix #10", "fix| #10"),
    ("fixes #10", "fixes| #10"),
    ("fixed #10", "fixed| #10"),
    ("resolve #10", "resolve| #10"),
    ("resolves #10", "resolves| #10"),
    ("resolved #10", "resolved| #10"),
    ("Closes #10", "Closes| #10"),
    ("CLOSES #10", "CLOSES| #10"),
    ("Closes: #10", "Closes|: #10"),
    ("CLOSES: #10", "CLOSES|: #10"),
    (
        "fixes octo-org/octo-repo#100",
        "fixes| octo-org/octo-repo#100",
    ),
    (
        "Fixes https://github.com/octo-org/octo-repo/issues/100",
        "Fixes| https://github.com/octo-org/octo-repo/issues/100",
    ),
    (
        "Resolves #10, resolves #123, resolves octo-org/octo-repo#100",
        "Resolves| #10, resolves| #123, resolves| octo-org/octo-repo#100",
    ),
    ("a PR: closes #240.", "a PR: closes| #240."),
    ("(fixes #3)", "(fixes| #3)"),
    ("prefix #12", "prefix #12"),
    ("unresolved #3", "unresolved #3"),
    ("suffixes #4", "suffixes #4"),
    ("fixing #4", "fixing #4"),
    ("fixes the parser", "fixes the parser"),
    ("ask @octocat", "ask @|octocat"),
    ("@team-a, please", "@|team-a, please"),
    ("mail a@example.com", "mail a@example.com"),
];

fn expected(case: &str) -> String {
    case.replace('|', &ZWSP.to_string())
}

/// The body with nothing but sbxm's own lines and `text` where `place` puts it.
fn body_with(place: &str, text: &str) -> String {
    let risk = format!("{RISK}\n- {text}\n");
    let unresolved = format!("## Unresolved\n\n- {text}\n");
    let (body, _) = match place {
        "result" => pr_body(7, RISK, None, Some(text), None),
        "review" => pr_body(7, RISK, None, None, Some(text)),
        "risk" => pr_body(7, &risk, None, None, None),
        "unresolved" => pr_body(7, RISK, Some(&unresolved), None, None),
        other => panic!("no place {other}"),
    };
    body
}

#[test]
fn agent_text_in_every_place_is_neutralized_and_stays_readable() {
    for place in ["result", "review", "risk", "unresolved"] {
        for (input, want) in CASES {
            let body = body_with(place, input);

            assert!(
                body.contains(&expected(want)),
                "{place}: {input:?} should read {want:?} in:\n{body}"
            );
            assert!(
                body.replace(ZWSP, "").contains(input),
                "{place}: {input:?}: only zero-width spaces may be added"
            );
        }
    }
}

#[test]
fn sbxms_own_fixes_line_stays_active() {
    let (body, _) = pr_body(
        7,
        "## Risk: \u{1f7e2} low\n\n- fixes #8\n",
        Some("## Unresolved\n\n- closes #9\n"),
        Some("Fixes #10\n"),
        Some("resolves #11\n"),
    );

    assert!(body.contains("\nFixes #7\n"), "{body}");
    assert_eq!(body.matches("Fixes #7").count(), 1, "{body}");
    for n in 8..=11 {
        assert!(body.contains(&format!("{ZWSP} #{n}")), "#{n}: {body}");
    }
}

#[test]
fn a_keyword_right_at_the_cut_stays_inert() {
    // Every split of the keyword, its zero-width space and the reference around the cap.
    for pad in SECTION_CAP - 12..=SECTION_CAP {
        let text = format!("{} closes #10 and more", "x".repeat(pad));

        let (body, cuts) = pr_body(7, RISK, None, Some(&text), None);

        assert!(!body.contains("closes #"), "pad {pad}: {:?}", tail(&body));
        if !cuts.is_empty() {
            assert!(body.contains("(cut: result.md"), "pad {pad}");
        }
    }
}

fn tail(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    chars[chars.len().saturating_sub(200)..].iter().collect()
}
