//! Issue #691: milestone 5 is done; `docs/project.md`, the `AGENTS.md` status and the plan's checklist say so.

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text =
        fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    text.split(heading)
        .nth(1)
        .and_then(|rest| rest.split(" ## ").next())
        .unwrap_or_else(|| panic!("no section {heading:?}"))
}

#[test]
fn docs_status_m5_done_the_project_state_names_milestone_5_as_merged() {
    let project = read("docs/project.md");
    let state = section(&project, "## State (as of 2026-11-16)");

    assert!(state.contains(
        "4 (cloud sandboxes, PR #322) and 5 (the loop runs local and cloud tasks at once"
    ));
    assert!(state.contains("`sandbox_slots`, `cloud_slots`, children, adoption on restart"));
    assert!(!state.contains("milestone 5 is in progress"));
    assert!(!state.contains("milestone 5's remaining slices"));
}

#[test]
fn docs_status_m5_done_agents_md_names_milestones_1_to_5_as_merged() {
    let agents = read("AGENTS.md");
    let status = section(&agents, "## Status");

    assert!(status.contains("Milestones 1 to 5 are merged to `main`"));
    assert!(status.contains("M5 the loop runs local and cloud tasks at once)"));
    assert!(!status.contains("is in progress"));
    assert!(status.contains("`docs/project.md`") && status.contains("#405"));
}

#[test]
fn docs_status_m5_done_the_checklist_records_its_results() {
    let plan = read("sdlc/milestone-5.md");
    let checklist = section(&plan, "## 6. Manual end-to-end checklist");

    assert!(checklist.contains("**Results.** Steps 1, 2, 3, 5 and 6 passed"));
    for name in ["932b6c3", "ed2a3a5", "#676", "#680", "#683", "#681", "#561"] {
        assert!(checklist.contains(name), "the results miss {name}");
    }
    assert!(checklist.contains("Step 4 (`task loop --stop` during a run) passed on 2026-11-16"));
}
