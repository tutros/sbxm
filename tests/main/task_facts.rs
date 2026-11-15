//! `sbxm task facts --since <date> [--until <date>] [--json]` (issue 593): a retro's per-task
//! facts table, read from the task records and the archive. Read-only. The boundary cases are
//! the issue's eight, in its order.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::Env;
use sbxm::commands::task_facts::{self, Options};
use sbxm::run::results::parse_timestamp;
use sbxm::task::merged;
use sbxm::task::record::{
    self, Agent, Kind, NewTask, Process, Record, RunInfo, Stage, Stamp, Status, Stopped,
};
use serde_json::{Value, json};

const HEADER: &str = "| task | issue | started | size | review rounds | must-fix per round | \
                      wall to ready (s) | worker (s) | worker cost | reviewer (s) | \
                      reviewer cost | outcome | PR |\n\
                      |---|---|---|---|---|---|---|---|---|---|---|---|---|\n";

/// Seconds since the epoch of a stamp written as sbxm writes them.
fn at(stamp: &str) -> u64 {
    parse_timestamp(stamp).unwrap()
}

/// An issue task started at `started`, prepared and nothing more.
fn issue_task(number: u32, started: &str) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: "o/r",
            title: "title",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        at(started),
        Process::new(1, at(started)),
    )
}

fn save(dir: &Path, record: &Record) -> PathBuf {
    record::write(dir, record).unwrap();
    dir.to_path_buf()
}

fn save_live(env: &Env, record: &Record) -> PathBuf {
    save(&record::task_dir(&env.base_dir(), &record.id), record)
}

fn save_archived(env: &Env, folder: &str, record: &Record) -> PathBuf {
    save(&merged::archive_root(&env.base_dir()).join(folder), record)
}

fn agent(duration_s: u64, usage: Value) -> Agent {
    Agent {
        harness: "claude".to_owned(),
        model: None,
        sandbox: "s".to_owned(),
        workspace: "w".to_owned(),
        profile: None,
        cpus: None,
        memory: None,
        template: None,
        run: Some(RunInfo {
            status: "completed".to_owned(),
            usage,
            duration_s,
        }),
        session: None,
    }
}

fn opts(since: &str, until: Option<&str>, json: bool) -> Options {
    Options {
        since: since.to_owned(),
        until: until.map(str::to_owned),
        json,
    }
}

/// Runs the command: its result, what it printed and what it warned.
/// The time the tests run "now": a window without `--until` ends here.
const NOW: &str = "2026-11-15T12:00:00Z";

fn facts(env: &Env, opts: &Options) -> (anyhow::Result<()>, String, String) {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_facts::run(&env.config_dir(), opts, at(NOW), &mut out, &mut warn);
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap(),
    )
}

fn json_rows(env: &Env, since: &str, until: Option<&str>) -> Vec<Value> {
    let (result, out, _) = facts(env, &opts(since, until, true));
    result.unwrap();
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn ids(rows: &[Value]) -> Vec<&str> {
    rows.iter().map(|r| r["task"].as_str().unwrap()).collect()
}

/// Every file under `dir`, with its bytes, so a test can show nothing was written.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

// 1 of 8
#[test]
fn task_facts_includes_a_task_started_on_since_and_not_one_the_day_before() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-09T23:59:59Z"));
    save_live(&env, &issue_task(2, "2026-11-10T00:00:00Z"));

    let rows = json_rows(&env, "2026-11-10", None);

    assert_eq!(ids(&rows), ["issue-2"]);
}

// 2 of 8
#[test]
fn task_facts_includes_a_task_started_on_until_and_not_one_the_day_after() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-12T23:59:59Z"));
    save_live(&env, &issue_task(2, "2026-11-13T00:00:00Z"));

    let rows = json_rows(&env, "2026-11-10", Some("2026-11-12"));

    assert_eq!(ids(&rows), ["issue-1"]);
}

