//! Issue 688: `sbxm status`'s `queue:` list (and its count) leaves out an issue that has a task
//! record in this base dir, whatever its stage: the task table and the `task:` / `open task PRs:`
//! lines show it already. An issue with no task that the loop would pass over is still listed as
//! blocked, with the reason.

use crate::common;

use std::path::{Path, PathBuf};

use common::Env as TestEnv;
use common::task_fixture::{issue_text, ready_open_issue};
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, PrInfo, PrState};
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const T0: u64 = 1_790_000_000;

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(T0 - 1_000)
    }
}

fn issue(number: u32, title: &str) -> Issue {
    Issue {
        number,
        open: true,
        title: title.into(),
        labels: vec![READY.to_owned(), "size:S".to_owned()],
        // What the readiness rules ask of a body (issue 725).
        body: ready_open_issue(number, &[READY, "size:S"], "").body,
    }
}

fn github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for i in &issues {
        gh = gh.with_issue_text(issue_text(i.number));
    }
    gh.with_open_issues(issues)
}

fn env() -> Env {
    Env {
        placement: Default::default(),
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 200,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    }
}

/// The Loop section of one `status` frame, up to the disk footer.
fn section(base: &Path, gh: &FakeGitHub) -> String {
    let source = LoopSource {
        github: gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        &Probe,
        &env(),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let loop_part = text.split("\nLoop:").nth(1).expect("a Loop section");
    format!(
        "Loop:{}",
        loop_part.split("\nFree space:").next().unwrap_or_default()
    )
}

fn task(number: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: REPO,
            title: &format!("title {number}"),
            base: "main",
            branch: &format!("issue-{number}"),
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

fn running(number: u32) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Working;
    rec.status = Status::Running;
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    rec
}

fn finished(number: u32, pr: u32) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
    rec.process = None;
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    rec
}

/// A task at `ready` the loop claimed, with no PR and no live lock: parked.
fn parked(number: u32) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.process = None;
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    rec
}

fn open_pr(gh: FakeGitHub, number: u32, branch: &str) -> FakeGitHub {
    gh.with_pr(PrInfo {
        number,
        head_ref: branch.into(),
        head_oid: "abc".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    })
}

#[test]
fn status_queue_tasks_a_running_task_leaves_the_queue_and_its_count() {
    let env = TestEnv::new();
    let base = env.base_dir();
    save(&base, &running(682));
    let gh = github(vec![issue(682, "Running"), issue(7, "Seven")]);

    let text = section(&base, &gh);

    assert!(
        text.contains("  queue: 1 issue\n    #7: Seven (size S)\n"),
        "{text}"
    );
    assert!(!text.contains("#682: Running"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
}

#[test]
fn status_queue_tasks_a_finished_task_with_an_open_pr_is_not_listed() {
    let env = TestEnv::new();
    let base = env.base_dir();
    save(&base, &finished(682, 684));
    let gh = open_pr(
        github(vec![issue(682, "Done"), issue(7, "Seven")]),
        684,
        "issue-682",
    )
    .with_branch_head("issue-682", "abc");

    let text = section(&base, &gh);

    assert!(
        text.contains("  queue: 1 issue\n    #7: Seven (size S)\n"),
        "{text}"
    );
    assert!(!text.contains("#682: Done"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
}

#[test]
fn status_queue_tasks_a_parked_task_is_not_listed() {
    let env = TestEnv::new();
    let base = env.base_dir();
    save(&base, &parked(682));
    let gh = github(vec![issue(682, "Parked"), issue(7, "Seven")]);

    let text = section(&base, &gh);

    assert!(
        text.contains("  queue: 1 issue\n    #7: Seven (size S)\n"),
        "{text}"
    );
    assert!(!text.contains("#682: Parked"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
}

#[test]
fn status_queue_tasks_no_task_with_its_branch_on_origin_is_still_blocked() {
    let env = TestEnv::new();
    let base = env.base_dir();
    let gh = github(vec![issue(682, "Orphan")]).with_branch_head("issue-682", "abc");

    let text = section(&base, &gh);

    assert!(
        text.contains(
            "  queue: 1 issue (1 blocked)\n    #682: Orphan (size S) — blocked: branch issue-682 \
             is already on origin"
        ),
        "{text}"
    );
}

#[test]
fn status_queue_tasks_no_task_and_nothing_blocking_is_listed_as_before() {
    let env = TestEnv::new();
    let base = env.base_dir();
    let gh = github(vec![issue(7, "Seven")]);

    let text = section(&base, &gh);

    assert!(
        text.contains("  queue: 1 issue\n    #7: Seven (size S)\n"),
        "{text}"
    );
}

#[test]
fn status_queue_tasks_every_queued_issue_with_a_task_shows_an_empty_queue() {
    let env = TestEnv::new();
    let base = env.base_dir();
    save(&base, &running(682));
    save(&base, &parked(683));
    let gh = github(vec![issue(682, "Running"), issue(683, "Parked")]);

    let text = section(&base, &gh);

    assert!(text.contains("  queue: empty\n"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
}
