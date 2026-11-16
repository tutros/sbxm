//! Issue 693: `common::file_tail`, the helper the flaky tests of #104 print in their failure
//! messages, so the next flake names its cause.

use crate::common;

use common::file_tail;

#[test]
fn file_tail_of_a_missing_file_says_so_instead_of_panicking() {
    let dir = tempfile::TempDir::new().unwrap();

    assert_eq!(file_tail(&dir.path().join("run.log"), 40), "no run.log");
}

#[test]
fn file_tail_keeps_only_the_last_lines() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("run.log");
    let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&path, text).unwrap();

    let tail = file_tail(&path, 40);

    let lines: Vec<&str> = tail.lines().collect();
    assert_eq!(lines.len(), 40, "{tail}");
    assert_eq!((lines[0], lines[39]), ("line 11", "line 50"));
}

#[test]
fn file_tail_of_a_short_file_is_the_whole_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("run.log");
    std::fs::write(&path, "one\r\ntwo\r\n").unwrap();

    assert_eq!(file_tail(&path, 40), "one\ntwo");
}
