//! Issue 695: every reviewer prompt names both lists, `## Not run` for the checks the repository
//! or the task names, and `## Out of reach` for the other checks its sandbox can never run, which
//! don't hold auto-merge.

use sbxm::task::config::Prompts;
use sbxm::task::prompts::{self, Role};

const REVIEWERS: [Role; 4] = [
    Role::Reviewer,
    Role::ReviewerPr,
    Role::ReviewerPrFix,
    Role::ReviewerSpec,
];

#[test]
fn every_reviewer_prompt_names_both_sections() {
    for role in REVIEWERS {
        let text = prompts::template(role, &Prompts::default()).unwrap().text;

        for needle in [
            "## Not run",
            "## Out of reach",
            "only if the repo or the task names it",
        ] {
            assert!(text.contains(needle), "{role:?} prompt lacks {needle}");
        }
    }
}

#[test]
fn the_out_of_reach_section_comes_after_not_run() {
    for role in REVIEWERS {
        // A Windows checkout embeds the prompt with CRLF: make it LF first, so the CRLF copy
        // isn't CR-CR-LF, then find the headings as whole lines in both copies.
        let text = prompts::template(role, &Prompts::default()).unwrap().text;
        let lf = text.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        for text in [lf, crlf] {
            let heading = |name: &str| text.lines().position(|line| line == name);
            let not_run = heading("## Not run").expect("a Not run heading");
            let out_of_reach = heading("## Out of reach").expect("an Out of reach heading");
            assert!(not_run < out_of_reach, "{role:?}");
        }
    }
}
