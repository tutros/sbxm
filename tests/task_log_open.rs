//! Issue 87: the opener the `sbxm task` commands use in `main` to log into the task folders,
//! with the real clock, the real record for the stage and this process's own header.

mod common;

use std::fs;

use common::task_fixture::fixture;
use sbxm::commands::task_log;
use sbxm::task::record;

#[test]
fn the_opened_log_starts_with_this_processs_header_and_then_the_lines() {
    let f = fixture();
    let dir = record::task_dir(&f.env.base_dir(), "issue-41");
    fs::create_dir_all(&dir).unwrap();

    let log = task_log::open(&f.env.config_dir(), vec!["issue-41".to_owned()], false).unwrap();
    log.lock().unwrap().line("hello");

    let text = fs::read_to_string(dir.join("run.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("# "), "{text}");
    assert!(
        lines[0].contains(
            &format!("| pid {} ", std::process::id())
                .trim_end()
                .to_owned()
        ),
        "{text}"
    );
    let sha = lines[0]
        .split("| sha256 ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    assert_eq!(sha.len(), 64, "{text}");
    assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()), "{text}");
    assert!(lines[1].ends_with("[-] hello"), "{text}");
}
