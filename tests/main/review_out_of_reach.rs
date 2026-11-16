//! Issue 695: a review's `## Out of reach` section, the checks its sandbox can never run that
//! neither the repository nor the task names, is not part of the `## Not run` list the loop reads.

use sbxm::task::review::not_run;

const HEAD: &str = "Must-fix findings: 0\n\nRisk: low\n\nScope: `a..b`\n\n## Risk\n\n- docs\n\n";

fn review(sections: &str) -> String {
    format!("{HEAD}{sections}")
}

#[test]
fn bullets_are_not_checks_not_run() {
    let text = review(
        "## Not run\n\n- none\n\n## Out of reach\n\n- GitHub smoke: no GitHub access here\n",
    );
    assert_eq!(not_run(&text), Some(Vec::new()));
}

#[test]
fn a_not_run_bullet_is_listed_alone() {
    let text = review("## Not run\n\n- Pester tests: no pwsh\n\n## Out of reach\n\n- none\n");
    assert_eq!(
        not_run(&text),
        Some(vec!["Pester tests: no pwsh".to_owned()])
    );
}

#[test]
fn without_a_not_run_section_the_list_is_unknown() {
    let text = review("## Out of reach\n\n- GitHub smoke: no GitHub access here\n");
    assert_eq!(not_run(&text), None);
}

#[test]
fn before_not_run_is_still_left_out() {
    let text =
        review("## Out of reach\n\n- no sbx here\n\n## Not run\n\n- real_sbx_x: needs sbx login\n");
    assert_eq!(
        not_run(&text),
        Some(vec!["real_sbx_x: needs sbx login".to_owned()])
    );
}
