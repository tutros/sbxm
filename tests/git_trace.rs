//! Issue 468: a test that rebuilds a template runs in a child process with git's trace
//! (`GIT_TRACE2_EVENT`) going to a file of its own, and its failure ends with the trace's last
//! lines (`common::git_trace`). One test per boundary case.

mod common;

use std::fs;
use std::path::Path;

use common::git_trace::{self, TRACE_LINES, run_traced, trace_tail};

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn tail_of(text: Option<&str>) -> String {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.json");
    if let Some(text) = text {
        fs::write(&trace, text).unwrap();
    }
    trace_tail(&trace)
}

#[test]
fn a_trace_longer_than_the_limit_shows_exactly_its_last_lines() {
    let tail = tail_of(Some(&lines(TRACE_LINES + 1)));

    let shown: Vec<&str> = tail.lines().collect();
    assert_eq!(shown.len(), TRACE_LINES);
    assert_eq!(shown[0], "line 2");
    assert_eq!(shown[TRACE_LINES - 1], format!("line {}", TRACE_LINES + 1));
}

#[test]
fn a_much_longer_trace_shows_exactly_its_last_lines() {
    let tail = tail_of(Some(&lines(10 * TRACE_LINES)));

    let shown: Vec<&str> = tail.lines().collect();
    assert_eq!(shown.len(), TRACE_LINES);
    assert_eq!(shown[0], format!("line {}", 9 * TRACE_LINES + 1));
}

#[test]
fn a_trace_at_the_limit_is_shown_whole() {
    assert_eq!(
        tail_of(Some(&lines(TRACE_LINES))).lines().count(),
        TRACE_LINES
    );
}

#[test]
fn a_trace_under_the_limit_is_shown_whole() {
    let tail = tail_of(Some(&lines(TRACE_LINES - 1)));

    assert_eq!(tail.lines().count(), TRACE_LINES - 1);
    assert!(tail.starts_with("line 1\n"), "{tail}");
}

#[test]
fn a_missing_trace_file_shows_no_trace_written() {
    assert_eq!(tail_of(None), "no trace written");
}

#[test]
fn an_empty_trace_file_shows_no_trace_written() {
    assert_eq!(tail_of(Some("")), "no trace written");
    assert_eq!(tail_of(Some("\n\n")), "no trace written");
}

/// Runs a git call through sbxm, then fails or not: the child of the two tests below.
fn clone_through_sbxm(force_failure: bool) {
    let root = tempfile::tempdir().unwrap();
    let origin = common::git_origin(root.path());
    if force_failure {
        // A symlink among the source's objects: `git clone --local` refuses to copy it.
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path().join("nowhere"),
            origin.join("objects").join("info").join("forced-failure"),
        )
        .unwrap();
    }
    let dest = root.path().join("checkout");

    sbxm::task::repo::clean_checkout(&origin, "main", &dest).unwrap();
}

#[cfg(unix)]
#[test]
fn a_forced_clone_failure_fails_with_the_traces_last_lines() {
    if git_trace::in_traced_run() {
        clone_through_sbxm(true);
        return;
    }

    let failure = run_traced("a_forced_clone_failure_fails_with_the_traces_last_lines")
        .expect_err("the forced clone failure fails");

    assert!(failure.contains("refusing to clone"), "{failure}");
    let (_, trace) = failure
        .split_once("the last lines of git's trace")
        .expect(&failure);
    assert!(trace.contains(r#""event":"start""#), "{trace}");
    assert!(trace.contains("clone"), "{trace}");
    assert!(!trace.contains("no trace written"), "{trace}");
}

#[test]
fn a_passing_traced_run_prints_no_trace() {
    if git_trace::in_traced_run() {
        clone_through_sbxm(false);
        return;
    }

    assert_eq!(run_traced("a_passing_traced_run_prints_no_trace"), Ok(()));
}

#[test]
fn a_traced_run_of_a_test_that_does_not_exist_fails() {
    let failure = run_traced("no_such_test").expect_err("no test ran");

    assert!(failure.contains("no_such_test"), "{failure}");
}

#[test]
fn a_traced_run_gets_a_trace_file_of_its_own() {
    if git_trace::in_traced_run() {
        let trace = std::env::var_os("GIT_TRACE2_EVENT").expect("set for the child");
        assert!(Path::new(&trace).is_absolute(), "{trace:?}");
        clone_through_sbxm(false);
        assert!(Path::new(&trace).is_file(), "{trace:?}");
        return;
    }

    assert_eq!(
        run_traced("a_traced_run_gets_a_trace_file_of_its_own"),
        Ok(())
    );
}
