//! Issue 376 (decision 294): the Loop section of `sbxm status`: whether a loop runs (its lock),
//! the task it is on and the tasks it finished (the task records), the PRs awaiting the user and
//! the queue (one GitHub fetch at most once a minute). Golden-tested like the Tasks section.

mod common;

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use common::Env as TestEnv;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{Issue, IssueText, PrInfo, PrState};
use sbxm::run::results::format_timestamp;
use sbxm::status::looping::{self, Lock, QueueItem, Snapshot, View, Work};
use sbxm::task::merged;
use sbxm::task::queue::{LOOP_HOOK, lock_path};
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Stamp, Status,
};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;
/// When the loop in the fixtures started.
const LOOP_START: u64 = T0 - 1_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

/// The loop's process as the fixtures write it into the lock.
fn loop_process() -> Process {
    Process::new(1, LOOP_START)
}

/// A probe under which the loop's process is alive.
fn alive() -> Probe {
    Probe(Some(LOOP_START))
}

fn write_lock(base: &Path) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string(&loop_process()).unwrap()).unwrap();
}

fn task(number: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: REPO,
            title: &format!("title {number}"),
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, rec: &Record) {
    record::write(&record::task_dir(base, &rec.id), rec).unwrap();
}

/// A task the loop is working on: still running, marked as the loop's (`hooks.task_loop`).
fn running(number: u32, stage: Stage) -> Record {
    let mut rec = task(number);
    rec.stage = stage;
    rec.status = Status::Running;
    rec.process = Some(loop_process());
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    rec
}

/// A task finished at `at`, marked as the loop's.
fn finished(number: u32, at: u64, marked: bool) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.stages.push(Stamp {
        stage: Stage::Finished,
        at: format_timestamp(at),
    });
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + number));
    if marked {
        rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    }
    rec
}

fn issue(number: u32, title: &str, labels: &[&str]) -> Issue {
    Issue {
        number,
        open: true,
        title: title.into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        body: String::new(),
    }
}

/// GitHub with these open issues, each one's text scripted, so `task run`'s check of a queued
/// issue passes (issue 536).
fn open_github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default();
    for i in &issues {
        gh = gh.with_issue_text(IssueText {
            number: i.number,
            title: i.title.clone(),
            state: "OPEN".into(),
            text: String::new(),
        });
    }
    gh.with_open_issues(issues)
}

fn labeled(number: u32, title: &str, size: &str) -> Issue {
    issue(number, title, &["ready-for-worker", size])
}

fn pr(number: u32) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: "abc".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// A footer that reads no real drive: 40 GiB free, the state folder on `base_dir`'s drive.
fn quiet_disk() -> sbxm::status::footer::Disk {
    sbxm::status::footer::Disk {
        free_space: |_| Ok(40 * 1024 * 1024 * 1024),
        same_drive: |_, _| true,
        state_dir: Some(std::path::PathBuf::from("/state")),
        min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
    }
}

fn env(terminal: bool, width: usize) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width,
        iterations: None,
        slots: None,
        disk: quiet_disk(),
    }
}

/// One frame of `status`, with the Loop section.
fn frame(base: &Path, probe: &Probe, gh: &FakeGitHub, terminal: bool, width: usize) -> String {
    let source = LoopSource {
        github: gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        probe,
        &env(terminal, width),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

fn issues_open_calls(gh: &FakeGitHub) -> usize {
    gh.calls()
        .iter()
        .filter(|c| matches!(c, GhCall::IssuesOpen(_)))
        .count()
}

#[test]
fn no_lock_file_is_a_loop_not_running_and_the_queue_and_awaiting_prs_still_show() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &finished(2, T0 - 10, true));
    let gh = open_github(vec![labeled(4, "Four", "size:S")]).with_pr(pr(102));

    let text = frame(&b, &alive(), &gh, false, 80);

    assert!(text.contains("\nLoop: not running\n"), "{text}");
    assert!(
        text.contains("  awaiting you: 1 of 5 PRs (#102: title 2)\n"),
        "{text}"
    );
    assert!(
        text.contains("  queue: 1 issue\n    #4: Four (size S)\n"),
        "{text}"
    );
    assert!(!text.contains("  task:"), "{text}");
    assert_eq!(issues_open_calls(&gh), 1);
}

