//! The Sandboxes section of `sbxm status` (decision 171(b), issue 92): every local sandbox in
//! `sbx ls`, its owner from the name and the host-side state files, its state and its age.

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use sbxm::backend::{FakeBackend, SandboxDetail, SandboxInfo};
use sbxm::commands::status::{self, Env};
use sbxm::status::sandboxes::{self, Local, Owner};
use sbxm::task::record::ProcessProbe;

/// 2026-09-21T14:13:20Z.
const T0: u64 = 1_790_000_000;

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn task_folder(base: &Path, id: &str) {
    let dir = base.join(".sbxm").join("tasks").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("task.json"), "{}").unwrap();
}

fn run_folder(base: &Path, id: &str) {
    fs::create_dir_all(base.join(".sbxm").join("runs").join(id)).unwrap();
}

fn project_state(base: &Path, project: &str, sandbox: &str) {
    let dir = base.join(".sbxm").join(project);
    fs::create_dir_all(&dir).unwrap();
    let json = serde_json::json!({
        "sandboxes": {"claude": {"sandbox": sandbox, "workspace": "/w", "created_at": 0}},
    });
    fs::write(dir.join("state.json"), json.to_string()).unwrap();
}

fn local(name: &str, status: &str, created_at: Option<&str>) -> SandboxDetail {
    SandboxDetail {
        name: name.to_owned(),
        status: status.to_owned(),
        cpus: None,
        memory_mib: None,
        expires_at: None,
        created_at: created_at.map(str::to_owned),
    }
}

fn info(name: &str, status: &str) -> SandboxInfo {
    SandboxInfo {
        name: name.to_owned(),
        agent: "claude".to_owned(),
        status: status.to_owned(),
    }
}

// ---- owners: the name and the state files -------------------------------------------------

#[test]
fn a_task_run_and_project_sandbox_get_their_owner_and_anything_else_is_an_orphan() {
    let base = tempfile::tempdir().unwrap();
    task_folder(base.path(), "issue-92");
    run_folder(base.path(), "20261030-100000");
    project_state(base.path(), "demo", "sbxm-demo-claude");
    let claims = sandboxes::claims(base.path());

    let owner = |name| sandboxes::owner(&claims, name);
    assert_eq!(
        owner("sbxm-task-issue-92-claude"),
        Owner::Task("issue-92".into())
    );
    assert_eq!(
        owner("sbxm-task-issue-92-review-codex"),
        Owner::Task("issue-92".into())
    );
    assert_eq!(
        owner("sbxm-run-20261030-100000-1-0"),
        Owner::Run("20261030-100000".into())
    );
    assert_eq!(owner("sbxm-demo-claude"), Owner::Project("demo".into()));
    // No task, run or project claims these.
    assert_eq!(owner("sbxm-task-issue-9-claude"), Owner::Orphan);
    assert_eq!(owner("sbxm-run-20261030-100000-x-0"), Owner::Orphan);
    assert_eq!(owner("sbxm-demo-codex"), Owner::Orphan);
    assert_eq!(owner("spike-s6-codex"), Owner::Orphan);
}

// ---- owners: a run's evaluator sandboxes (review M-1) ------------------------------------

#[test]
fn a_runs_judge_and_jev_sandboxes_belong_to_it_and_without_its_folder_are_orphans() {
    let base = tempfile::tempdir().unwrap();
    run_folder(base.path(), "20261105-120000");
    let claims = sandboxes::claims(base.path());

    let owner = |name: &str| sandboxes::owner(&claims, name);
    let run = Owner::Run("20261105-120000".into());
    assert_eq!(owner("sbxm-run-20261105-120000-0-0"), run);
    assert_eq!(owner("sbxm-run-20261105-120000-judge-0"), run);
    assert_eq!(owner("sbxm-run-20261105-120000-jev-2"), run);
    // The names the run commands make, not copies of them.
    assert_eq!(
        owner(&sbxm::run::sandbox_name("20261105-120000", "3", 1)),
        run
    );
    assert_eq!(owner(&sbxm::eval::jev::names("20261105-120000", 1).0), run);
    // No run folder, or a part no run sandbox has, or no repeat: orphans.
    assert_eq!(owner("sbxm-run-20261105-130000-judge-0"), Owner::Orphan);
    assert_eq!(owner("sbxm-run-20261105-120000-jury-0"), Owner::Orphan);
    assert_eq!(owner("sbxm-run-20261105-120000-judge-"), Owner::Orphan);
    assert_eq!(owner("sbxm-run-20261105-120000-judge-x"), Owner::Orphan);
}

