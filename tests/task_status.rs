//! M2b slice 4: `sbxm task status` (spec §4): one line per task with stage, status,
//! `interrupted` and which of `result.md`/`review.md` exist; `--json` prints the records.
//! Issue 129, M-1: its displayed output also reaches each displayed task's `run.log`.

mod common;

use std::fs;
use std::path::Path;

use common::Env;
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
            id: None,
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
    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    assert!(text.contains("No tasks"), "{text}");
}

#[test]
fn one_line_per_task_with_stage_status_and_title() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));
    save(base.path(), &working(7, Some(Status::Completed)));

    let (text, ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
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
    assert_eq!(ids, vec!["issue-7".to_owned(), "issue-41".to_owned()]);
}

/// Issue 142: a spec task has no `--spec` filter yet, but it shows up in the unfiltered listing
/// like any other task (`render` doesn't branch on `Kind` at all).
#[test]
fn a_spec_task_appears_in_the_unfiltered_listing() {
    let base = tempfile::tempdir().unwrap();
    let mut record = task(Kind::Spec, 0, "idea.md");
    record.id = "spec-idea-abc123".to_owned();
    save(base.path(), &record);

    let (text, ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(
        text.contains("spec-idea-abc123") && text.contains("idea.md"),
        "{text}"
    );
    assert_eq!(ids, vec!["spec-idea-abc123".to_owned()]);
}

#[test]
fn a_running_task_whose_process_is_gone_shows_interrupted() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(None)).unwrap();

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

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
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

/// Issue 123, decision 176: a task's recorded risk level shows in the plain listing
/// (`unknown` for one with no recorded review, as the field's default reads).
#[test]
fn the_recorded_risk_level_is_shown() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(1, Some(Status::Completed)));
    let mut with_review = working(2, Some(Status::Completed));
    with_review.review = Some(record::ReviewResult {
        round: 1,
        must_fix: 0,
        full: true,
        repeat: false,
        risk: sbxm::task::risk::Level::High,
        risk_reasons: vec!["touches the git trust boundary".to_owned()],
    });
    save(base.path(), &with_review);

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    assert!(lines[0].contains("risk: unknown"), "{text}");
    assert!(lines[1].contains("risk: high"), "{text}");
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

    let (text, ids) =
        task_status::render(base.path(), Some((Kind::Pr, 2)), false, &Probe(Some(T0))).unwrap();

    assert_eq!(ids, vec!["pr-2".to_owned()]);
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

    let (text, _ids) = task_status::render(base.path(), None, true, &Probe(None)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    let tasks = value.as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["id"], "issue-7");
    assert_eq!(tasks[0]["interrupted"], false);
    assert_eq!(tasks[1]["id"], "issue-41");
    assert_eq!(tasks[1]["interrupted"], true);
    assert_eq!(tasks[1]["stage"], "working");
}

fn spec_task() -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Spec,
            number: 0,
            repo: "o/r",
            title: "idea.md",
            base: "main",
            branch: "spec-idea-abc123",
            config_hash: "h",
            id: Some("spec-idea-abc123"),
        },
        T0,
        Process::new(1, T0),
    )
}

#[test]
fn a_persisted_spec_task_shows_in_the_plain_listing() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &spec_task());

    let (text, ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert_eq!(ids, vec!["spec-idea-abc123".to_owned()]);
    assert!(text.starts_with("spec-idea-abc123 "), "{text}");
    assert!(
        text.contains("prepared") && text.contains("idea.md"),
        "{text}"
    );
}

#[test]
fn a_persisted_spec_task_shows_in_the_json_listing() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &spec_task());

    let (text, _ids) = task_status::render(base.path(), None, true, &Probe(Some(T0))).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    let tasks = value.as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "spec-idea-abc123");
    assert_eq!(tasks[0]["kind"], "spec");
    assert_eq!(tasks[0]["interrupted"], false);
}

