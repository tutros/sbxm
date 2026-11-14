//! Issue #444: `scripts/count-git-starts.sh <test> [max]` prints the starts by caller and
//! subcommand, and with `max` fails when sbxm's own starts are over it (the stop line: 395 passes,
//! 396 does not). A fake `cargo` stands in for the test run, writing a known number of starts.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

/// Runs the script with a fake `cargo` that logs `sbxm` starts of `rev-parse` and `fixture`
/// starts of `commit`.
fn run(sbxm: u32, fixture: u32, max: Option<&str>) -> Output {
    run_with(sbxm, fixture, max, false)
}

/// Like [`run`]; `fails` makes the fake test run exit 1 after logging its starts.
fn run_with(sbxm: u32, fixture: u32, max: Option<&str>, fails: bool) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\n\
         case \"$*\" in *--no-run*) exit 0;; esac\n\
         i=0; while [ $i -lt \"$FAKE_SBXM\" ]; do echo \"sbxm rev-parse\" >> \"$COUNTLOG_GITSTARTS\"; i=$((i+1)); done\n\
         i=0; while [ $i -lt \"$FAKE_FIXTURE\" ]; do echo \"fixture commit\" >> \"$COUNTLOG_GITSTARTS\"; i=$((i+1)); done\n\
         [ \"$FAKE_FAIL\" = 1 ] && exit 1\n\
         exit 0\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/count-git-starts.sh");
    let mut command = Command::new("sh");
    command
        .arg(script)
        .arg("anything")
        .env("PATH", path)
        .env("FAKE_SBXM", sbxm.to_string())
        .env("FAKE_FIXTURE", fixture.to_string())
        .env("FAKE_FAIL", if fails { "1" } else { "0" });
    if let Some(max) = max {
        command.arg(max);
    }
    command.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_report_counts_starts_by_caller_and_by_subcommand() {
    let out = run(7, 3, None);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    assert!(text.contains("7 sbxm"), "{text}");
    assert!(text.contains("3 fixture"), "{text}");
    assert!(text.contains("7 sbxm rev-parse"), "{text}");
    assert!(text.contains("3 fixture commit"), "{text}");
}

#[test]
fn the_stop_line_395_passes() {
    let out = run(395, 5, Some("395"));

    assert!(out.status.success(), "{}", stdout(&out));
}

#[test]
fn the_stop_line_396_does_not_pass() {
    let out = run(396, 5, Some("395"));

    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("396"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_max_that_is_not_a_number_is_refused_before_anything_runs() {
    let out = run(1, 1, Some("lots"));

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("lots"));
}

#[test]
fn a_failing_test_run_fails_the_script_without_a_limit_and_still_prints_the_counts() {
    let out = run_with(7, 3, None, true);

    assert!(!out.status.success());
    assert!(stdout(&out).contains("7 sbxm"), "{}", stdout(&out));
}

#[test]
fn a_failing_test_run_never_passes_the_limit() {
    let out = run_with(1, 1, Some("395"), true);

    assert!(!out.status.success());
    assert!(
        !stdout(&out).contains("within the limit"),
        "{}",
        stdout(&out)
    );
}