// 3 of 8
#[test]
fn task_facts_without_until_lists_everything_from_since_to_now_in_start_order() {
    let env = Env::new();
    save_live(&env, &issue_task(3, NOW));
    save_live(&env, &issue_task(1, "2026-11-10T08:00:00Z"));
    save_archived(&env, "issue-2", &issue_task(2, "2026-11-12T00:00:00Z"));
    // Started after now: one second over, later the same day, and next year.
    save_live(&env, &issue_task(4, "2026-11-15T12:00:01Z"));
    save_live(&env, &issue_task(5, "2026-11-15T23:59:59Z"));
    save_archived(&env, "issue-6", &issue_task(6, "2027-06-01T00:00:00Z"));

    let rows = json_rows(&env, "2026-11-10", None);
    assert_eq!(ids(&rows), ["issue-1", "issue-2", "issue-3"]);

    let (result, out, _) = facts(&env, &opts("2026-11-10", None, false));
    result.unwrap();
    let tasks: Vec<&str> = out
        .lines()
        .skip(2)
        .map(|l| l.split(" | ").next().unwrap().trim_start_matches("| "))
        .collect();
    assert_eq!(tasks, ["issue-1", "issue-2", "issue-3"]);

    // An explicit --until is a whole day, whatever the time now.
    let rows = json_rows(&env, "2026-11-10", Some("2026-11-15"));
    assert_eq!(
        ids(&rows),
        ["issue-1", "issue-2", "issue-3", "issue-4", "issue-5"]
    );
}

// 4 of 8
#[test]
fn task_facts_lists_a_task_in_both_tasks_and_the_archive_once_from_its_live_record() {
    let env = Env::new();
    let mut task = issue_task(4, "2026-11-10T08:00:00Z");
    save_archived(&env, "issue-4", &task);
    task.pr = Some("https://github.com/o/r/pull/9".to_owned());
    save_live(&env, &task);
    // The same issue's earlier task, archived and removed: a task of its own.
    save_archived(&env, "issue-4-2", &issue_task(4, "2026-11-10T06:00:00Z"));

    let rows = json_rows(&env, "2026-11-10", None);

    assert_eq!(ids(&rows), ["issue-4", "issue-4"]);
    assert_eq!(rows[0]["pr"], Value::Null);
    assert_eq!(rows[1]["pr"], "https://github.com/o/r/pull/9");
}

// 5 of 8
#[test]
fn task_facts_prints_an_old_record_missing_newer_fields_with_dashes_and_nulls() {
    let env = Env::new();
    let dir = record::task_dir(&env.base_dir(), "issue-5");
    fs::create_dir_all(&dir).unwrap();
    // A record from before titles, rounds, reviews, PRs and runs were kept.
    let old = json!({
        "schema": 1,
        "id": "issue-5",
        "kind": "issue",
        "number": 5,
        "repo": "o/r",
        "base": "main",
        "branch": "issue-5",
        "stage": "working",
        "status": "running",
        "stages": [{"stage": "prepared", "at": "2026-11-10T08:00:00Z"}],
        "process": null,
        "worker": null,
        "reviewer": null,
        "gates": [],
        "related": [],
        "hooks": {},
        "sbxm_version": "0.1.0",
        "config_hash": "h"
    });
    fs::write(dir.join("task.json"), old.to_string()).unwrap();

    let rows = json_rows(&env, "2026-11-10", None);
    assert_eq!(
        rows,
        [json!({
            "task": "issue-5",
            "issue": 5,
            "started": "2026-11-10T08:00:00Z",
            "size": null,
            "review_rounds": 0,
            "must_fix": [],
            "wall_to_ready_s": null,
            "worker_s": null,
            "worker_cost_usd": null,
            "reviewer_s": null,
            "reviewer_cost_usd": null,
            "outcome": "working running",
            "pr": null
        })]
    );

    let (result, out, _) = facts(&env, &opts("2026-11-10", None, false));
    result.unwrap();
    assert_eq!(
        out,
        format!(
            "{HEADER}| issue-5 | 5 | 2026-11-10T08:00:00Z | - | 0 | - | - | - | - | - | - | \
             working running | - |\n"
        )
    );
}

// 6 of 8
#[test]
fn task_facts_with_no_task_in_the_window_prints_the_header_alone_or_an_empty_array() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-01T00:00:00Z"));

    let (result, out, warn) = facts(&env, &opts("2026-11-10", None, false));
    result.unwrap();
    assert_eq!(out, HEADER);
    assert_eq!(warn, "");

    let (result, out, _) = facts(&env, &opts("2026-11-10", None, true));
    result.unwrap();
    assert_eq!(out, "[]\n");
}

#[test]
fn task_facts_with_no_task_folders_at_all_prints_an_empty_array() {
    let env = Env::new();

    let (result, out, _) = facts(&env, &opts("2026-11-10", None, true));

    result.unwrap();
    assert_eq!(out, "[]\n");
}

