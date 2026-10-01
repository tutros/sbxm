//! M2b slice 6: prompt templates (spec §10, decision 156): `{{name}}` placeholders, an unknown
//! one or one with no value is an error naming the template; the worker prompt is embedded and
//! a repo may override it with a file inside the repo.

use std::fs;

use sbxm::task::config::Prompts;
use sbxm::task::prompts::{self, Role};

#[test]
fn placeholders_are_replaced_by_their_values() {
    let text = prompts::render(
        "t",
        "Issue #{{number}} on {{branch}}.",
        &[("number", "41"), ("branch", "issue-41")],
    )
    .unwrap();
    assert_eq!(text, "Issue #41 on issue-41.");
}

#[test]
fn a_value_is_inserted_as_is_and_never_expanded_again() {
    let text = prompts::render(
        "t",
        "{{issue}} / {{number}}",
        &[
            ("issue", "mentions {{number}} and {{nope}}"),
            ("number", "7"),
        ],
    )
    .unwrap();
    assert_eq!(text, "mentions {{number}} and {{nope}} / 7");
}

#[test]
fn an_unknown_placeholder_is_an_error_naming_the_template() {
    let message = format!(
        "{:#}",
        prompts::render("worker.md", "Hello {{colour}}", &[("number", "1")]).unwrap_err()
    );
    assert!(
        message.contains("worker.md") && message.contains("colour"),
        "{message}"
    );
    assert!(message.contains("unknown"), "{message}");
}

#[test]
fn a_known_placeholder_with_no_value_is_an_error_naming_it() {
    let message = format!(
        "{:#}",
        prompts::render("fix.md", "Branch {{branch}}", &[("number", "1")]).unwrap_err()
    );
    assert!(
        message.contains("fix.md") && message.contains("branch"),
        "{message}"
    );
    assert!(message.contains("no value"), "{message}");
}

#[test]
fn an_unclosed_placeholder_is_an_error() {
    let message = format!(
        "{:#}",
        prompts::render("t.md", "oops {{number", &[("number", "1")]).unwrap_err()
    );
    assert!(
        message.contains("t.md") && message.contains("unclosed"),
        "{message}"
    );
}

#[test]
fn spaces_inside_the_braces_are_allowed() {
    assert_eq!(
        prompts::render("t", "{{ number }}", &[("number", "3")]).unwrap(),
        "3"
    );
}

#[test]
fn every_documented_name_is_known() {
    let all = [
        "issue",
        "number",
        "branch",
        "base",
        "repo",
        "gates_sandbox",
        "gates_host",
        "review_path",
        "previous_review_path",
    ];
    for name in all {
        let template = format!("{{{{{name}}}}}");
        assert_eq!(
            prompts::render("t", &template, &[(name, "v")]).unwrap(),
            "v",
            "{name}"
        );
    }
}

fn values() -> Vec<(&'static str, &'static str)> {
    vec![
        ("number", "41"),
        ("repo", "o/r"),
        ("branch", "issue-41"),
        ("base", "main"),
        ("gates_sandbox", "- `cargo test`"),
    ]
}

#[test]
fn the_embedded_worker_prompt_renders_with_the_worker_values() {
    let template = prompts::template(Role::Worker, &Prompts::default()).unwrap();

    let text = prompts::render(&template.name, &template.text, &values()).unwrap();

    assert!(
        text.contains("#41") && text.contains("o/r") && text.contains("issue-41"),
        "{text}"
    );
    assert!(
        text.contains("main") && text.contains("- `cargo test`"),
        "{text}"
    );
    assert!(
        text.contains(".sbxm-task/issue.md") && text.contains(".sbxm-task/result.md"),
        "{text}"
    );
    assert!(text.contains("Fixes #41"), "{text}");
    assert!(!text.contains("{{"), "{text}");
}

#[test]
fn the_worker_prompt_says_not_to_push_and_that_there_is_no_github_access() {
    let template = prompts::template(Role::Worker, &Prompts::default()).unwrap();
    let text = prompts::render(&template.name, &template.text, &values()).unwrap();
    assert!(text.contains("no GitHub access"), "{text}");
    assert!(text.to_lowercase().contains("don't push"), "{text}");
}

