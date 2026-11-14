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
    run_test("anything", 1, sbxm, fixture, max, fails).0
}

/// Runs the script for test file `name` with a fake `cargo` that lists `listed` tests for it,
/// then logs the starts as [`run_with`] does. Also gives every `cargo` command line, one per line.
fn run_test(
    name: &str,
    listed: u32,
    sbxm: u32,
    fixture: u32,
    max: Option<&str>,
    fails: bool,
) -> (Output, String) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    let args = tmp.path().join("cargo-args.log");
    fs::write(
        &cargo,
        "#!/bin/sh\n\
         echo \"$*\" >> \"$FAKE_ARGS\"\n\
         case \"$*\" in *--no-run*) exit 0;; esac\n\
         case \"$*\" in *--list*) i=0; while [ $i -lt \"$FAKE_LISTED\" ]; do echo \"t$i: test\"; i=$((i+1)); done; exit 0;; esac\n\
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
        .arg(name)
        .env("PATH", path)
        .env("FAKE_ARGS", &args)
        .env("FAKE_LISTED", listed.to_string())
        .env("FAKE_SBXM", sbxm.to_string())
        .env("FAKE_FIXTURE", fixture.to_string())
        .env("FAKE_FAIL", if fails { "1" } else { "0" });
    if let Some(max) = max {
        command.arg(max);
    }
    let out = command.output().unwrap();
    (out, fs::read_to_string(&args).unwrap_or_default())
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

// Issue #473 review (M-1): the tests are one binary, so a file is chosen by a name filter, which
// matches nothing for a name no file has; such a run must fail, never pass a limit.

#[test]
fn a_name_no_test_file_has_fails_before_the_run_and_never_passes_the_limit() {
    let (out, args) = run_test("no_such_test", 0, 0, 0, Some("0"), false);

    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        !stdout(&out).contains("within the limit"),
        "{}",
        stdout(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no_such_test"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !args
            .lines()
            .any(|line| !line.contains("--no-run") && !line.contains("--list")),
        "{args}"
    );
}

#[test]
fn a_file_of_the_main_binary_runs_its_module() {
    let (out, args) = run_test("task_merged", 3, 7, 3, Some("395"), false);

    assert!(out.status.success(), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("within the limit"),
        "{}",
        stdout(&out)
    );
    assert!(
        args.contains("--test main -- --list --format terse task_merged::"),
        "{args}"
    );
    assert!(
        args.lines()
            .any(|line| line == "test --test main task_merged::"),
        "{args}"
    );
}

#[test]
fn a_file_that_is_a_binary_of_its_own_runs_that_binary() {
    let (out, args) = run_test("hint", 2, 1, 1, Some("395"), false);

    assert!(out.status.success(), "{}", stdout(&out));
    assert!(
        args.lines().any(|line| line == "test --test hint"),
        "{args}"
    );
    assert!(!args.contains("--test main"), "{args}");
}
