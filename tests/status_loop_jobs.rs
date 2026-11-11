//! Issue 528: `sbxm status` shows the loop's queued jobs (`<base>/.sbxm/queue/*.json`, decision
//! 309) on a `jobs:` line in the order the loop takes them, and the job in `queue/running/` on a
//! `job:` line, marked interrupted when no loop runs. It reads files only.

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::Env as TestEnv;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::github::fake::FakeGitHub;
use sbxm::run::results::format_timestamp;
use sbxm::task::jobs::{queue_dir, running_dir};
use sbxm::task::queue::{LockFile, lock_path};
use sbxm::task::record::{Process, ProcessProbe};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;
const LOOP_START: u64 = T0 - 1_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn alive() -> Probe {
    Probe(Some(LOOP_START))
}

fn gone() -> Probe {
    Probe(None)
}

/// The lock of a running `task loop --issue <issues>`.
fn write_lock(base: &Path, issues: &[u32]) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: Process::new(1, LOOP_START),
        issues: issues.to_vec(),
        cap: None,
        progress: None,
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
}

/// Writes `text` as `dir/name`, modified at `secs`.
fn file_at(dir: &Path, name: &str, text: &str, secs: u64) {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

fn queued(base: &Path, name: &str, text: &str, secs: u64) {
    file_at(&queue_dir(base), name, text, secs);
}

fn env(terminal: bool) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width: 200,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(std::path::PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    }
}

/// One frame of `status`, with the Loop section.
fn frame(base: &Path, probe: &Probe, terminal: bool) -> String {
    let gh = FakeGitHub::default();
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        probe,
        &env(terminal),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn a_job_queued_behind_a_running_issue_loop_shows_on_a_jobs_line() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[527]);
    queued(&b, "a.json", r#"{"kind": "run", "issue": 524}"#, T0 - 60);

    let text = frame(&b, &alive(), false);

    assert!(text.contains("\n  jobs: 1 job\n    run #524\n"), "{text}");
}

#[test]
fn no_queue_folder_and_an_empty_one_are_jobs_none_and_no_job_line() {
    let base = TestEnv::new();
    let b = base.base_dir();

    let text = frame(&b, &alive(), false);
    assert!(text.contains("\n  jobs: none\n"), "{text}");
    assert!(!text.contains("  job:"), "{text}");

    fs::create_dir_all(running_dir(&b)).unwrap();
    let text = frame(&b, &alive(), false);
    assert!(text.contains("\n  jobs: none\n"), "{text}");
    assert!(!text.contains("  job:"), "{text}");
}