#[test]
fn a_lock_whose_process_is_gone_is_not_running_names_the_stale_lock_and_shows_the_queue() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    for probe in [Probe(None), Probe(Some(LOOP_START + 5))] {
        let gh = open_github(vec![labeled(4, "Four", "size:S")]);

        let text = frame(&base.base_dir(), &probe, &gh, false, 80);

        assert!(
            text.contains("Loop: not running (stale lock: process 1, started "),
            "{text}"
        );
        assert!(text.contains(&format_timestamp(LOOP_START)), "{text}");
        assert!(
            text.contains("  queue: 1 issue\n    #4: Four (size S)\n"),
            "{text}"
        );
        assert!(!text.contains("  task:"), "{text}");
        assert_eq!(issues_open_calls(&gh), 1);
    }
}

#[test]
fn without_a_loop_a_lookup_failure_is_unreadable_and_the_rest_still_shows() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &running(7, Stage::Working));
    let gh = FakeGitHub::default().failing("boom");

    let text = frame(&b, &alive(), &gh, false, 80);

    assert!(text.contains("Loop: not running"), "{text}");
    assert!(text.contains("  queue: unreadable (boom)\n"), "{text}");
    assert!(text.contains("issue-7"), "{text}");
}

#[test]
fn a_lock_file_that_cannot_be_read_is_said_so() {
    let base = TestEnv::new();
    let path = lock_path(&base.base_dir());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "not json").unwrap();

    let text = frame(
        &base.base_dir(),
        &alive(),
        &FakeGitHub::default(),
        false,
        80,
    );

    assert!(text.contains("Loop: lock unreadable ("), "{text}");
}

#[test]
fn a_running_loop_shows_its_task_its_stage_the_tasks_done_and_the_prs_awaiting() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b);
    save(&b, &running(7, Stage::Reviewing));
    // Done in the window: marked, finished since the loop started. Not done: finished before it,
    // and finished but not the loop's.
    save(&b, &finished(2, T0 - 10, true));
    save(&b, &finished(5, LOOP_START - 5, true));
    save(&b, &finished(4, T0 - 10, false));
    let gh = open_github(Vec::new())
        .with_pr(pr(102))
        .with_pr(pr(103))
        .with_auto_merge(103)
        .with_pr(PrInfo {
            state: PrState::Merged,
            ..pr(105)
        });
    save(&b, &finished(3, T0 - 20, true));

    let text = frame(&b, &alive(), &gh, false, 80);

    let section = text.split("Loop:").nth(1).expect("a Loop section");
    // The disk footer comes after it (issue 92).
    let section = section.split("\nFree space:").next().unwrap();
    assert_eq!(
        format!("Loop:{section}"),
        format!(
            "Loop: running (process 1, started {})\n  task: issue-7 (title 7) at reviewing\n  done: 2 \
             (issue-2 (title 2), issue-3 (title 3))\n  jobs: none\n  awaiting you: 1 of 5 PRs (#102: title 2)\n  \
             queue: empty\n",
            format_timestamp(LOOP_START)
        )
    );
}

#[test]
fn a_running_loop_with_no_task_running_says_so() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = open_github(Vec::new());

    let text = frame(&base.base_dir(), &alive(), &gh, false, 80);

    assert!(text.contains("  task: none running\n"), "{text}");
    assert!(text.contains("  done: none yet\n"), "{text}");
}

#[test]
fn an_unmarked_running_task_with_the_locks_process_is_the_loops_during_work_and_review() {
    for stage in [Stage::Working, Stage::Reviewing] {
        let base = TestEnv::new();
        let b = base.base_dir();
        write_lock(&b);
        let mut unmarked = running(8, stage);
        unmarked.hooks = serde_json::Value::Null;
        save(&b, &unmarked);
        let gh = open_github(Vec::new());

        let text = frame(&b, &alive(), &gh, false, 80);

        assert!(
            text.contains(&format!("  task: issue-8 (title 8) at {}\n", stage.name())),
            "{text}"
        );
    }
}

#[test]
fn a_marked_running_task_of_another_process_is_not_the_loops() {
    // A different pid, and the lock's pid started at another time.
    for other in [
        Process::new(99, LOOP_START),
        Process::new(1, LOOP_START + 5),
    ] {
        let base = TestEnv::new();
        let b = base.base_dir();
        write_lock(&b);
        let mut foreign = running(8, Stage::Working);
        foreign.process = Some(other);
        save(&b, &foreign);
        let gh = open_github(Vec::new());

        // Alive under every pid, so only the identity decides.
        let text = frame(&b, &Probe(Some(LOOP_START)), &gh, false, 80);

        assert!(text.contains("  task: none running\n"), "{text}");
    }
}