// 7 of 8
#[test]
fn task_facts_warns_once_naming_a_malformed_task_json_and_prints_the_other_rows() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-10T08:00:00Z"));
    let live_bad = record::task_dir(&env.base_dir(), "issue-2");
    fs::create_dir_all(&live_bad).unwrap();
    fs::write(live_bad.join("task.json"), "{ not json").unwrap();
    let archived_bad = merged::archive_root(&env.base_dir()).join("issue-3");
    fs::create_dir_all(&archived_bad).unwrap();
    fs::write(archived_bad.join("task.json"), "{\"schema\": 1}").unwrap();

    let (result, out, warn) = facts(&env, &opts("2026-11-10", None, true));

    result.unwrap();
    let rows: Vec<Value> = serde_json::from_str(&out).unwrap();
    assert_eq!(ids(&rows), ["issue-1"]);
    let lines: Vec<&str> = warn.lines().collect();
    assert_eq!(lines.len(), 2, "{warn}");
    let live_path = live_bad.join("task.json").display().to_string();
    let archived_path = archived_bad.join("task.json").display().to_string();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("warning: ") && l.contains(&live_path)),
        "{warn}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("warning: ") && l.contains(&archived_path)),
        "{warn}"
    );
}

// 8 of 8
#[test]
fn task_facts_refuses_since_after_until_and_bad_dates() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-10T08:00:00Z"));
    let cases = [
        ("2026-11-12", Some("2026-11-11"), "after --until"),
        ("2026-11-1", None, "2026-11-1"),
        ("2026/11/10", None, "2026/11/10"),
        ("2026-02-30", None, "2026-02-30"),
        ("2026-13-01", None, "2026-13-01"),
        ("yesterday", None, "yesterday"),
        ("2026-11-10", Some("2026-11-31"), "2026-11-31"),
    ];
    for (since, until, named) in cases {
        let (result, out, _) = facts(&env, &opts(since, until, false));
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains(named), "{since} {until:?}: {message}");
        assert_eq!(out, "", "{since} {until:?}");
    }
    // A leap day is a date; since == until is a one-day window.
    assert!(
        facts(&env, &opts("2028-02-29", Some("2028-02-29"), false))
            .0
            .is_ok()
    );
}

#[test]
fn task_facts_fills_every_column_from_the_record_the_reviews_and_the_issue() {
    let env = Env::new();
    let mut task = issue_task(7, "2026-11-10T08:00:00Z");
    task.stages.extend([
        Stamp {
            stage: Stage::Working,
            at: "2026-11-10T08:00:05Z".to_owned(),
        },
        Stamp {
            stage: Stage::Ready,
            at: "2026-11-10T09:00:00Z".to_owned(),
        },
        // A later fix round and a second ready: the first ready counts.
        Stamp {
            stage: Stage::Fixing,
            at: "2026-11-10T10:00:00Z".to_owned(),
        },
        Stamp {
            stage: Stage::Ready,
            at: "2026-11-10T11:00:00Z".to_owned(),
        },
    ]);
    task.stage = Stage::Ready;
    task.status = Status::Ok;
    task.stopped = Some(Stopped::RoundsExhausted);
    task.worker = Some(agent(1200, json!({"cost_usd": 1.5})));
    task.reviewer = Some(agent(300, json!({})));
    task.pr = Some("https://github.com/o/r/pull/12".to_owned());
    let dir = save_live(&env, &task);
    fs::write(
        dir.join("review-1.md"),
        "Reviewer: claude (default model)\n\nMust-fix findings: 2\n",
    )
    .unwrap();
    fs::write(dir.join("review-2.md"), "no count here\n").unwrap();
    fs::write(dir.join("review-3.md"), "Must-fix findings: 0\r\n").unwrap();
    fs::write(
        dir.join("issue.md"),
        "title:\tt\nstate:\tOPEN\nlabels:\tenhancement, size:M\r\n--\nbody\n",
    )
    .unwrap();

    let rows = json_rows(&env, "2026-11-10", None);
    assert_eq!(
        rows,
        [json!({
            "task": "issue-7",
            "issue": 7,
            "started": "2026-11-10T08:00:00Z",
            "size": "M",
            "review_rounds": 3,
            "must_fix": [2, null, 0],
            "wall_to_ready_s": 3600,
            "worker_s": 1200,
            "worker_cost_usd": 1.5,
            "reviewer_s": 300,
            "reviewer_cost_usd": null,
            "outcome": "ready (rounds-exhausted)",
            "pr": "https://github.com/o/r/pull/12"
        })]
    );

    let (result, out, _) = facts(&env, &opts("2026-11-10", None, false));
    result.unwrap();
    assert_eq!(
        out,
        format!(
            "{HEADER}| issue-7 | 7 | 2026-11-10T08:00:00Z | M | 3 | 2, -, 0 | 3600 | 1200 | \
             $1.50 | 300 | - | ready (rounds-exhausted) | https://github.com/o/r/pull/12 |\n"
        )
    );
}

