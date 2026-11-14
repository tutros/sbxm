//! Issue 220 part 2, decision 283: `task finish` copies the worker result's "Needs a decision"
//! and "Choices made" sections into the PR description, after the risk section, "Needs a
//! decision" first; a section that is absent, empty or "none" is left out.

use sbxm::task::finish::{HANDBACK_CAP, SECTION_CAP, pr_body};

const RISK: &str = "## Risk: \u{26aa} unknown\n";

fn body_of(result: &str) -> String {
    pr_body(7, RISK, None, Some(result), None).0
}

/// The part of `body` before `## Result`, where the copied sections go.
fn head(body: &str) -> &str {
    &body[..body.find("## Result\n").expect(body)]
}

const BOTH: &str = "## Done\n\nAll of it.\n\n\
## Choices made\n\n- Kept the cap at 3 kB: the risk section uses the same.\n\n\
## Needs a decision\n\n- Should X be Y? I'd pick Y.\n\n\
## Self-review\n\nNothing.\n";

#[test]
fn a_non_empty_choices_made_section_follows_the_risk_section() {
    let result = "## Done\n\nyes\n\n## Choices made\n\n- Picked A over B: simpler.\n\n## Needs a decision\n\nnone\n";

    let body = body_of(result);

    let head = head(&body);
    let choices = head.find("## Choices made\n").expect(head);
    assert!(head.find(RISK).unwrap() < choices, "{body}");
    assert!(
        head[choices..].contains("- Picked A over B: simpler.\n"),
        "{body}"
    );
}

#[test]
fn a_non_empty_needs_a_decision_section_comes_before_choices_made() {
    let body = body_of(BOTH);

    let head = head(&body);
    let risk = head.find(RISK).expect(head);
    let needs = head.find("## Needs a decision\n").expect(head);
    let choices = head.find("## Choices made\n").expect(head);
    assert!(risk < needs && needs < choices, "{body}");
    assert!(
        head[needs..choices].contains("- Should X be Y? I'd pick Y.\n"),
        "{body}"
    );
    // Each section stops at the next heading of the result.
    assert!(!head[needs..choices].contains("Kept the cap"), "{body}");
    assert!(!head[choices..].contains("Self-review"), "{body}");
    assert!(!head[choices..].contains("Should X"), "{body}");
}

#[test]
fn a_needs_a_decision_section_alone_is_copied() {
    let body = body_of("## Needs a decision\n\n- Q? I'd pick A.\n\n## Choices made\n\nNone.\n");

    let head = head(&body);
    assert!(
        head.contains("## Needs a decision\n\n- Q? I'd pick A.\n"),
        "{body}"
    );
    assert!(!head.contains("## Choices made"), "{body}");
}

#[test]
fn a_section_that_is_none_empty_or_absent_is_left_out() {
    for result in [
        "## Choices made\n\nnone\n\n## Needs a decision\n\nnone\n",
        "## Choices made\n\nNone.\n\n## Needs a decision\n\n- none\n",
        "## Choices made\n\n**None**\n\n## Needs a decision\n\n_none_\n",
        "## Choices made\n\n## Needs a decision\n\n\n## Self-review\n\nok\n",
        "## Choices made: none\n\n## Needs a decision: none\n",
        "## Done\n\nNo hand-back sections at all.\n",
        "",
    ] {
        let body = body_of(result);

        let head = head(&body);
        assert!(!head.contains("## Choices made"), "{result:?}:\n{body}");
        assert!(!head.contains("## Needs a decision"), "{result:?}:\n{body}");
    }
}

#[test]
fn no_result_md_gives_no_copied_sections() {
    let (body, _) = pr_body(7, RISK, None, None, None);

    assert!(!body.contains("## Choices made"), "{body}");
    assert!(!body.contains("## Needs a decision"), "{body}");
}

#[test]
fn headings_of_any_level_case_and_a_trailing_colon_are_found() {
    let result = "# Result\n\n### needs a decision:\n\n- Q?\n\n### **Choices made**\n\n- C.\n#### detail\n\nmore of C\n\n## Self-review\n\nok\n";

    let body = body_of(result);

    let head = head(&body);
    let needs = head.find("## Needs a decision\n").expect(&body);
    let choices = head.find("## Choices made\n").expect(&body);
    assert!(needs < choices, "{body}");
    // A deeper heading belongs to the section; the next one at its level or above ends it.
    assert!(head[choices..].contains("more of C"), "{body}");
    assert!(!head[choices..].contains("ok"), "{body}");
}

#[test]
fn a_heading_inside_a_code_fence_neither_starts_nor_ends_a_section() {
    let result = "## Choices made\n\n- C, shown as:\n\n```\n## Self-review\n## Needs a decision\nfake\n```\n\nstill C\n\n## Needs a decision\n\nnone\n";

    let body = body_of(result);

    let head = head(&body);
    let choices = head.find("## Choices made\n").expect(&body);
    assert!(head[choices..].contains("still C"), "{body}");
    assert_eq!(head.matches("## Needs a decision").count(), 1, "{body}");
    assert!(
        head[choices..].contains("## Needs a decision\nfake"),
        "{body}"
    );
}

#[test]
fn a_copied_section_is_neutralised_like_the_rest_of_the_agent_text() {
    let body = body_of("## Choices made\n\n- closes #10, ask @octocat\n");

    let head = head(&body);
    assert!(head.contains("closes\u{200b} #10"), "{body}");
    assert!(head.contains("@\u{200b}octocat"), "{body}");
}

#[test]
fn a_section_at_the_cap_is_whole_and_one_over_is_cut_with_a_note() {
    for (len, cut) in [
        (HANDBACK_CAP - 1, false),
        (HANDBACK_CAP, false),
        (HANDBACK_CAP + 1, true),
    ] {
        let text = "x".repeat(len);
        let result = format!("## Choices made\n\n{text}\n");

        let (body, cuts) = pr_body(7, RISK, None, Some(&result), None);

        let head = head(&body);
        assert_eq!(head.contains(&text), !cut, "{len}");
        assert_eq!(cuts.len(), usize::from(cut), "{len}: {cuts:?}");
        if cut {
            assert!(head.contains(&"x".repeat(HANDBACK_CAP)), "{len}");
            assert!(
                head.contains("(cut: the \"Choices made\" section"),
                "{head}"
            );
        }
    }
}

#[test]
fn every_part_at_its_largest_still_fits_what_github_accepts() {
    let huge = "x".repeat(70_000);
    let risk = format!("## Risk: \u{1f534} high\n\n- {huge}\n");
    let unresolved = format!("## Unresolved\n\n{}\n", "u".repeat(4_990));
    let result = format!(
        "## Needs a decision\n\n{huge}\n\n## Choices made\n\n{huge}\n\n## Rest\n\n{huge}\n"
    );
    assert!(result.chars().count() > SECTION_CAP);

    let (body, cuts) = pr_body(7, &risk, Some(&unresolved), Some(&result), Some(&huge));

    assert!(body.chars().count() < 65_536, "{}", body.chars().count());
    assert_eq!(cuts.len(), 5, "{cuts:?}");
}
