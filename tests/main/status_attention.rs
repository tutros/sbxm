//! Issue 671: what `sbxm status` shows that needs the user's eye. Where each task runs (the
//! WHERE column), a task the loop parked at `ready` with the command to run next, every open task
//! PR with its risk and whether auto-merge is armed, a dead loop with the tasks it left, and when
//! an idle loop next looks for work.

use crate::common;

use std::fs;
use std::path::Path;

use common::Env as TestEnv;
use sbxm::backend::Location;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::status::looping::{self, Jobs, Lock, View};
use sbxm::status::tasks::{self, Parked, TaskRow, TaskTime};
use sbxm::task::loop_pause;
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, ReviewResult, SandboxLife, SandboxRole,
    Stage, Status,
};
use sbxm::task::risk::Level;

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
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

fn life(name: &str, removed: bool) -> SandboxLife {
    SandboxLife {
        name: name.to_owned(),
        role: SandboxRole::Worker,
        location: Location::Cloud,
        created_at: "2026-11-15T10:00:00Z".to_owned(),
        ttl_s: 3_600,
        removed_at: removed.then(|| "2026-11-15T11:00:00Z".to_owned()),
    }
}

fn row(id: &str, location: Option<&str>) -> TaskRow {
    TaskRow {
        id: id.to_owned(),
        title: String::new(),
        stage: "working",
        status: "running".to_owned(),
        time: TaskTime::InStage(None),
        commits_ahead: None,
        must_fix: None,
        pr: None,
        location: location.map(str::to_owned),
        parked: None,
        log_line: None,
    }
}

// ---- 1. where each task runs ----------------------------------------------------------------

#[test]
fn a_local_task_runs_local() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &task(1));

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("local"));
}

#[test]
fn a_cloud_task_names_its_sandbox_still_there() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = task(2);
    rec.location = Location::Cloud;
    rec.sandboxes = vec![life("old-box", true), life("live-box", false)];
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("cloud (live-box)"));
}

#[test]
fn a_cloud_task_without_a_sandbox_is_cloud_alone() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = task(3);
    rec.location = Location::Cloud;
    rec.sandboxes = vec![life("gone-box", true)];
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("cloud"));
}

#[test]
fn an_older_record_without_a_location_field_has_none() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let rec = task(4);
    save(&b, &rec);
    let path = record::task_dir(&b, &rec.id).join("task.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("task.json is JSON");
    json.as_object_mut().unwrap().remove("location");
    fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].location, None);
}

#[test]
fn the_where_column_shows_each_location_and_a_dash_for_none() {
    let text = tasks::render(
        &[
            row("issue-1", Some("local")),
            row("issue-2", Some("cloud (live-box)")),
            row("issue-3", None),
        ],
        0,
        None,
    );

    insta::assert_snapshot!(text, @r"
    ID       STAGE    STATUS   TIME  AHEAD  FINDINGS  PR  WHERE             LOG
    issue-1  working  running  -     -      -         -   local             -
    issue-2  working  running  -     -      -         -   cloud (live-box)  -
    issue-3  working  running  -     -      -         -   -                 -
    ");
}

// ---- 2. a task parked at `ready`, with its next step ------------------------------------------

/// A task the loop took to `ready` and claimed for `task finish` (its marker set), with no PR:
/// its finish failed (the workflow-file guard, say), so it waits on the user.
fn parked(number: u32) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    rec
}

fn hold_lock(base: &Path, id: &str, process: &Process) {
    let mut json = serde_json::to_value(process).unwrap();
    json["token"] = "t".into();
    fs::write(
        record::task_dir(base, id).join("lock.json"),
        json.to_string(),
    )
    .unwrap();
}

#[test]
fn a_ready_task_the_loop_claimed_without_a_pr_is_parked_with_its_finish_next() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &parked(5));

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].status, "parked");
    assert_eq!(
        rows[0].parked,
        Some(Parked {
            next: "sbxm task finish --issue 5".to_owned(),
            checkout: None,
        })
    );
}

#[test]
fn a_parked_task_with_must_fix_findings_names_their_next_step() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = parked(6);
    rec.review = Some(ReviewResult {
        round: 1,
        commit: None,
        posted: None,
        must_fix: 2,
        full: true,
        repeat: false,
        risk: sbxm::task::risk::Level::Medium,
        risk_reasons: vec![],
        not_run: None,
    });
    rec.checkout = Some("/work/repo".to_owned());
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(
        rows[0].parked,
        Some(Parked {
            next: "fix them by hand, or file them: sbxm task file-findings --issue 6".to_owned(),
            checkout: Some("/work/repo".to_owned()),
        })
    );
}

#[test]
fn a_ready_task_run_by_hand_is_not_parked() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = parked(7);
    rec.hooks = serde_json::json!({});
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].status, "ok");
    assert_eq!(rows[0].parked, None);
}

#[test]
fn a_claimed_task_whose_finish_is_running_is_not_parked() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &parked(8));
    hold_lock(&b, "issue-8", &Process::new(9, T0));

    let rows = tasks::rows(&b, &Probe(Some(T0))).unwrap();

    assert_eq!(rows[0].status, "ok");
    assert_eq!(rows[0].parked, None);
}

#[test]
fn a_claimed_task_with_a_pr_is_not_parked() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = parked(9);
    rec.pr = Some(format!("https://github.com/{REPO}/pull/109"));
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].parked, None);
}

