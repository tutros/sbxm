//! Issue 116 (migration step 3 of spec `sdlc/specs/task-state-machine.md` section 5.5): `sbxm task
//! states` prints the transition table from `src/task/machine.rs::TABLE`, and that same rendering
//! is checked against the spec's section 5.2 by `the_generated_table_matches_the_spec` below, so
//! the two cannot silently drift apart.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use sbxm::commands::task_states;

#[test]
fn starts_with_the_header_row() {
    let text = task_states::render();
    assert!(
        text.starts_with("| From | Event | To | Action |\n|---|---|---|---|\n"),
        "{text}"
    );
}

#[test]
fn has_one_row_per_table_entry() {
    let text = task_states::render();
    // The header is two lines (the titles, the `---` separator); every other line is one row.
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), sbxm::task::machine::TABLE.len() + 2, "{text}");
}

#[test]
fn names_both_kinds_where_a_row_is_not_for_both() {
    let text = task_states::render();
    assert!(text.contains("(issue)"), "{text}");
    assert!(text.contains("(pr)"), "{text}");
    assert!(text.contains("(issue/spec)"), "{text}");
}

#[test]
fn flags_an_interrupted_row() {
    let text = task_states::render();
    assert!(text.contains("interrupted"), "{text}");
}

#[test]
fn the_cli_command_prints_the_same_table() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(tmp.path())
        .env("SBXM_CONFIG_DIR", tmp.path().join("no-such-config"))
        .args(["task", "states"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout, task_states::render());
}

/// Pulls the "Today (generated)" table out of the spec's text. Line endings are normalised first
/// so a CRLF checkout (the project's primary platform is Windows) still finds the blank line that
/// ends the table; without it the table text runs to the end of the file and the `find("\n\n")`
/// below panics even when the table itself is correct.
fn generated_table_block(spec_source: &str) -> String {
    let spec = spec_source.replace("\r\n", "\n");
    let start = spec
        .find("#### Today (generated)")
        .expect("spec is missing the 'Today (generated)' block");
    let after_heading = &spec[start..];
    let table_start = after_heading
        .find("| From |")
        .expect("spec's generated block has no table");
    let table_text = &after_heading[table_start..];
    let table_end = table_text
        .find("\n\n")
        .expect("spec's generated table has no blank line after it");
    table_text[..table_end].to_string()
}

/// The spec names this test as where the table is checked (section 5.2). The generated table is
/// copied verbatim into the "Today (generated)" block; this test fails the moment the code's
/// `TABLE` and that block disagree, so the file cannot go stale.
#[test]
fn the_generated_table_matches_the_spec() {
    let spec = fs::read_to_string(Path::new("sdlc/specs/task-state-machine.md")).unwrap();
    let spec_table = generated_table_block(&spec);

    let generated = task_states::render();
    let generated_trimmed = generated.trim_end_matches('\n');

    assert_eq!(spec_table, generated_trimmed);
}

/// Reproduces issue 138: on a Windows checkout the spec file has CRLF line endings. `git
/// checkout-index` with `core.autocrlf=true` performs that same conversion, so this test exercises
/// genuinely CRLF file content rather than a synthetic one.
#[test]
fn the_generated_table_matches_the_spec_on_a_crlf_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "core.autocrlf=true",
            "checkout-index",
            &format!("--prefix={}/", tmp.path().display()),
            "--",
            "sdlc/specs/task-state-machine.md",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "git checkout-index failed");

    let crlf_spec =
        fs::read_to_string(tmp.path().join("sdlc/specs/task-state-machine.md")).unwrap();
    assert!(
        crlf_spec.contains("\r\n"),
        "checkout-index did not produce CRLF line endings"
    );

    let spec_table = generated_table_block(&crlf_spec);

    let generated = task_states::render();
    let generated_trimmed = generated.trim_end_matches('\n');

    assert_eq!(spec_table, generated_trimmed);
}

/// The CRLF fix must not blind the comparison: a spec whose table is missing a row still has to
/// fail it.
#[test]
#[should_panic]
fn the_generated_table_matches_the_spec_still_fails_when_a_row_is_missing() {
    let generated = task_states::render();
    let generated_trimmed = generated.trim_end_matches('\n');
    let lines: Vec<&str> = generated_trimmed.lines().collect();
    let table_missing_a_row = lines[..lines.len() - 1].join("\n");
    let spec_source = format!("#### Today (generated)\n\n{table_missing_a_row}\n\n");

    assert_eq!(generated_table_block(&spec_source), generated_trimmed);
}

/// Same as above, for a row whose content has drifted rather than being dropped entirely.
#[test]
#[should_panic]
fn the_generated_table_matches_the_spec_still_fails_when_a_row_is_different() {
    let generated = task_states::render();
    let generated_trimmed = generated.trim_end_matches('\n');
    let table_with_a_changed_row = generated_trimmed.replacen("interrupted", "changed", 1);
    let spec_source = format!("#### Today (generated)\n\n{table_with_a_changed_row}\n\n");

    assert_eq!(generated_table_block(&spec_source), generated_trimmed);
}
