//! Issue 545 (#460 guardrail 1): every reviewer prompt asks for the `## Not run` section
//! `review.rs` parses, in the shape it reads, and tells the reviewer where to see what CI runs.

use sbxm::task::config::Prompts;
use sbxm::task::prompts::{self, Role};

const REVIEWERS: [Role; 4] = [
    Role::Reviewer,
    Role::ReviewerPr,
    Role::ReviewerPrFix,
    Role::ReviewerSpec,
];

#[test]
fn every_reviewer_prompt_asks_for_a_not_run_section() {
    for role in REVIEWERS {
        let text = prompts::template(role, &Prompts::default()).unwrap().text;

        for needle in [
            "## Not run",
            "- <check>: <why it couldn't run>",
            "- none",
            ".github/workflows/",
        ] {
            assert!(text.contains(needle), "{role:?} prompt lacks {needle}");
        }
    }
}