#[test]
fn jobs_show_oldest_first_and_by_file_name_on_the_same_time() {
    let base = TestEnv::new();
    let b = base.base_dir();
    queued(&b, "a.json", r#"{"kind": "run", "issue": 3}"#, T0 - 10);
    queued(&b, "z.json", r#"{"kind": "run", "issue": 1}"#, T0 - 100);
    queued(&b, "c.json", r#"{"kind": "run", "issue": 5}"#, T0 - 10);
    queued(&b, "b.json", r#"{"kind": "run", "issue": 4}"#, T0 - 10);

    let text = frame(&b, &alive(), false);

    assert!(
        text.contains("  jobs: 4 jobs\n    run #1\n    run #3\n    run #4\n    run #5\n"),
        "{text}"
    );
}

fn many(base: &Path, count: u64) {
    for n in 0..count {
        queued(
            base,
            &format!("j{n:02}.json"),
            &format!(r#"{{"kind": "run", "issue": {}}}"#, n + 1),
            T0 - 100 + n,
        );
    }
}

#[test]
fn ten_jobs_on_a_terminal_show_all_ten() {
    let base = TestEnv::new();
    let b = base.base_dir();
    many(&b, 10);

    let text = frame(&b, &alive(), true);

    assert!(text.contains("  jobs: 10 jobs\n"), "{text}");
    assert!(text.contains("    run #10\n"), "{text}");
    assert!(!text.contains("more"), "{text}");
}

#[test]
fn eleven_jobs_on_a_terminal_show_ten_and_one_more() {
    let base = TestEnv::new();
    let b = base.base_dir();
    many(&b, 11);

    let text = frame(&b, &alive(), true);

    assert!(text.contains("  jobs: 11 jobs\n    run #1\n"), "{text}");
    assert!(text.contains("    run #10\n    … 1 more\n"), "{text}");
    assert!(!text.contains("run #11"), "{text}");
}

#[test]
fn eleven_jobs_on_a_pipe_show_all_eleven() {
    let base = TestEnv::new();
    let b = base.base_dir();
    many(&b, 11);

    let text = frame(&b, &alive(), false);

    assert!(text.contains("    run #10\n    run #11\n"), "{text}");
    assert!(!text.contains("more"), "{text}");
}

#[test]
fn a_file_that_is_not_a_job_shows_as_refused_and_a_tmp_file_not_at_all() {
    let base = TestEnv::new();
    let b = base.base_dir();
    queued(&b, "bad.json", "{not json", T0 - 30);
    queued(
        &b,
        "extra.json",
        r#"{"kind": "run", "issue": 5, "flag": "--yes"}"#,
        T0 - 20,
    );
    queued(&b, "x.json.tmp", r#"{"kind": "run", "issue": 6}"#, T0 - 10);

    let text = frame(&b, &alive(), false);

    assert!(
        text.contains(
            "  jobs: 2 jobs\n    bad.json (not a job: will be refused)\n    extra.json (not a \
             job: will be refused)\n"
        ),
        "{text}"
    );
    assert!(!text.contains("x.json.tmp"), "{text}");
    assert!(!text.contains("run #6"), "{text}");
}

#[test]
fn each_kind_shows_its_kind_and_target() {
    let base = TestEnv::new();
    let b = base.base_dir();
    queued(&b, "1.json", r#"{"kind": "run", "issue": 524}"#, T0 - 50);
    queued(&b, "2.json", r#"{"kind": "review", "pr": 465}"#, T0 - 40);
    queued(&b, "3.json", r#"{"kind": "fix", "pr": 465}"#, T0 - 30);
    queued(
        &b,
        "4.json",
        r#"{"kind": "fix", "pr": 465, "check_run": 42452742075}"#,
        T0 - 20,
    );
    queued(&b, "5.json", r#"{"kind": "rm-merged"}"#, T0 - 10);

    let text = frame(&b, &alive(), false);

    assert!(
        text.contains(
            "  jobs: 5 jobs\n    run #524\n    review #465\n    fix #465\n    fix #465 (check \
             run 42452742075)\n    rm-merged\n"
        ),
        "{text}"
    );
}

#[test]
fn a_spec_job_and_a_template_job_show_their_kind_and_target() {
    let base = TestEnv::new();
    let b = base.base_dir();
    queued(
        &b,
        "1.json",
        r#"{"kind": "spec", "file": "retro-2026-11-08.md"}"#,
        T0 - 30,
    );
    queued(
        &b,
        "2.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        T0 - 20,
    );
    queued(
        &b,
        "3.json",
        r#"{"kind": "template", "profile": "sbxm-dev", "harness": "codex", "warm": true}"#,
        T0 - 10,
    );

    let text = frame(&b, &alive(), false);

    assert!(
        text.contains(
            "  jobs: 3 jobs\n    spec retro-2026-11-08.md\n    template sbxm-dev\n    template \
             sbxm-dev --harness codex --warm\n"
        ),
        "{text}"
    );
}

/// A running file as the loop's `mark` writes it once the job started.
fn running_file(base: &Path, secs: u64) {
    file_at(
        &running_dir(base),
        "a.json",
        r#"{"job": {"kind": "run", "issue": 524}, "started_at": "x", "task": "issue-524"}"#,
        secs,
    );
}

#[test]
fn a_running_job_with_a_loop_running_shows_on_a_job_line_since_its_modified_time() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[527]);
    running_file(&b, T0 - 300);

    let text = frame(&b, &alive(), false);

    assert!(
        text.contains(&format!(
            "\n  job: run #524 (since {})\n",
            format_timestamp(T0 - 300)
        )),
        "{text}"
    );
    assert!(text.contains("\n  jobs: none\n"), "{text}");
}

#[test]
fn a_job_moved_to_running_and_not_yet_marked_shows_too() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b, &[527]);
    file_at(
        &running_dir(&b),
        "a.json",
        r#"{"kind": "review", "pr": 9}"#,
        T0 - 5,
    );

    let text = frame(&b, &alive(), false);

    assert!(text.contains("\n  job: review #9 (since "), "{text}");
}

#[test]
fn a_running_job_with_no_loop_is_marked_interrupted() {
    for stale in [false, true] {
        let base = TestEnv::new();
        let b = base.base_dir();
        if stale {
            write_lock(&b, &[527]);
        }
        running_file(&b, T0 - 300);

        let text = frame(&b, &gone(), false);

        assert!(text.contains("Loop: not running"), "{text}");
        assert!(
            text.contains("\n  job: run #524 (interrupted: the next loop reports it)\n"),
            "{text}"
        );
    }
}

#[test]
fn finished_results_are_not_shown_and_nothing_is_written() {
    let base = TestEnv::new();
    let b = base.base_dir();
    file_at(
        &queue_dir(&b).join("done"),
        "old.json",
        r#"{"job": {"kind": "run", "issue": 1}, "outcome": "done"}"#,
        T0 - 500,
    );
    queued(&b, "a.json", r#"{"kind": "run", "issue": 2}"#, T0 - 60);
    let names = |dir: &Path| {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let before = names(&queue_dir(&b));

    let text = frame(&b, &alive(), false);

    assert!(text.contains("  jobs: 1 job\n    run #2\n"), "{text}");
    assert!(!text.contains("run #1\n"), "{text}");
    assert!(!running_dir(&b).exists());
    assert_eq!(names(&queue_dir(&b)), before);
    assert!(!lock_path(&b).exists());
}
