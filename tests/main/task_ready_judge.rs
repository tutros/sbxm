//! Issue 726: `sbxm task ready-check --issue N --judge [--write-files [--yes]]`, decision 331(2):
//! the model's answer and its parse, the `**Files:**` line it proposes written into the body, the
//! embedded prompt, and the one-shot sandbox run with its clean-up.

use sbxm::task::ready_judge::{self, SECTIONS};

/// An answer with every section.
const FULL: &str = "files: `src/a.rs`, `src/b.rs` (new)\n\
open_points:\n\
- \"It may or may not be cached.\": which one?\n\
boundary_cases: needed\n\
- an empty list\n\
acceptance_vs_answers: none\n\
size: fits\n";

// The answer's parse.

#[test]
fn ready_judge_an_answer_with_every_section_parses() {
    let answer = ready_judge::parse(FULL).unwrap();
    assert_eq!(answer.files, "`src/a.rs`, `src/b.rs` (new)");
    assert_eq!(
        answer.open_points,
        "- \"It may or may not be cached.\": which one?"
    );
    assert_eq!(answer.boundary_cases, "needed\n- an empty list");
    assert_eq!(answer.acceptance_vs_answers, "none");
    assert_eq!(answer.size, "fits");
}

#[test]
fn ready_judge_text_before_the_first_section_and_fence_lines_are_left_out() {
    let text = format!("Here is my answer.\n```\n{FULL}```\n");
    let answer = ready_judge::parse(&text).unwrap();
    assert_eq!(answer.files, "`src/a.rs`, `src/b.rs` (new)");
    assert_eq!(answer.size, "fits");
}

#[test]
fn ready_judge_crlf_line_endings_parse_the_same() {
    let answer = ready_judge::parse(&FULL.replace('\n', "\r\n")).unwrap();
    assert_eq!(answer, ready_judge::parse(FULL).unwrap());
}

#[test]
fn ready_judge_an_answer_with_one_section_missing_does_not_parse() {
    let text = FULL.replace("size: fits\n", "");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("size"), "{why}");
}

#[test]
fn ready_judge_an_answer_not_in_the_format_does_not_parse() {
    let why = ready_judge::parse("The issue looks fine to me.").unwrap_err();
    for section in SECTIONS {
        assert!(why.contains(section), "{why}");
    }
}

#[test]
fn ready_judge_an_empty_section_does_not_parse() {
    let text = FULL.replace("acceptance_vs_answers: none", "acceptance_vs_answers:");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("acceptance_vs_answers"), "{why}");
}

#[test]
fn ready_judge_a_section_given_twice_does_not_parse() {
    let why = ready_judge::parse(&format!("{FULL}size: M\n")).unwrap_err();
    assert!(why.contains("size") && why.contains("twice"), "{why}");
}

#[test]
fn ready_judge_files_that_are_not_backticked_paths_do_not_parse() {
    let text = FULL.replace("`src/a.rs`, `src/b.rs` (new)", "src/a.rs and src/b.rs");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("files"), "{why}");
}

#[test]
fn ready_judge_files_on_two_lines_do_not_parse() {
    let text = FULL.replace("`src/b.rs` (new)", "\n`src/b.rs` (new)");
    let why = ready_judge::parse(&text).unwrap_err();
    assert!(why.contains("files"), "{why}");
}

#[test]
fn ready_judge_files_none_parses_and_proposes_no_line() {
    let text = FULL.replace("`src/a.rs`, `src/b.rs` (new)", "none");
    let answer = ready_judge::parse(&text).unwrap();
    assert_eq!(answer.files_line(), None);
}

#[test]
fn ready_judge_the_files_line_is_the_bold_label_and_the_paths() {
    let answer = ready_judge::parse(FULL).unwrap();
    assert_eq!(
        answer.files_line().as_deref(),
        Some("**Files:** `src/a.rs`, `src/b.rs` (new)")
    );
}

// The `**Files:**` line written into the body.

const LINE: &str = "**Files:** `src/a.rs`";

#[test]
fn ready_judge_a_body_with_no_files_line_gets_it_before_depends_on() {
    let body = "Text.\n\n**Depends on:** none\n\n## Acceptance criteria\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\n\n**Files:** `src/a.rs`\n\n**Depends on:** none\n\n## Acceptance criteria\n"
    );
}

#[test]
fn ready_judge_a_body_with_a_files_line_gets_it_replaced() {
    let body = "Text.\n\n**Files:** `old.rs`, `x.rs`\n\n**Depends on:** none\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\n\n**Files:** `src/a.rs`\n\n**Depends on:** none\n"
    );
}

#[test]
fn ready_judge_a_files_line_in_a_crlf_body_keeps_its_line_ending() {
    let body = "Text.\r\n**Files:** `old.rs`\r\n**Depends on:** none\r\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "Text.\r\n**Files:** `src/a.rs`\r\n**Depends on:** none\r\n"
    );
}

#[test]
fn ready_judge_lines_inside_a_code_fence_are_not_touched() {
    let body = "```\n**Files:** `x`\n**Depends on:** none\n```\n\n**Depends on:** #1\n";
    assert_eq!(
        ready_judge::with_files_line(body, LINE),
        "```\n**Files:** `x`\n**Depends on:** none\n```\n\n**Files:** `src/a.rs`\n\n**Depends on:** #1\n"
    );
}

#[test]
fn ready_judge_a_body_with_neither_line_gets_it_at_the_end() {
    assert_eq!(
        ready_judge::with_files_line("Text.", LINE),
        "Text.\n\n**Files:** `src/a.rs`\n"
    );
    assert_eq!(
        ready_judge::with_files_line("Text.\n", LINE),
        "Text.\n\n**Files:** `src/a.rs`\n"
    );
}

// The embedded prompt.

#[test]
fn ready_judge_the_prompt_asks_for_every_answer_section() {
    for section in SECTIONS {
        assert!(
            ready_judge::PROMPT.contains(&format!("{section}:")),
            "the prompt doesn't ask for `{section}:`"
        );
    }
}

#[test]
fn ready_judge_the_prompt_states_the_readiness_rules_and_names_its_inputs() {
    let prompt = ready_judge::PROMPT;
    for needle in [
        "boundary case",
        "**Files:**",
        ready_judge::ISSUE_FILE,
        ready_judge::PATHS_FILE,
        ready_judge::OPEN_PRS_FILE,
    ] {
        assert!(prompt.contains(needle), "the prompt doesn't name {needle}");
    }
}