#[test]
fn a_parked_row_is_followed_by_its_next_command() {
    let mut r = row("issue-5", Some("local"));
    r.stage = "ready";
    r.status = "parked".to_owned();
    r.parked = Some(Parked {
        next: "sbxm task finish --issue 5".to_owned(),
        checkout: None,
    });

    let text = tasks::render(&[r, row("issue-6", Some("local"))], 0, None);

    assert!(
        text.ends_with("\nissue-5 is parked; next: `sbxm task finish --issue 5`\n"),
        "{text}"
    );
    assert_eq!(text.lines().count(), 4, "{text}");
}

#[test]
fn a_parked_row_puts_the_command_of_a_longer_next_step_in_backticks() {
    let mut r = row("issue-6", Some("local"));
    r.parked = Some(Parked {
        next: "fix them by hand, or file them: sbxm task file-findings --issue 6".to_owned(),
        checkout: None,
    });

    let text = tasks::render(&[r], 0, None);

    assert!(
        text.contains(
            "issue-6 is parked; next: fix them by hand, or file them: `sbxm task file-findings \
             --issue 6`\n"
        ),
        "{text}"
    );
}

#[test]
fn the_dashboard_names_a_parked_tasks_checkout_and_config_dir_as_the_run_hints_do() {
    let env = TestEnv::new();
    let b = env.base_dir();
    let checkout = env.tmp.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    let mut rec = parked(5);
    rec.checkout = Some(checkout.to_string_lossy().into_owned());
    save(&b, &rec);
    let elsewhere = env.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(&elsewhere)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .arg("status")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let shell = sbxm::hint::Shell::HOST;
    assert!(stdout.contains(" task finish --issue 5` (in "), "{stdout}");
    assert!(!stdout.contains("`sbxm task finish"), "{stdout}");
    assert!(
        stdout.contains(&shell.cd(&checkout.to_string_lossy())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&shell.set_config_dir(&env.config_dir().to_string_lossy())),
        "{stdout}"
    );
}

#[test]
fn a_parked_row_names_the_checkout_its_command_runs_in() {
    let mut r = row("issue-5", Some("local"));
    r.status = "parked".to_owned();
    r.parked = Some(Parked {
        next: "sbxm task finish --issue 5".to_owned(),
        checkout: Some("/work/repo".to_owned()),
    });

    let text = tasks::render(&[r], 0, None);

    assert!(
        text.contains("issue-5 is parked; next: `sbxm task finish --issue 5` (in /work/repo)\n"),
        "{text}"
    );
}

// ---- 3. every open task PR, with its risk and auto-merge ---------------------------------------

fn review(risk: Level) -> ReviewResult {
    ReviewResult {
        round: 1,
        commit: None,
        posted: None,
        must_fix: 0,
        full: true,
        repeat: false,
        risk,
        risk_reasons: vec![],
        not_run: None,
    }
}

/// A finished task whose PR is `100 + number`, with the review's `risk` when given.
fn with_pr(number: u32, risk: Option<Level>, marked: bool) -> Record {
    let mut rec = task(number);
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + number));
    rec.review = risk.map(review);
    if marked {
        rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    }
    rec
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: "abc".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// The Loop section with no loop, over GitHub's answers.
fn section(base: &Path, gh: &FakeGitHub) -> String {
    let view = View {
        lock: Lock::Absent,
        work: None,
        snapshot: Some(looping::fetch(gh, &Ok(REPO.into()), base)),
        pause: loop_pause::State::None,
        jobs: Jobs::default(),
        cloud_first: false,
    };
    looping::render(&view, None)
}

#[test]
fn every_open_task_pr_shows_its_risk_and_whether_auto_merge_is_armed() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &with_pr(2, Some(Level::Medium), true));
    save(&b, &with_pr(3, Some(Level::Low), true));
    // Finished by hand, before its review recorded a risk.
    save(&b, &with_pr(4, None, false));
    save(&b, &with_pr(5, Some(Level::High), true));
    let gh = FakeGitHub::default()
        .with_pr(pr(102, PrState::Open))
        .with_pr(pr(103, PrState::Open))
        .with_auto_merge(103)
        .with_pr(pr(104, PrState::Open))
        .with_draft(104)
        .with_pr(pr(105, PrState::Merged));

    let text = section(&b, &gh);

    assert!(
        text.contains(
            "  awaiting you: 1 of 5 PRs (#102: title 2)\n  open task PRs: 3\n    #102: title 2 \
             — medium risk, not armed: awaits your merge\n    #103: title 3 — low risk, \
             auto-merge armed\n    #104: title 4 — unknown risk, draft, not armed: awaits your \
             merge\n"
        ),
        "{text}"
    );
    assert!(!text.contains("#105"), "{text}");
}

#[test]
fn no_open_task_pr_leaves_the_list_out() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &with_pr(5, Some(Level::High), true));
    let gh = FakeGitHub::default().with_pr(pr(105, PrState::Merged));

    let text = section(&b, &gh);

    assert!(text.contains("  awaiting you: 0 of 5 PRs\n"), "{text}");
    assert!(!text.contains("open task PRs"), "{text}");
}

#[test]
fn a_github_failure_is_one_line_and_the_rest_still_shows() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &with_pr(2, Some(Level::Medium), true));
    let gh = FakeGitHub::default().failing("offline");

    let text = section(&b, &gh);

    assert!(
        text.contains("  open task PRs: unreadable (offline)\n"),
        "{text}"
    );
    assert_eq!(text.matches("open task PRs").count(), 1, "{text}");
    assert!(text.contains("  queue: unreadable (offline)\n"), "{text}");
    assert!(text.contains("  jobs: none\n"), "{text}");
}