#[test]
fn a_task_folder_without_task_json_claims_nothing() {
    let base = tempfile::tempdir().unwrap();
    fs::create_dir_all(base.path().join(".sbxm/tasks/issue-5")).unwrap();
    let claims = sandboxes::claims(base.path());

    assert_eq!(
        sandboxes::owner(&claims, "sbxm-task-issue-5-claude"),
        Owner::Orphan
    );
}

#[test]
fn the_longest_task_id_in_the_name_wins() {
    let base = tempfile::tempdir().unwrap();
    task_folder(base.path(), "spec-a");
    task_folder(base.path(), "spec-a-b");
    let claims = sandboxes::claims(base.path());

    assert_eq!(
        sandboxes::owner(&claims, "sbxm-task-spec-a-b-claude"),
        Owner::Task("spec-a-b".into())
    );
    assert_eq!(
        sandboxes::owner(&claims, "sbxm-task-spec-a-claude"),
        Owner::Task("spec-a".into())
    );
}

#[test]
fn a_project_state_naming_the_sandbox_beats_a_task_id_in_its_name() {
    let base = tempfile::tempdir().unwrap();
    task_folder(base.path(), "x");
    project_state(base.path(), "task-x", "sbxm-task-x-claude");
    let claims = sandboxes::claims(base.path());

    assert_eq!(
        sandboxes::owner(&claims, "sbxm-task-x-claude"),
        Owner::Project("task-x".into())
    );
}

#[test]
fn an_unreadable_state_file_claims_nothing_and_is_not_an_error() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join(".sbxm").join("demo");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("state.json"), "not json").unwrap();
    let claims = sandboxes::claims(base.path());

    assert_eq!(sandboxes::owner(&claims, "sbxm-demo-claude"), Owner::Orphan);
}

#[test]
fn no_sbxm_folder_claims_nothing() {
    let base = tempfile::tempdir().unwrap();
    let claims = sandboxes::claims(&base.path().join("missing"));

    assert_eq!(sandboxes::owner(&claims, "sbxm-demo-claude"), Owner::Orphan);
}

// ---- age: now minus `created_at` ----------------------------------------------------------

#[test]
fn the_age_is_now_minus_created_at_and_unknown_without_one() {
    // 1h05m before T0, with and without fractional seconds.
    assert_eq!(
        sandboxes::age(&local("a", "running", Some("2026-09-21T13:08:20Z")), T0),
        Some(3_900)
    );
    assert_eq!(
        sandboxes::age(
            &local("a", "running", Some("2026-09-21T13:08:20.656712Z")),
            T0
        ),
        Some(3_900)
    );
    assert_eq!(sandboxes::age(&local("a", "running", None), T0), None);
    assert_eq!(
        sandboxes::age(&local("a", "running", Some("yesterday")), T0),
        None
    );
    // A clock ahead of this one is no negative age.
    assert_eq!(
        sandboxes::age(&local("a", "running", Some("2026-09-21T14:13:25Z")), T0),
        Some(0)
    );
}

// ---- rendering ----------------------------------------------------------------------------

#[test]
fn the_section_is_a_table_of_name_owner_state_and_age() {
    let base = tempfile::tempdir().unwrap();
    task_folder(base.path(), "issue-92");
    project_state(base.path(), "demo", "sbxm-demo-claude");
    let claims = sandboxes::claims(base.path());
    let listing = Local::Sandboxes(vec![
        local(
            "sbxm-task-issue-92-claude",
            "running",
            Some("2026-09-21T13:08:20Z"),
        ),
        local("sbxm-demo-claude", "stopped", Some("2026-09-19T14:13:20Z")),
        local("spike-s6-codex", "stopped", None),
    ]);

    assert_eq!(
        sandboxes::render(&listing, &claims, T0, None),
        "\n\
         SANDBOX                    OWNER          STATE    AGE\n\
         sbxm-task-issue-92-claude  task issue-92  running  1h05m\n\
         sbxm-demo-claude           project demo   stopped  2d00h\n\
         spike-s6-codex             orphan         stopped  -\n"
    );
}

#[test]
fn no_local_sandboxes_says_so() {
    let claims = sandboxes::claims(Path::new("/nonexistent"));

    assert_eq!(
        sandboxes::render(&Local::Sandboxes(Vec::new()), &claims, T0, None),
        "\nNo local sandboxes.\n"
    );
}

#[test]
fn an_unreadable_list_is_one_line() {
    let claims = sandboxes::claims(Path::new("/nonexistent"));

    assert_eq!(
        sandboxes::render(
            &Local::Unreadable("daemon not running".into()),
            &claims,
            T0,
            None
        ),
        "\nLocal sandboxes couldn't be read (`sbx ls --json`): daemon not running\n"
    );
}

