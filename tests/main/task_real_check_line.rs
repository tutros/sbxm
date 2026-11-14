//! Issue 547 (#460 guardrail 3): an issue names the real check its PR needs (decision 273) with a
//! body line that starts with `**Real check:**`, outside a code fence, read the way `task finish`
//! reads `**Closes on merge:** no`. No such line, or the value `none` in any case, means none.

use sbxm::task::real_check::required;

/// `gh issue view` output with `body` under its header.
fn issue(body: &str) -> String {
    format!("title:\tFix 41\nstate:\tOPEN\nlabels:\tsize:M\n--\n{body}")
}

#[test]
fn the_line_names_the_check() {
    let text = issue(
        "**Size:** M\n\n**Real check:** run the Windows tests 50 times; a pass is 50 green \
         runs\n\nDo it.\n",
    );
    assert_eq!(
        required(&text).as_deref(),
        Some("run the Windows tests 50 times; a pass is 50 green runs")
    );
}

#[test]
fn no_line_needs_no_check() {
    assert_eq!(required(&issue("**Size:** S\n\nDo it.\n")), None);
    assert_eq!(required(""), None);
}

#[test]
fn none_in_any_case_needs_no_check() {
    for value in ["none", "NONE", "None", "  none  "] {
        let text = issue(&format!("**Real check:** {value}\n"));
        assert_eq!(required(&text), None, "{value}");
    }
}

#[test]
fn the_line_inside_a_code_fence_is_ignored() {
    let text = issue("Write it like this:\n\n```md\n**Real check:** run it\n```\n\nDo it.\n");
    assert_eq!(required(&text), None);
    let tilde = issue("~~~\n**Real check:** run it\n~~~\n");
    assert_eq!(required(&tilde), None);
}

#[test]
fn the_line_not_at_the_start_of_a_line_is_ignored() {
    for body in [
        "Add a line like **Real check:** run it.\n",
        " **Real check:** run it\n",
        "- **Real check:** run it\n",
    ] {
        assert_eq!(required(&issue(body)), None, "{body:?}");
    }
}

#[test]
fn a_header_line_like_it_is_not_the_body() {
    let text = "title:\tFix\n**Real check:** run it\n--\nbody\n";
    assert_eq!(required(text), None);
}

#[test]
fn a_crlf_line_and_no_space_after_the_label_still_count() {
    assert_eq!(
        required(&issue("**Real check:**run it\r\nDo it.\r\n")).as_deref(),
        Some("run it")
    );
}

#[test]
fn an_empty_value_takes_the_next_line_as_the_checks_first_line() {
    let text = issue("**Real check:**\n\n- run the Windows tests 50 times\n- a pass is 50 green\n");
    assert_eq!(
        required(&text).as_deref(),
        Some("run the Windows tests 50 times")
    );
}

#[test]
fn an_empty_value_with_nothing_after_it_is_still_required() {
    assert_eq!(
        required(&issue("**Real check:**\n")).as_deref(),
        Some("(see the issue)")
    );
}

#[test]
fn the_last_line_counts() {
    let text = issue("**Real check:** run it\n\n**Real check:** none\n");
    assert_eq!(required(&text), None);
    let text = issue("**Real check:** none\n\n**Real check:** run it\n");
    assert_eq!(required(&text).as_deref(), Some("run it"));
}