#[test]
fn filtering_status_by_spec_is_refused_and_says_what_to_do() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &spec_task());

    let error = task_status::render(base.path(), Some((Kind::Spec, 0)), false, &Probe(Some(T0)))
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("run it without a filter"),
        "{error:#}"
    );
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
    let (text, _ids) = task_status::render(base.path(), None, true, &Probe(None)).unwrap();
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

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: 2"), "{text}");
}

#[test]
fn a_task_without_a_repo_yet_shows_no_count_instead_of_failing() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, None));

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: -"), "{text}");
}

#[test]
fn a_branch_with_no_new_commits_shows_zero() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, Some(Status::Completed)));
    repo_with_commits_ahead(base.path(), "issue-41", 0);

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();

    assert!(text.contains("ahead: 0"), "{text}");
}

#[test]
fn json_carries_the_commit_count_or_null() {
    let base = tempfile::tempdir().unwrap();
    save(base.path(), &working(41, Some(Status::Completed)));
    repo_with_commits_ahead(base.path(), "issue-41", 2);
    save(base.path(), &working(42, None));

    let (text, _ids) = task_status::render(base.path(), None, true, &Probe(Some(T0))).unwrap();
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

#[test]
fn a_task_continuing_a_pr_shows_the_pr_and_its_branch() {
    let base = tempfile::tempdir().unwrap();
    let mut record = working(41, None);
    record.continues = Some(record::PrBranch {
        pr: 7,
        branch: "feature-x".into(),
        base: "abc123".into(),
    });
    save(base.path(), &record);
    save(base.path(), &working(42, None));

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    assert!(lines[0].contains("PR #7 (feature-x)"), "{text}");
    assert!(!lines[1].contains("PR #"), "{text}");
}

#[test]
fn a_stopped_task_shows_the_reason() {
    let base = tempfile::tempdir().unwrap();
    let mut record = working(41, Some(Status::Completed));
    record.stopped = Some(record::Stopped::RepeatFinding);
    save(base.path(), &record);
    save(base.path(), &working(42, None));

    let (text, _ids) = task_status::render(base.path(), None, false, &Probe(Some(T0))).unwrap();
    let lines: Vec<&str> = text.lines().collect();

    assert!(lines[0].contains("stopped: repeat-finding"), "{text}");
    assert!(!lines[1].contains("stopped:"), "{text}");
}

#[test]
fn the_cli_appends_a_selected_issues_output_to_its_run_log() {
    let env = Env::new();
    save(&env.base_dir(), &working(41, None));

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--issue", "41"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.starts_with("issue-41"), "{stdout}");

    let log =
        fs::read_to_string(record::task_dir(&env.base_dir(), "issue-41").join("run.log")).unwrap();
    assert!(log.starts_with("# "), "{log}");
    assert!(log.contains(stdout.trim_end()), "{log}");
}

#[test]
fn the_cli_appends_a_selected_prs_output_to_its_run_log() {
    let env = Env::new();
    let mut pr = task(Kind::Pr, 2, "a pr");
    pr.advance(Stage::Reviewing, T0, Process::new(1, T0))
        .unwrap();
    save(&env.base_dir(), &pr);

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status", "--pr", "2"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let log =
        fs::read_to_string(record::task_dir(&env.base_dir(), "pr-2").join("run.log")).unwrap();
    assert!(log.starts_with("# "), "{log}");
    assert!(log.contains("reviewing"), "{log}");
}

#[test]
fn the_cli_appends_unfiltered_output_to_every_displayed_tasks_run_log() {
    let env = Env::new();
    save(&env.base_dir(), &working(41, None));
    save(&env.base_dir(), &working(7, Some(Status::Completed)));

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "status"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    for id in ["issue-7", "issue-41"] {
        let log =
            fs::read_to_string(record::task_dir(&env.base_dir(), id).join("run.log")).unwrap();
        assert!(log.starts_with("# "), "{id}: {log}");
        for line in stdout.lines() {
            assert!(log.contains(line), "{id}: missing {line:?} in {log}");
        }
    }
}
