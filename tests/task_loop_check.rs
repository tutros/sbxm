//! Issue 511 (decision 307): `sbxm task loop --check` says for each issue whether the loop would
//! take it and prints the loop command, running nothing; and the loop itself passes over an
//! issue whose body still has a `## Open points` heading.

mod common;

use sbxm::task::queue::has_open_points;

// --- the `## Open points` heading ---

#[test]
fn an_open_points_heading_is_found() {
    assert!(has_open_points("Why\n\n## Open points\n\n1. A or B?\n"));
    assert!(has_open_points("## Open points\n"));
    assert!(has_open_points("## Open points  \r\n- a?\n"));
}

#[test]
fn an_answered_heading_and_no_open_points_is_not_open() {
    assert!(!has_open_points("Why\n\n## Answered (the user)\n\n1. A.\n"));
    assert!(!has_open_points(""));
}

#[test]
fn open_points_in_a_code_block_or_not_at_a_line_start_do_not_count() {
    assert!(!has_open_points(
        "Example:\n\n```md\n## Open points\n```\n\n## Answered\n"
    ));
    assert!(!has_open_points("~~~\n## Open points\n~~~\n"));
    assert!(!has_open_points(
        "readied issues rename `## Open points` to `## Answered`\n"
    ));
    assert!(!has_open_points("  see ## Open points\n"));
    assert!(!has_open_points("### Open points\n"));
    // After a closed fence, a heading counts again.
    assert!(has_open_points("```\nx\n```\n## Open points\n"));
}
