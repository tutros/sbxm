//! PR 57 review, S-2: the hand-over documents must not say M2b's slice 13 is still to do, and the
//! PRD/spec experiment (decision 140) must have its recorded assessment.

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn agents_md_does_not_call_slice_13_pending() {
    let agents = read("AGENTS.md");

    for stale in ["built through slice 12 of 14", "left are slice 13"] {
        assert!(!agents.contains(stale), "AGENTS.md still says {stale:?}");
    }
    assert!(agents.contains("m2b-experiment-report.md"));
}

#[test]
fn the_prd_is_not_a_draft_and_the_experiment_has_a_report_and_a_decision() {
    assert!(!read("sdlc/prd-m2b.md").contains("Status: draft"));
    assert!(read("sdlc/m2b-experiment-report.md").contains("Options for the user"));
    assert!(read("sdlc/decisions.md").contains("164. **M2b PRD/spec experiment assessed"));
}

#[test]
fn the_spec_does_not_send_readers_to_the_deleted_pester_tests() {
    let spec = read("sdlc/spec-m2b.md");
    let details = spec
        .split("## Details to verify first")
        .nth(1)
        .expect("the spec has a details section");

    assert!(
        !details.contains("scripts/tests/"),
        "the live checklist names deleted files"
    );
    assert!(details.contains("tests/fixtures/review-findings"));
}

#[test]
fn decision_165_says_it_allows_factual_skill_updates_but_not_the_methodology() {
    let decisions = read("sdlc/decisions.md");
    let at = decisions.find("165. **").expect("decision 165 exists");

    let text = &decisions[at..];

    assert!(text.contains("factual command and path updates"));
    assert!(text.contains("methodology stays frozen"));
}

/// Review M-1 (issue #108): section 6's bundle commands name the task's own branch, so a
/// continued PR's bundle is created and fetched under that PR's branch, not `issue-N`.
#[test]
fn the_spec_bundle_commands_use_the_tasks_branch() {
    let spec = read("sdlc/spec-m2b.md");
    let section = spec
        .split("## 6. Git trust boundary")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("the spec has section 6");

    assert!(
        section.contains("bundle create <ws>/.sbxm-task/branch.bundle <task-branch> ^"),
        "bundle create does not name the task's branch"
    );
    assert!(
        section.contains("refs/heads/<task-branch>:refs/heads/<task-branch>"),
        "the fetch does not name the task's branch"
    );
    assert!(
        !section.contains("refs/heads/issue-N"),
        "a hard-coded issue-N ref is left"
    );
    assert!(section.contains("`continues.branch`"));
}