#[test]
fn a_task_that_is_not_running_is_not_the_loops_even_with_its_process() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b);
    let mut failed = running(8, Stage::Working);
    failed.status = Status::Failed;
    save(&b, &failed);
    let gh = open_github(Vec::new());

    let text = frame(&b, &alive(), &gh, false, 80);

    assert!(text.contains("  task: none running\n"), "{text}");
}

#[test]
fn a_marked_finished_task_is_not_the_running_one() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b);
    save(&b, &finished(9, T0 - 10, true));
    let gh = open_github(Vec::new());

    let text = frame(&b, &alive(), &gh, false, 80);

    assert!(text.contains("  task: none running\n"), "{text}");
}

/// What the merged-task cleanup the loop runs before each step does to a task (decision 195):
/// archives its records under `.sbxm/task-archive/<id>/`, then removes its folder.
fn archive_and_remove(base: &Path, id: &str) {
    merged::archive_task(base, id).unwrap();
    fs::remove_dir_all(record::task_dir(base, id)).unwrap();
}

/// Review M-1 of PR 437: a task the loop finished in its window is still done after the
/// cleanup archived and removed it once its PR merged.
#[test]
fn a_task_done_in_the_window_stays_done_after_the_merged_cleanup_archives_it() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &finished(3, T0 - 10, true));
    assert_eq!(
        looping::work(&b, &loop_process()).unwrap().done,
        ["issue-3"]
    );

    archive_and_remove(&b, "issue-3");

    assert!(
        merged::archive_dir(&b, "issue-3")
            .join("task.json")
            .is_file()
    );
    assert!(!record::task_dir(&b, "issue-3").exists());
    assert_eq!(
        looping::work(&b, &loop_process()).unwrap().done,
        ["issue-3"]
    );
}

#[test]
fn an_archived_task_counts_once_and_only_when_marked_and_finished_in_the_window() {
    let base = TestEnv::new();
    let b = base.base_dir();
    // In both places, as between the archive and the removal: counted once.
    save(&b, &finished(2, T0 - 10, true));
    merged::archive_task(&b, "issue-2").unwrap();
    // Archived and gone: done in the window, before it, and not the loop's.
    for rec in [
        finished(3, T0 - 20, true),
        finished(5, LOOP_START - 5, true),
        finished(4, T0 - 10, false),
    ] {
        save(&b, &rec);
        archive_and_remove(&b, &rec.id);
    }

    let work = looping::work(&b, &loop_process()).unwrap();

    assert_eq!(work.done, ["issue-2", "issue-3"]);
    assert_eq!(work.current, None);
}

/// An archived task is never the running one, even if its record says running with the lock's
/// process: only the active records say what runs now.
#[test]
fn an_archived_running_record_is_not_the_loops_task() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &running(8, Stage::Working));
    archive_and_remove(&b, "issue-8");

    let work = looping::work(&b, &loop_process()).unwrap();

    assert_eq!(work, Work::default());
}

/// The archive is read like `task rm --merged` reads task folders: never through a link, at
/// the archive's root, a task's folder or its `task.json`.
#[cfg(unix)]
#[test]
fn archived_records_behind_a_link_are_not_read() {
    for link_at in ["root", "folder", "file"] {
        let base = TestEnv::new();
        let b = base.base_dir();
        let elsewhere = base.tmp.path().join("elsewhere");
        let archive = b.join(".sbxm").join("task-archive");
        let rec = finished(3, T0 - 10, true);
        match link_at {
            "root" => {
                let folder = elsewhere.join("issue-3");
                record::write(&folder, &rec).unwrap();
                fs::create_dir_all(archive.parent().unwrap()).unwrap();
                common::dir_link(&archive, &elsewhere);
            }
            "folder" => {
                record::write(&elsewhere, &rec).unwrap();
                fs::create_dir_all(&archive).unwrap();
                common::dir_link(&archive.join("issue-3"), &elsewhere);
            }
            _ => {
                record::write(&elsewhere, &rec).unwrap();
                fs::create_dir_all(archive.join("issue-3")).unwrap();
                common::file_link(
                    &archive.join("issue-3").join("task.json"),
                    &elsewhere.join("task.json"),
                );
            }
        }

        let work = looping::work(&b, &loop_process()).unwrap();

        assert!(work.done.is_empty(), "{link_at}: {work:?}");
    }
}

