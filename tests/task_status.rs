//! M2b slice 4: `sbxm task status` (spec §4): one line per task with stage, status,
//! `interrupted` and which of `result.md`/`review.md` exist; `--json` prints the records.

use std::fs;
use std::path::Path;

use sbxm::commands::task_status;
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

const T0: u64 = 1_790_000_000;

fn task(kind: Kind, number: u32, title: &str) -> Record {
    Record::new(
        &NewTask {
            kind,
            number,
            repo: "o/r",
            title,
            base: "main",
            branch: "b",
            config_hash: "h",
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, record: &Record) {
    record::write(&record::task_dir(base, &record.id), record).unwrap();
}

fn working(number: u32, done: Option<Status>) -> Record {
    let mut record = task(Kind::Issue, number, &format!("title {number}"));
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    if let Some(status) = done {
        record.finish(status).unwrap();
    }
    record
}

#[test]
fn no_tasks_says_how_to_start_one() {
    let base = tempfile::tempdir().unwrap();
    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    assert!(text.contains("No tasks"), "{text}");
}

#[test]
fn one_line_per_task_with_stage_status_and_title() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));
    save(base.path(), &working(7, Some(Status::Completed)));

    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].starts_with("issue-7 ")
            && lines[0].contains("working")
            && lines[0].contains("completed")
    );
    assert!(lines[0].contains("title 7"), "{text}");
    assert!(
        lines[1].starts_with("issue-41") && lines[1].contains("running"),
        "{text}"
    );
}

#[test]
fn a_running_task_whose_process_is_gone_shows_interrupted() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));

    let text = task_status::render(base.path(), None, false, &Probe(None)).unwrap();

    assert!(text.contains("interrupted"), "{text}");
    assert!(!text.contains("running"), "{text}");
}

#[test]
fn result_and_review_files_are_reported_when_present() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1, Some(Status::Completed)));
    save(base.path(), &working(2, Some(Status::Completed)));
    fs::write(
        record::task_dir(base.path(), "issue-2").join("result.md"),
        "done",
    )
    .unwrap();
    fs::write(
        record::task_dir(base.path(), "issue-2").join("review.md"),
        "ok",
    )
    .unwrap();

    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    assert!(
        lines[0].contains("result: no") && lines[0].contains("review: no"),
        "{text}"
    );
    assert!(
        lines[1].contains("result: yes") && lines[1].contains("review: yes"),
        "{text}"
    );
}

#[test]
fn a_selector_shows_only_that_task() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1, None));
    save(base.path(), &working(2, None));
    let mut pr = task(Kind::Pr, 2, "a pr");
    pr.advance(Stage::Reviewing, T0, Process::new(1, T0))
        .unwrap();
    save(base.path(), &pr);

    let text =
        task_status::render(base.path(), Some((Kind::Pr, 2)), false, &Probe(Some(T0))).unwrap();

    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(
        text.starts_with("pr-2 ") && text.contains("reviewing"),
        "{text}"
    );
}

#[test]
fn an_unknown_task_is_an_error_with_a_fix() {
    let base = tempfile::tempdir().unwrap();
    let message = format!(
        "{:#}",
        task_status::render(base.path(), Some((Kind::Issue, 9)), false, &Probe(Some(T0)))
            .unwrap_err()
    );
    assert!(
        message.contains("no task issue-9") && message.contains("sbxm task status"),
        "{message}"
    );
}

#[test]
fn json_prints_the_records_with_an_interrupted_flag() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));
    save(base.path(), &working(7, Some(Status::Completed)));

    let text = task_status::render(base.path(), None, true, &Probe(None)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    let tasks = value.as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["id"], "issue-7");
    assert_eq!(tasks[0]["interrupted"], false);
    assert_eq!(tasks[1]["id"], "issue-41");
    assert_eq!(tasks[1]["interrupted"], true);
    assert_eq!(tasks[1]["stage"], "working");
}

#[test]
fn the_cli_refuses_both_selectors_at_once() {
    let config = tempfile::tempdir().unwrap();
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", config.path())
        .args(["task", "status", "--issue", "1", "--pr", "2"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn json_with_no_tasks_is_an_empty_array() {
    let base = tempfile::tempdir().unwrap();
    let text = task_status::render(base.path(), None, true, &Probe(None)).unwrap();
    assert_eq!(text.trim(), "[]");
}

/// Plain `git` for building a fixture repo (not the code under test).
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// `<base>/.sbxm/tasks/<id>/repo.git` with `main` and branch `b`, `ahead` commits past it.
fn repo_with_commits_ahead(base: &Path, id: &str, ahead: u32) {
    let work = base.join(format!("{id}-work"));
    fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    fs::write(work.join("base.txt"), "base").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "base"]);
    git(&work, &["checkout", "-q", "-b", "b"]);
    for n in 0..ahead {
        fs::write(work.join(format!("f{n}.txt")), "x").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", &format!("c{n}")]);
    }
    let repo_git = record::task_dir(base, id).join("repo.git");
    fs::create_dir_all(repo_git.parent().unwrap()).unwrap();
    git(
        base,
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            repo_git.to_str().unwrap(),
        ],
    );
}

#[test]
fn the_line_shows_the_commits_ahead_of_base() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, Some(Status::Completed)));
    repo_with_commits_ahead(base.path(), "issue-41", 2);

    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: 2"), "{text}");
}

#[test]
fn a_task_without_a_repo_yet_shows_no_count_instead_of_failing() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));

    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: -"), "{text}");
}

#[test]
fn a_branch_with_no_new_commits_shows_zero() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, Some(Status::Completed)));
    repo_with_commits_ahead(base.path(), "issue-41", 0);

    let text = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: 0"), "{text}");
}

#[test]
fn json_carries_the_commit_count_or_null() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, Some(Status::Completed)));
    repo_with_commits_ahead(base.path(), "issue-41", 2);
    save(base.path(), &working(42, None));

    let text = task_status::render(base.path(), None, true, &Probe(Some(T0))).unwrap();
    let values: serde_json::Value = serde_json::from_str(&text).unwrap();
    let find = |id: &str| {
        values
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap()
            .clone()
    };

    assert_eq!(find("issue-41")["commits_ahead"], 2);
    assert!(find("issue-42")["commits_ahead"].is_null());
}
