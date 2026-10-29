//! PR 322 review round 5, S-1: once milestone 4's end-to-end check on the real cloud passed, the
//! status sources say so and link its record, and the plan says which `sbx` output was captured.

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text =
        fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const RECORD: &str = "sdlc/reviews/2026-10-27-m4-end-to-end.md";

#[test]
fn project_and_agents_say_the_end_to_end_check_passed_and_link_its_record() {
    for path in ["docs/project.md", "AGENTS.md"] {
        let text = read(path);
        for stale in [
            "not yet recorded",
            "the end-to-end check on the real cloud (slice 11) is pending",
            "**Next:** slice 11",
        ] {
            assert!(!text.contains(stale), "{path} still says {stale:?}");
        }
        assert!(text.contains(RECORD), "{path} doesn't link {RECORD}");
    }
    // Round 6, S-1: the state section is dated no earlier than the check it records.
    assert!(read("docs/project.md").contains("## State (as of 2026-10-27)"));
}

#[test]
fn the_plan_says_the_logged_out_message_was_captured_and_the_removal_error_was_not() {
    let plan = read("sdlc/milestone-4.md");

    for stale in [
        "two outputs are not captured",
        "Still open: the not-logged-in message",
    ] {
        assert!(!plan.contains(stale), "the plan still says {stale:?}");
    }
    assert!(plan.contains("sbx-cloud-not-logged-in.txt"));
    assert!(plan.contains("S14-worded"));
}