#[test]
fn the_queue_lists_the_labeled_issues_in_the_order_the_loop_takes_them() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = open_github(vec![
        labeled(9, "Nine", "size:M"),
        labeled(4, "Four", "size:S"),
        issue(5, "Not labeled", &["size:S"]),
    ]);

    let text = frame(&base.base_dir(), &alive(), &gh, false, 80);

    assert!(
        text.contains("  queue: 2 issues\n    #4: Four (size S)\n    #9: Nine (size M)\n"),
        "{text}"
    );
}

#[test]
fn a_queue_of_zero_is_empty() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = open_github(vec![issue(5, "Not labeled", &[])]);

    let text = frame(&base.base_dir(), &alive(), &gh, false, 80);

    assert!(text.contains("  queue: empty\n"), "{text}");
}

fn long_queue(count: u32) -> FakeGitHub {
    open_github(
        (1..=count)
            .map(|n| labeled(n, &format!("Issue number {n} with a title"), "size:S"))
            .collect(),
    )
}

#[test]
fn more_issues_than_fit_show_the_first_lines_and_how_many_more() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let count = looping::MAX_QUEUE_LINES as u32 + 3;

    let text = frame(&base.base_dir(), &alive(), &long_queue(count), true, 80);

    assert!(
        text.contains(&format!("  queue: {count} issues\n")),
        "{text}"
    );
    assert!(
        text.contains("    #1: Issue number 1 with a title (size S)\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("    #{}: Issue", looping::MAX_QUEUE_LINES)),
        "{text}"
    );
    assert!(
        !text.contains(&format!("    #{}: Issue", looping::MAX_QUEUE_LINES + 1)),
        "{text}"
    );
    assert!(text.contains("    … 3 more\n"), "{text}");
}

#[test]
fn on_a_terminal_a_long_line_ends_in_an_ellipsis_and_fits() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = open_github(vec![labeled(
        1,
        "A title that is far too long to fit in twenty columns",
        "size:S",
    )]);

    let text = frame(&base.base_dir(), &alive(), &gh, true, 20);

    let line = text
        .lines()
        .find(|l| l.contains("#1: "))
        .expect("the issue line");
    assert_eq!(line.chars().count(), 20, "{line}");
    assert!(line.ends_with('…'), "{line}");
}

#[test]
fn on_a_non_terminal_nothing_is_cut() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let title = "A title that is far too long to fit in twenty columns";
    let count = looping::MAX_QUEUE_LINES as u32 + 3;
    let mut issues = vec![labeled(1, title, "size:S")];
    issues.extend((2..=count).map(|n| labeled(n, "x", "size:S")));
    let gh = open_github(issues);

    let text = frame(&base.base_dir(), &alive(), &gh, false, 20);

    assert!(
        text.contains(&format!("    #1: {title} (size S)\n")),
        "{text}"
    );
    assert!(
        text.contains(&format!("    #{count}: x (size S)\n")),
        "{text}"
    );
    assert!(!text.contains("more\n"), "{text}");
    assert!(!text.contains('…'), "{text}");
}

#[test]
fn a_github_failure_shows_the_queue_unreadable_and_the_rest_of_status_still_shows() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b);
    save(&b, &running(7, Stage::Working));
    let gh = FakeGitHub::default().failing("HTTP 502: bad gateway\nsecond line");

    let text = frame(&b, &alive(), &gh, false, 80);

    assert!(
        text.contains("  queue: unreadable (HTTP 502: bad gateway)\n"),
        "{text}"
    );
    assert!(!text.contains("second line"), "{text}");
    assert!(text.contains("issue-7"), "the Tasks table: {text}");
    assert!(
        text.contains("  task: issue-7 (title 7) at working\n"),
        "{text}"
    );
}

#[test]
fn a_repo_that_cannot_be_told_makes_the_queue_unreadable() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = FakeGitHub::default();
    let source = LoopSource {
        github: &gh,
        repo: Err("cannot read the origin".into()),
    };
    let mut out = Vec::new();

    status::run_with(
        &base.base_dir(),
        &FakeBackend::default(),
        &alive(),
        &env(false, 80),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();

    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("  queue: unreadable (cannot read the origin)\n"),
        "{text}"
    );
    assert!(gh.calls().is_empty());
}

