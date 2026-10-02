//! M2b slice 8, step 1: reading a reviewer's `review.md` (spec §5.2): the first line says how many
//! must-fix findings there are; a review without that line is not used.

use sbxm::task::review::{must_fix_count, with_header};

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
