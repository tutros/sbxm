//! Issue 545 (#460 guardrail 1): a review's `## Not run` section, the checks neither the reviewer
//! nor CI ran, is read into a list; a missing or empty section is unknown (`None`).

use sbxm::task::review::{not_run, saved_not_run};

const HEAD: &str = "Must-fix findings: 0\n\nRisk: low\n\nScope: `a..b`\n\n## Risk\n\n- docs\n\n";

fn review(section: &str) -> String {
    format!("{HEAD}{section}")
}

#[test]
fn a_single_none_bullet_is_an_empty_list() {
    let text = review("## Not run\n\n- none\n");
    assert_eq!(not_run(&text), Some(Vec::new()));
}

#[test]
fn none_in_another_case_is_an_empty_list_too() {
    let text = review("## Not run\n\n- None\n");
    assert_eq!(not_run(&text), Some(Vec::new()));
}

#[test]
fn one_bullet_is_one_check_not_run() {
    let text = review("## Not run\n\n- Pester tests: no pwsh in the sandbox\n");
    assert_eq!(
        not_run(&text),
        Some(vec!["Pester tests: no pwsh in the sandbox".to_owned()])
    );
}

#[test]
fn several_bullets_are_listed_in_order() {
    let text = review(
        "## Not run\n\n- Pester tests: no pwsh\n* real_sbx_creates: needs sbx login\n- third: why\n",
    );
    assert_eq!(
        not_run(&text),
        Some(vec![
            "Pester tests: no pwsh".to_owned(),
            "real_sbx_creates: needs sbx login".to_owned(),
            "third: why".to_owned(),
        ])
    );
}

#[test]
fn a_missing_section_is_unknown() {
    assert_eq!(not_run(&review("## Summary\n\nAll good.\n")), None);
}

#[test]
fn a_section_with_no_bullets_is_unknown() {
    assert_eq!(not_run(&review("## Not run\n\nNothing to say.\n")), None);
    assert_eq!(not_run(&review("## Not run\n")), None);
}

#[test]
fn none_next_to_another_bullet_lists_the_other() {
    let text = review("## Not run\n\n- none\n- Pester tests: no pwsh\n");
    assert_eq!(
        not_run(&text),
        Some(vec!["Pester tests: no pwsh".to_owned()])
    );
}

#[test]
fn bullets_after_the_next_heading_are_not_read() {
    let text = review("## Not run\n\n- none\n\n## Nits\n\n- Nit: a typo\n");
    assert_eq!(not_run(&text), Some(Vec::new()));
    let empty_then_bullets = review("## Not run\n\n## Nits\n\n- Nit: a typo\n");
    assert_eq!(not_run(&empty_then_bullets), None);
}

#[test]
fn a_heading_inside_a_code_fence_is_not_the_section() {
    let text = review("## Summary\n\n```\n## Not run\n- none\n```\n");
    assert_eq!(not_run(&text), None);
}

#[test]
fn a_saved_review_is_read_past_its_reviewer_line() {
    let saved = format!(
        "Reviewer: claude (default model)\n\n{}",
        review("## Not run\n\n- x: y\n")
    );
    assert_eq!(saved_not_run(&saved), Some(vec!["x: y".to_owned()]));
}