/// Three `--watch` frames, the second `59` seconds after the first and the third `2` later.
#[test]
fn github_is_called_on_the_first_frame_not_59_seconds_later_and_again_after_61() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = open_github(Vec::new());
    let clock = Rc::new(Cell::new(T0));
    let steps = Rc::new(std::cell::RefCell::new(vec![2u64, 59]));
    let env = Env {
        now: Box::new({
            let clock = Rc::clone(&clock);
            move || clock.get()
        }),
        sleep: Box::new({
            let clock = Rc::clone(&clock);
            let steps = Rc::clone(&steps);
            move |_: Duration| {
                let step = steps.borrow_mut().pop().unwrap();
                clock.set(clock.get() + step);
            }
        }),
        terminal: true,
        width: 80,
        iterations: Some(3),
        slots: None,
        disk: quiet_disk(),
    };
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    // Frames at T0, T0 + 59 and T0 + 61.

    status::run_with(
        &base.base_dir(),
        &FakeBackend::default(),
        &alive(),
        &env,
        Some(Duration::from_secs(1)),
        Some(&source),
        &mut out,
    )
    .unwrap();

    // Frame 1 fetches; frame 2 (59 s) reuses; frame 3 (61 s) fetches again.
    assert_eq!(issues_open_calls(&gh), 2);
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.matches("queue: empty").count(), 3, "{text}");
}

#[test]
fn the_cache_reuses_a_snapshot_under_a_minute_old_and_refetches_at_a_minute() {
    let mut cache = looping::Cache::default();
    let fetches = Cell::new(0);
    let fetch = || {
        fetches.set(fetches.get() + 1);
        Snapshot {
            queue: Ok(Vec::<QueueItem>::new()),
            awaiting: Ok(Vec::new()),
            pr_titles: Default::default(),
            issue_titles: Default::default(),
            blocked: Default::default(),
        }
    };

    cache.get(1_000, fetch);
    cache.get(1_059, fetch);
    assert_eq!(fetches.get(), 1);
    cache.get(1_060, fetch);
    assert_eq!(fetches.get(), 2);
    cache.get(1_061, fetch);
    assert_eq!(fetches.get(), 2);
}

#[test]
fn a_failed_fetch_is_not_retried_inside_the_minute_either() {
    let base = TestEnv::new();
    write_lock(&base.base_dir());
    let gh = FakeGitHub::default().failing("boom");
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let env = Env {
        iterations: Some(2),
        slots: None,
        disk: quiet_disk(),
        terminal: true,
        ..env(true, 80)
    };
    let mut out = Vec::new();

    status::run_with(
        &base.base_dir(),
        &FakeBackend::default(),
        &alive(),
        &env,
        Some(Duration::from_secs(1)),
        Some(&source),
        &mut out,
    )
    .unwrap();

    assert_eq!(issues_open_calls(&gh), 1);
}

#[test]
fn the_section_writes_nothing_and_never_calls_sbx() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_lock(&b);
    save(&b, &running(7, Stage::Working));
    let gh = open_github(Vec::new());
    let backend = FakeBackend::default();
    let before = fs::read(lock_path(&b)).unwrap();
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };

    status::run_with(
        &b,
        &backend,
        &alive(),
        &env(false, 80),
        None,
        Some(&source),
        &mut Vec::new(),
    )
    .unwrap();

    assert_eq!(fs::read(lock_path(&b)).unwrap(), before);
    assert!(!b.join(".sbxm").join("task-loop.log").exists());
}

#[test]
fn render_is_pure_over_its_view() {
    let view = View {
        lock: Lock::Running(loop_process()),
        work: Some(Work::default()),
        snapshot: Some(Snapshot {
            queue: Ok(vec![QueueItem {
                number: 4,
                title: "Four".into(),
                size: None,
            }]),
            awaiting: Ok(vec![1, 2]),
            pr_titles: Default::default(),
            issue_titles: Default::default(),
            blocked: Default::default(),
        }),
        pause: sbxm::task::loop_pause::State::None,
        jobs: Default::default(),
    };

    let text = looping::render(&view, None);

    assert!(
        text.contains("  awaiting you: 2 of 5 PRs (#1, #2)\n"),
        "{text}"
    );
    assert!(text.contains("  queue: 1 issue\n    #4: Four\n"), "{text}");
}
