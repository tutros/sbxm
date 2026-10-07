//! M2b slice 8, step 1: reading a reviewer's `review.md` (spec §5.2): the first line says how many
//! must-fix findings there are; a review without that line is not used.
//!
//! Issue 119 (decisions 174(b), 177(e)): the no-progress rule's matching. A claimed repeat
//! (`**Repeat of:** <id>`) stops the task only when it names a must-fix finding of the previous
//! review and the two share a `Where:` file path (line numbers ignored); anything else counts as
//! new.

use sbxm::task::review::{must_fix_count, repeats_a_must_fix_finding, with_header};

const PREVIOUS: &str = "Must-fix findings: 1\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n";

fn current(where_line: &str, repeat_of: Option<&str>, label: &str) -> String {
    let heading = match label {
        "must fix" => "## Must fix",
        "should fix" => "## Should fix",
        _ => unreachable!(),
    };
    let id = if label == "must fix" { "M-1" } else { "S-1" };
    let repeat = repeat_of.map_or_else(String::new, |id| format!("**Repeat of:** {id}\n"));
    format!(
        "Must-fix findings: 1\n\n\
         {heading}\n\n\
         ### {id} - a.txt still does the wrong thing\n\n\
         **Where:** `{where_line}`\n\
         **What happens:** it still returns the wrong value\n\
         **Why it matters:** decision 1\n\
         **Fix:** return the right value\n\
         {repeat}"
    )
}

#[test]
fn a_repeat_naming_the_same_file_stops_the_task() {
    let text = current("a.txt:99", Some("M-1"), "must fix");
    assert!(repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn line_numbers_are_ignored_when_matching_the_file() {
    let text = current("a.txt:1", Some("M-1"), "must fix");
    assert!(repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn a_repeat_naming_a_different_file_counts_as_new() {
    let text = current("b.txt:99", Some("M-1"), "must fix");
    assert!(!repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn a_repeat_naming_an_id_that_does_not_exist_counts_as_new() {
    let text = current("a.txt:99", Some("M-9"), "must fix");
    assert!(!repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn a_missing_repeat_of_line_counts_as_new() {
    let text = current("a.txt:1", None, "must fix");
    assert!(!repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn literal_new_is_not_a_repeat() {
    let text = current("a.txt:1", Some("new"), "must fix");
    assert!(!repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn a_repeated_should_fix_finding_never_stops_the_task() {
    let text = current("a.txt:1", Some("M-1"), "should fix");
    assert!(!repeats_a_must_fix_finding(&text, PREVIOUS));
}

#[test]
fn the_count_comes_from_the_first_line() {
    assert_eq!(
        must_fix_count("Must-fix findings: 0\n\nNo findings.\n"),
        Some(0)
    );
    assert_eq!(must_fix_count("Must-fix findings: 3\n\n1. ..."), Some(3));
    assert_eq!(must_fix_count("Must-fix findings: 12"), Some(12));
}

#[test]
fn trailing_spaces_and_a_windows_line_ending_are_fine() {
    assert_eq!(must_fix_count("Must-fix findings: 2  \r\nrest"), Some(2));
    assert_eq!(must_fix_count("Must-fix findings: 2\r\n"), Some(2));
}

#[test]
fn anything_else_on_the_first_line_means_no_usable_count() {
    for bad in [
        "",
        "\n",
        "Must-fix findings:\n",
        "Must-fix findings: none\n",
        "Must-fix findings: -1\n",
        "must-fix findings: 1\n",
        "Must-fix findings: 1 (see below)\n",
        "# Review\nMust-fix findings: 0\n",
        "\nMust-fix findings: 0\n",
        "Must-fix findings: 99999999999999999999\n",
    ] {
        assert_eq!(must_fix_count(bad), None, "{bad:?}");
    }
}

#[test]
fn a_byte_order_mark_before_the_line_is_tolerated() {
    assert_eq!(must_fix_count("\u{feff}Must-fix findings: 1\n"), Some(1));
}

#[test]
fn the_saved_review_names_its_reviewer_above_the_text() {
    let text = with_header(
        "codex",
        Some("gpt-5.6-sol"),
        "Must-fix findings: 1\n\nfinding\n",
    );
    assert_eq!(
        text,
        "Reviewer: codex (gpt-5.6-sol)\n\nMust-fix findings: 1\n\nfinding\n"
    );
}

#[test]
fn a_reviewer_with_no_chosen_model_says_default_model() {
    let text = with_header("claude", None, "Must-fix findings: 0\n");
    assert!(
        text.starts_with("Reviewer: claude (default model)\n\n"),
        "{text}"
    );
}
