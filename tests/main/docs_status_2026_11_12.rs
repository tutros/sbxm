//! Issue #309: the status sections say what was merged by 2026-11-12, and the open-spikes list
//! marks the spikes that were run as done.

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

fn spike_line<'a>(spikes: &'a str, label: &str) -> &'a str {
    let start = spikes
        .find(&format!("- **{label}"))
        .unwrap_or_else(|| panic!("the spikes list has no {label}"));
    let rest = &spikes[start + 2..];
    &rest[..rest.find(" - **").unwrap_or(rest.len())]
}

#[test]
fn the_project_state_is_dated_2026_11_12_and_names_milestone_5() {
    let project = read("docs/project.md");
    let state = section(&project, "## State (as of 2026-11-12)");

    assert!(state.contains("milestone 5 is in progress"));
    assert!(state.contains("slices 1 and 2 merged as PRs #605 and #613"));
    assert!(state.contains("decision 316, #568"));
    assert!(state.contains("`[resources] cloud_slots`"));
    assert!(state.contains("labelled `run-cloud` to the cloud (#613)"));
    assert!(state.contains("(the last: `sdlc/retros/2026-11-11-retro.md`)"));
    assert!(state.contains("milestone 5's remaining slices (#561"));
    assert!(!state.contains("toasts when the user is needed"));
}

#[test]
fn the_spikes_that_were_run_read_as_done() {
    let decisions = read("sdlc/decisions.md");
    let spikes = section(&decisions, "## Open research spikes");

    let s9 = spike_line(spikes, "S9");
    assert!(s9.contains("Done"), "{s9}");
    assert!(s9.contains("`sdlc/spikes/S12.md`") && s9.contains("decision 241"));
    assert!(!s9.contains("Deferred with slice 14"), "{s9}");

    let s15 = spike_line(spikes, "S15");
    assert!(!s15.contains("not run yet"), "{s15}");
    assert!(s15.contains("works") && s15.contains("231"), "{s15}");

    assert!(spike_line(spikes, "S14").contains("`sdlc/spikes/S14.md`"));
    assert!(spike_line(spikes, "S16").contains("decision 270"));
    assert!(spike_line(spikes, "#199").contains("not run"));
}

#[test]
fn agents_md_names_milestone_5_as_in_progress() {
    let agents = read("AGENTS.md");
    let status = section(&agents, "## Status");

    assert!(status.contains("Milestones 1 to 4 are merged to `main`"));
    assert!(status.contains("Milestone 5 (the loop runs local and cloud tasks at once"));
    assert!(status.contains("is in progress"));
}
