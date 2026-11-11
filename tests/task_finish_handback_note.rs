//! Issue 585: a "Needs a decision" section whose first line is "none" hands no question back;
//! the rest of it is a note, kept in the PR body under the section's heading as `None. Note:`.

use sbxm::task::finish::{handed_back, hands_back, pr_body};

const RISK: &str = "## Risk: \u{26aa} unknown\n";

/// The part of the PR body for `result` before `## Result`, where the copied sections go.
fn head_of(result: &str) -> String {
    let body = pr_body(7, RISK, None, Some(result), None).0;
    body[..body.find("## Result\n").expect(&body)].to_owned()
}

fn assert_no_question(result: &str) {
    assert_eq!(handed_back(result), None, "{result}");
    assert!(!hands_back(result), "{result}");
}

#[test]
fn a_section_that_is_just_none_is_no_question() {
    for result in [
        "## Needs a decision\n\nnone\n",
        "## Needs a decision\n\nNone.\n",
        "## Needs a decision\n\n**none**\n",
        "## Needs a decision: none\n",
    ] {
        assert_no_question(result);
        assert!(!head_of(result).contains("Needs a decision"), "{result}");
    }
}

#[test]
fn an_empty_section_is_no_question() {
    let result = "## Needs a decision\n\n\n## Self-review\n\nok\n";
    assert_no_question(result);
    assert!(!head_of(result).contains("Needs a decision"), "{result}");
}

/// The shape of PR #553's `result.md`.
const NONE_WITH_NOTE: &str = "## Choices made\n\nnone\n\n## Needs a decision\n\nnone\n\n\
The `--foo` flag could be renamed later, but nothing here depends on it.\n\n\
## Self-review\n\nok\n";

#[test]
fn none_then_a_note_paragraph_is_no_question() {
    assert_no_question(NONE_WITH_NOTE);
    for first in ["None.", "**None**", "- none", "NONE"] {
        let result = format!("## Needs a decision\n\n{first}\n\nA note.\n");
        assert_no_question(&result);
    }
}

#[test]
fn none_then_a_note_is_kept_in_the_pr_body_as_a_note() {
    let head = head_of(NONE_WITH_NOTE);
    assert!(
        head.contains(
            "## Needs a decision\n\nNone. Note: The `--foo` flag could be renamed later, but \
             nothing here depends on it.\n"
        ),
        "{head}"
    );
    assert!(!head.contains("## Choices made"), "{head}");
}

#[test]
fn an_inline_none_heading_then_a_note_is_no_question() {
    let result = "## Needs a decision: none\n\nA note on the flag.\n\n## Self-review\n\nok\n";
    assert_no_question(result);
    assert!(
        head_of(result).contains("## Needs a decision\n\nNone. Note: A note on the flag.\n"),
        "{}",
        head_of(result)
    );
}

#[test]
fn a_first_line_that_starts_with_none_but_says_more_is_a_question() {
    let result = "## Needs a decision\n\nnone of the options fit: pick A or B\n";
    assert_eq!(
        handed_back(result).as_deref(),
        Some("none of the options fit: pick A or B")
    );
    assert!(hands_back(result));
}

#[test]
fn a_real_question_whose_second_line_is_none_is_a_question() {
    let result = "## Needs a decision\n\n- Should X be Y? I'd pick Y.\nnone\n";
    assert_eq!(
        handed_back(result).as_deref(),
        Some("- Should X be Y? I'd pick Y.\nnone")
    );
    assert!(hands_back(result));
}

#[test]
fn of_a_none_with_a_note_and_a_real_question_only_the_real_one_is_shown() {
    let result = "## Needs a decision\n\nnone\n\nJust a note.\n\n## Done\n\nyes\n\n\
## Needs a decision\n\n- Should X be Y? I'd pick Y.\n";
    assert_eq!(
        handed_back(result).as_deref(),
        Some("- Should X be Y? I'd pick Y.")
    );
    assert!(hands_back(result));
    let head = head_of(result);
    assert!(
        head.contains("## Needs a decision\n\n- Should X be Y? I'd pick Y.\n"),
        "{head}"
    );
    assert!(!head.contains("Just a note"), "{head}");
}

// --- review M-1: only a first line that is "none" makes a note ---

#[test]
fn a_divider_before_a_real_question_is_still_a_question() {
    for divider in ["---", "***", "___"] {
        let result = format!("## Needs a decision\n\n{divider}\n\nShould we pick A or B?\n");
        assert_eq!(
            handed_back(&result).as_deref(),
            Some(format!("{divider}\n\nShould we pick A or B?").as_str()),
            "{result}"
        );
        assert!(hands_back(&result), "{result}");
        let head = head_of(&result);
        assert!(head.contains("Should we pick A or B?"), "{head}");
        assert!(!head.contains("None. Note:"), "{head}");
    }
}

#[test]
fn a_section_of_only_a_divider_is_no_question() {
    let result = "## Needs a decision\n\n---\n\n## Self-review\n\nok\n";
    assert_no_question(result);
}
