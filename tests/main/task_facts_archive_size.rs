//! Issue 698: `sbxm task facts` reads a task's size from the `issue.md` in the archive as well
//! as in a live task, since `task rm` now archives it. An archive made before that has none, so
//! its size stays `null`; facts never asks GitHub. The boundary cases are the issue's, in order
//! (the `task rm` one is in `task_rm_archive.rs`).

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};

use common::Env;
use sbxm::commands::task_facts::{self, Options};
use sbxm::run::results::parse_timestamp;
use sbxm::task::merged;
use sbxm::task::record::{self, Kind, NewTask, Process, Record};
use serde_json::Value;

const STARTED: &str = "2026-11-10T08:00:00Z";
const SIZED_M: &str = "title:\tt\nstate:\tOPEN\nlabels:\tenhancement, size:M\r\n--\nbody\n";

fn task(kind: Kind, number: u32, id: Option<&str>) -> Record {
    let at = parse_timestamp(STARTED).unwrap();
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title: "title",
            base: "main",
            branch: "b",
            config_hash: "h",
            id,
        },
        at,
        Process::new(1, at),
    )
}

fn save(dir: PathBuf, record: &Record, issue_md: Option<&str>) -> PathBuf {
    record::write(&dir, record).unwrap();
    if let Some(text) = issue_md {
        fs::write(dir.join("issue.md"), text).unwrap();
    }
    dir
}

fn archived(env: &Env, record: &Record, issue_md: Option<&str>) -> PathBuf {
    let dir = merged::archive_root(&env.base_dir()).join(&record.id);
    save(dir, record, issue_md)
}

fn live(env: &Env, record: &Record, issue_md: Option<&str>) -> PathBuf {
    let dir = record::task_dir(&env.base_dir(), &record.id);
    save(dir, record, issue_md)
}

/// The `size` of every row, in order.
fn sizes(env: &Env) -> Vec<Value> {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let opts = Options {
        since: "2026-11-10".to_owned(),
        until: Some("2026-11-10".to_owned()),
        json: true,
    };
    let now = parse_timestamp("2026-11-15T12:00:00Z").unwrap();
    task_facts::run(&env.config_dir(), &opts, now, &mut out, &mut warn).unwrap();
    assert_eq!(String::from_utf8(warn).unwrap(), "");
    let rows: Value = serde_json::from_slice(&out).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r["size"].clone())
        .collect()
}

fn assert_no_issue_md(dir: &Path) {
    assert!(!dir.join("issue.md").exists());
}

// 1 of 5
#[test]
fn task_facts_archive_size_reads_an_archived_issue_md() {
    let env = Env::new();
    archived(&env, &task(Kind::Issue, 7, None), Some(SIZED_M));

    assert_eq!(sizes(&env), ["M"]);
}

// 2 of 5
#[test]
fn task_facts_archive_size_of_an_old_archive_without_issue_md_is_null() {
    let env = Env::new();
    let dir = archived(&env, &task(Kind::Issue, 7, None), None);
    assert_no_issue_md(&dir);

    assert_eq!(sizes(&env), [Value::Null]);
}

// 3 of 5
#[test]
fn task_facts_archive_size_of_a_live_task_is_unchanged() {
    let env = Env::new();
    live(
        &env,
        &task(Kind::Issue, 7, None),
        Some("title:\tt\nlabels:\tsize:S\n--\n"),
    );

    assert_eq!(sizes(&env), ["S"]);
}

// 4 of 5
#[test]
fn task_facts_archive_size_of_an_issue_md_without_a_size_label_is_null() {
    let env = Env::new();
    let unsized_md = "title:\tt\nlabels:\tenhancement\n--\nsize:L in the body\n";
    archived(&env, &task(Kind::Issue, 7, None), Some(unsized_md));
    live(&env, &task(Kind::Issue, 8, None), Some(unsized_md));

    assert_eq!(sizes(&env), [Value::Null, Value::Null]);
}

// 5 of 5
#[test]
fn task_facts_archive_size_of_a_spec_task_is_null_even_with_an_issue_md() {
    let env = Env::new();
    // An `issue.md` a spec task never has, there only to show it isn't read.
    archived(
        &env,
        &task(Kind::Spec, 0, Some("spec-idea-1a2b3c")),
        Some(SIZED_M),
    );
    live(
        &env,
        &task(Kind::Spec, 0, Some("spec-other-4d5e6f")),
        Some(SIZED_M),
    );

    assert_eq!(sizes(&env), [Value::Null, Value::Null]);
}