#[test]
fn a_terminal_cuts_each_line_to_its_width() {
    let claims = sandboxes::claims(Path::new("/nonexistent"));
    let listing = Local::Sandboxes(vec![local("spike-s6-codex", "stopped", None)]);

    let text = sandboxes::render(&listing, &claims, T0, Some(20));
    for line in text.lines() {
        let line: &str = line;
        assert!(line.chars().count() <= 20_usize, "{line:?}");
    }
}

// ---- reading: the local instance, through the command -------------------------------------

#[test]
fn read_asks_the_local_instance_with_created_at() {
    let backend = FakeBackend::with_sandboxes(vec![info("sbxm-demo-claude", "running")])
        .with_created_at(&[("sbxm-demo-claude", "2026-09-21T13:08:20Z")]);

    assert_eq!(
        sandboxes::read(&backend),
        Local::Sandboxes(vec![local(
            "sbxm-demo-claude",
            "running",
            Some("2026-09-21T13:08:20Z")
        )])
    );
    // `status` is given the local instance and doesn't ask for another one.
    assert!(backend.locations().is_empty());
}

#[test]
fn read_turns_a_failing_list_into_its_first_line() {
    let backend = FakeBackend::default().failing_list_with("daemon not running\nmore");

    assert_eq!(
        sandboxes::read(&backend),
        Local::Unreadable("daemon not running".into())
    );
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

fn env() -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        slots: None,
        disk: quiet_disk(),
    }
}

fn frame(base: &Path, backend: &FakeBackend) -> String {
    let mut out = Vec::new();
    status::run(base, backend, &Probe, &env(), None, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn status_shows_the_local_sandboxes_after_the_cloud_ones_and_before_the_runs() {
    let base = tempfile::tempdir().unwrap();
    let backend = FakeBackend::with_sandboxes(vec![info("sbxm-demo-claude", "running")])
        .with_cloud_sandboxes(vec![local("sbxm-task-issue-7-claude", "running", None)]);

    let text = frame(base.path(), &backend);

    let cloud = text.find("CLOUD SANDBOX").expect(&text);
    let local = text.find("sbxm-demo-claude  orphan").expect(&text);
    let runs = text.find("No comparison runs").expect(&text);
    assert!(cloud < local && local < runs, "{text}");
    // The local list doesn't hold the cloud's sandbox, nor the cloud's the local one.
    assert_eq!(
        text.matches("sbxm-task-issue-7-claude").count(),
        1,
        "{text}"
    );
    assert_eq!(text.matches("sbxm-demo-claude").count(), 1, "{text}");
}

#[test]
fn status_with_no_local_sandboxes_says_so_and_a_failing_list_does_not_fail_it() {
    let base = tempfile::tempdir().unwrap();

    assert!(frame(base.path(), &FakeBackend::default()).contains("\nNo local sandboxes.\n"));

    let text = frame(
        base.path(),
        &FakeBackend::default().failing_list_with("daemon down"),
    );
    assert!(
        text.contains("Local sandboxes couldn't be read (`sbx ls --json`): daemon down"),
        "{text}"
    );
}

#[test]
fn status_never_changes_a_sandbox() {
    let base = tempfile::tempdir().unwrap();
    let backend = FakeBackend::with_sandboxes(vec![info("sbxm-demo-claude", "running")]);

    let mut out = Vec::new();
    status::run(
        base.path(),
        &backend,
        &Probe,
        &env(),
        Some(Duration::from_secs(2)),
        &mut out,
    )
    .unwrap();

    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

fn unfinished_run(base: &Path, id: &str) {
    let dir = base.join(".sbxm").join("runs").join(id);
    fs::create_dir_all(&dir).unwrap();
    let json = serde_json::json!({
        "run_id": id,
        "started_at": "2026-09-21T14:00:00Z",
        "completed_at": null,
        "repeat": 1,
        "contestants": [
            {"index": 0, "harness": "claude", "model": "opus"},
            {"index": 1, "harness": "codex", "model": "gpt"},
        ],
    });
    fs::write(dir.join("run.json"), json.to_string()).unwrap();
}

#[test]
fn the_runs_section_reads_the_same_local_list() {
    let base = tempfile::tempdir().unwrap();
    unfinished_run(base.path(), "r1");
    let backend = FakeBackend::with_sandboxes(vec![info("sbxm-run-r1-0-0", "running")]);

    let text = frame(base.path(), &backend);
    assert!(text.contains("sbxm-run-r1-0-0  run r1"), "{text}");
    // Contestant 1's sandbox isn't in the list.
    assert!(text.contains("sandbox gone"), "{text}");

    let text = frame(
        base.path(),
        &FakeBackend::default().failing_list_with("daemon down"),
    );
    assert!(text.contains("sandbox unknown"), "{text}");
}
