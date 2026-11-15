//! Issue 514: the loop reads a `**Depends on:**` (or `**Related:**`) field over every line it
//! spans, with the same field ends `file-findings` uses: a blank line, a line starting with `**`,
//! or one starting with `<!--`. The cases are the issue's boundary cases, in its order.

use std::collections::BTreeMap;

use sbxm::task::findings::update_link_fields;
use sbxm::task::select::field_numbers;

#[test]
fn task_select_fields_a_one_line_field_gives_its_numbers() {
    assert_eq!(
        field_numbers("x\n**Depends on:** #12, #34\n", "Depends on"),
        [12, 34]
    );
}

#[test]
fn task_select_fields_the_format_field_shape_is_read_to_the_blank_line() {
    let body = "x\n**Depends on:**\n#12\n- #34 (note)\n\n#78 later\n";
    assert_eq!(field_numbers(body, "Depends on"), [12, 34]);
}

#[test]
fn task_select_fields_a_multi_line_field_ends_at_the_next_bold_field() {
    let body = "**Depends on:**\n#12\n- #34\n**Related:** #56\n";
    assert_eq!(field_numbers(body, "Depends on"), [12, 34]);
    assert_eq!(field_numbers(body, "Related"), [56]);
}

#[test]
fn task_select_fields_a_field_ends_at_a_comment() {
    let body = "**Depends on:**\n#12\n<!-- review-finding: r.md#34 -->\n";
    assert_eq!(field_numbers(body, "Depends on"), [12]);
}

#[test]
fn task_select_fields_a_field_on_the_last_line_without_a_newline_is_read() {
    assert_eq!(field_numbers("x\n**Depends on:** #12", "Depends on"), [12]);
    assert_eq!(
        field_numbers("x\n**Depends on:**\n#12\n- #34", "Depends on"),
        [12, 34]
    );
}

#[test]
fn task_select_fields_crlf_reads_the_same_as_lf() {
    let lf = "x\n**Depends on:**\n#12\n- #34 (note)\n\n#78\n**Related:** #56\n<!-- #90 -->\n";
    let crlf = lf.replace('\n', "\r\n");
    assert_eq!(field_numbers(&crlf, "Depends on"), [12, 34]);
    assert_eq!(field_numbers(&crlf, "Related"), [56]);
    assert_eq!(
        field_numbers(&crlf, "Depends on"),
        field_numbers(lf, "Depends on")
    );
}

#[test]
fn task_select_fields_none_gives_no_numbers() {
    assert!(field_numbers("**Depends on:** none.\n\nSee #12.\n", "Depends on").is_empty());
}

#[test]
fn task_select_fields_no_field_gives_no_numbers() {
    assert!(field_numbers("Blocked by #12, see #34.\n", "Depends on").is_empty());
    assert!(field_numbers("", "Depends on").is_empty());
}

#[test]
fn task_select_fields_only_the_first_of_two_fields_is_read() {
    let body = "**Depends on:** #12\n\n**Depends on:** #34\n";
    assert_eq!(field_numbers(body, "Depends on"), [12]);
    let adjacent = "**Depends on:**\n#12\n**Depends on:** #34\n";
    assert_eq!(field_numbers(adjacent, "Depends on"), [12]);
}

#[test]
fn task_select_fields_file_findings_rewrites_the_lines_the_loop_reads() {
    let map = BTreeMap::from([("S-2".to_owned(), 41), ("S-3".to_owned(), 42)]);
    let body = "**Depends on:**\r\nS-2\r\n- S-3 (note)\r\n**Fix:** S-2\r\n";
    let patched = update_link_fields(body, &map);
    assert_eq!(
        patched,
        "**Depends on:**\r\n#41\r\n- #42 (note)\r\n**Fix:** S-2\r\n"
    );
    assert_eq!(field_numbers(&patched, "Depends on"), [41, 42]);
}
