//! Issue #632: a documentation-only change runs a smaller check in the worker. The worker prompt
//! and the `sdlc-implementation` skill say the same thing: every changed file is a `.md` file and
//! none is under `prompts/` (those are compiled into the binary), so the worker runs
//! `cargo fmt --check` and the `tests/main/docs_*.rs` tests; any other change runs the full gate list.

use std::fs;
use std::path::Path;

use sbxm::task::config::Prompts;
use sbxm::task::prompts::{self, Role};

const DOCS_TESTS: &str = "cargo test --test main docs_";

fn worker_prompt() -> String {
    let template = prompts::template(Role::Worker, &Prompts::default()).unwrap();
    let values = [
        ("number", "41"),
        ("repo", "o/r"),
        ("branch", "issue-41"),
        ("base", "main"),
        ("branch_context", "which started from main."),
        ("gates_sandbox", "- `cargo test`"),
        ("closing_line", "Fixes #41."),
    ];
    prompts::render(&template.name, &template.text, &values).unwrap()
}

fn skill() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".claude")
        .join("skills")
        .join("sdlc-implementation")
        .join("SKILL.md");
    fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

/// Joins the text's lines so a wrapped sentence can be matched as one.
fn unwrapped(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_states_the_docs_only_rule(name: &str, text: &str) {
    let text = unwrapped(text);
    for needle in [
        // Only `.md` files outside `prompts/` make a change documentation-only.
        "documentation-only",
        "every changed file is a `.md` file and none is under `prompts/`",
        // The smaller check, with a command that selects the docs test targets.
        "`cargo fmt --check`",
        DOCS_TESTS,
        // A `prompts/*.md` file or any non-`.md` file keeps the full gate list.
        "Any other change",
    ] {
        assert!(text.contains(needle), "{name} lacks {needle:?}:\n{text}");
    }
}

#[test]
fn the_worker_prompt_states_the_documentation_only_check() {
    assert_states_the_docs_only_rule("worker.md", &worker_prompt());
}

#[test]
fn the_sdlc_implementation_skill_states_the_same_documentation_only_check() {
    assert_states_the_docs_only_rule("SKILL.md", &skill());
}

#[test]
fn the_worker_prompt_still_gives_the_full_gate_list_for_any_other_change() {
    assert!(worker_prompt().contains("- `cargo test`"));
}