fn review_values() -> Vec<(&'static str, &'static str)> {
    vec![
        ("number", "41"),
        ("repo", "o/r"),
        ("branch", "issue-41"),
        ("base", "main"),
        ("gates_sandbox", "- `cargo test`"),
        ("review_path", ".sbxm-task/review.md"),
        ("previous_review_path", ".sbxm-task/previous-review.md"),
    ]
}

#[test]
fn the_embedded_reviewer_prompt_renders_and_forbids_changing_anything() {
    let template = prompts::template(Role::Reviewer, &Prompts::default()).unwrap();

    let text = prompts::render(&template.name, &template.text, &review_values()).unwrap();

    assert!(
        text.contains("issue-41") && text.contains("o/r") && text.contains("#41"),
        "{text}"
    );
    assert!(
        text.contains("origin/main..HEAD") && text.contains("origin/main...HEAD"),
        "{text}"
    );
    assert!(text.contains(".sbxm-task/issue.md"), "{text}");
    assert!(
        text.contains("don't edit tracked files, commit, push or file issues"),
        "{text}"
    );
    assert!(text.contains("no GitHub access"), "{text}");
    // The reviewer must write the file sbxm reads, with the count on the first line.
    assert!(text.contains(".sbxm-task/review.md"), "{text}");
    assert!(text.contains("Must-fix findings: <count>"), "{text}");
    assert!(text.contains("- `cargo test`"), "{text}");
    assert!(!text.contains("{{"), "{text}");
}

#[test]
fn the_reviewer_prompt_covers_a_re_review_through_the_previous_review_file() {
    let template = prompts::template(Role::Reviewer, &Prompts::default()).unwrap();
    let text = prompts::render(&template.name, &template.text, &review_values()).unwrap();
    assert!(text.contains(".sbxm-task/previous-review.md"), "{text}");
    assert!(text.contains("re-review"), "{text}");
}

#[test]
fn the_embedded_fix_prompt_renders_and_asks_for_a_review_section_in_result_md() {
    let template = prompts::template(Role::Fix, &Prompts::default()).unwrap();

    let text = prompts::render(&template.name, &template.text, &review_values()).unwrap();

    assert!(text.contains("#41") && text.contains("issue-41"), "{text}");
    assert!(text.contains(".sbxm-task/review.md"), "{text}");
    assert!(
        text.contains("must-fix") && text.contains("should-fix"),
        "{text}"
    );
    assert!(
        text.contains("Review\" section") && text.contains(".sbxm-task/result.md"),
        "{text}"
    );
    assert!(text.contains("- `cargo test`"), "{text}");
    assert!(
        text.to_lowercase()
            .contains("don't rewrite existing commits"),
        "{text}"
    );
    assert!(!text.contains("{{"), "{text}");
}

#[test]
fn reviewer_and_fix_overrides_are_read_from_their_own_keys() {
    let dir = tempfile::tempdir().unwrap();
    let reviewer = dir.path().join("r.md");
    let fix = dir.path().join("f.md");
    fs::write(&reviewer, "R {{number}}").unwrap();
    fs::write(&fix, "F {{number}}").unwrap();
    let config = Prompts {
        reviewer: Some(reviewer),
        fix: Some(fix),
        ..Prompts::default()
    };
    assert_eq!(
        prompts::template(Role::Reviewer, &config).unwrap().text,
        "R {{number}}"
    );
    assert_eq!(
        prompts::template(Role::Fix, &config).unwrap().text,
        "F {{number}}"
    );
    // The worker is unaffected by them.
    assert!(
        prompts::template(Role::Worker, &config)
            .unwrap()
            .name
            .ends_with("worker.md")
    );
}

#[test]
fn a_repo_override_replaces_the_embedded_template() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("my-worker.md");
    fs::write(&path, "Custom for #{{number}}").unwrap();
    let prompts_config = Prompts {
        worker: Some(path.clone()),
        ..Prompts::default()
    };

    let template = prompts::template(Role::Worker, &prompts_config).unwrap();

    assert_eq!(template.text, "Custom for #{{number}}");
    assert!(template.name.contains("my-worker.md"), "{}", template.name);
}

#[test]
fn an_override_that_cannot_be_read_is_an_error_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let prompts_config = Prompts {
        worker: Some(dir.path().join("missing.md")),
        ..Prompts::default()
    };
    let message = format!(
        "{:#}",
        prompts::template(Role::Worker, &prompts_config).unwrap_err()
    );
    assert!(message.contains("missing.md"), "{message}");
}