#[test]
fn task_facts_gives_a_pr_task_no_issue_and_a_finished_task_its_stage() {
    let env = Env::new();
    let mut task = Record::new(
        &NewTask {
            kind: Kind::Pr,
            number: 30,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        at("2026-11-10T08:00:00Z"),
        Process::new(1, 0),
    );
    task.stage = Stage::Finished;
    task.status = Status::Ok;
    save_archived(&env, "pr-30", &task);

    let rows = json_rows(&env, "2026-11-10", None);

    assert_eq!(rows[0]["task"], "pr-30");
    assert_eq!(rows[0]["issue"], Value::Null);
    assert_eq!(rows[0]["outcome"], "finished");
}

#[test]
fn task_facts_writes_nothing_under_the_base_dir() {
    let env = Env::new();
    let dir = save_live(&env, &issue_task(1, "2026-11-10T08:00:00Z"));
    fs::write(dir.join("review-1.md"), "Must-fix findings: 1\n").unwrap();
    save_archived(&env, "issue-2", &issue_task(2, "2026-11-10T09:00:00Z"));
    let before = snapshot(&env.base_dir());

    for json in [false, true] {
        facts(&env, &opts("2026-11-10", None, json)).0.unwrap();
    }

    assert_eq!(snapshot(&env.base_dir()), before);
}

#[test]
fn task_facts_skips_an_archive_entry_that_is_a_link() {
    let env = Env::new();
    let elsewhere = env.tmp.path().join("elsewhere");
    save(&elsewhere, &issue_task(8, "2026-11-10T08:00:00Z"));
    let root = merged::archive_root(&env.base_dir());
    fs::create_dir_all(&root).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&elsewhere, root.join("issue-8")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&elsewhere, root.join("issue-8")).is_ok();
    if !linked {
        return; // no right to make a link here (Windows without developer mode)
    }

    let (result, out, warn) = facts(&env, &opts("2026-11-10", None, true));

    result.unwrap();
    assert_eq!(out, "[]\n");
    let named = root.join("issue-8").display().to_string();
    assert_eq!(warn.lines().count(), 1, "{warn}");
    assert!(
        warn.starts_with("warning: ") && warn.contains(&named) && warn.contains("link"),
        "{warn}"
    );
}

#[test]
fn task_facts_warns_about_a_task_json_that_is_a_link() {
    let env = Env::new();
    let elsewhere = env.tmp.path().join("elsewhere");
    let target = save(&elsewhere, &issue_task(10, "2026-11-10T08:00:00Z")).join("task.json");
    let dir = record::task_dir(&env.base_dir(), "issue-10");
    fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, dir.join("task.json")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&target, dir.join("task.json")).is_ok();
    if !linked {
        return; // no right to make a link here (Windows without developer mode)
    }

    let (result, out, warn) = facts(&env, &opts("2026-11-10", None, true));

    result.unwrap();
    assert_eq!(out, "[]\n");
    let named = dir.join("task.json").display().to_string();
    assert_eq!(warn.lines().count(), 1, "{warn}");
    assert!(
        warn.starts_with("warning: ") && warn.contains(&named) && warn.contains("link"),
        "{warn}"
    );
}

#[test]
fn task_facts_does_not_read_a_review_file_that_is_a_link() {
    let env = Env::new();
    let dir = save_live(&env, &issue_task(9, "2026-11-10T08:00:00Z"));
    let elsewhere = env.tmp.path().join("review.md");
    fs::write(&elsewhere, "Must-fix findings: 5\n").unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&elsewhere, dir.join("review-1.md")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&elsewhere, dir.join("review-1.md")).is_ok();
    if !linked {
        return; // no right to make a link here (Windows without developer mode)
    }

    let rows = json_rows(&env, "2026-11-10", None);

    assert_eq!(rows[0]["review_rounds"], 0);
    assert_eq!(rows[0]["must_fix"], json!([]));
}

#[test]
fn task_facts_cli_takes_since_until_and_json() {
    let env = Env::new();
    save_live(&env, &issue_task(1, "2026-11-10T08:00:00Z"));

    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args([
            "task",
            "facts",
            "--since",
            "2026-11-10",
            "--until",
            "2026-11-10",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(ids(&rows), ["issue-1"]);
}

#[test]
fn task_facts_cli_fails_on_a_bad_date() {
    let env = Env::new();

    Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "facts", "--since", "2026-11-31"])
        .assert()
        .failure();
}
