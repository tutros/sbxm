//! Issue 671: what `sbxm status` shows that needs the user's eye. Where each task runs (the
//! WHERE column), a task the loop parked at `ready` with the command to run next, every open task
//! PR with its risk and whether auto-merge is armed, a dead loop with the tasks it left, and when
//! an idle loop next looks for work.

use crate::common;

use std::fs;
use std::io::Write;
use std::path::Path;

use common::Env as TestEnv;
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::commands::task_loop;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::run::results::format_timestamp;
use sbxm::status::looping::{self, Jobs, Lock, View};
use sbxm::status::tasks::{self, Parked, TaskRow, TaskTime, Workflow};
use sbxm::task::loop_pause;
use sbxm::task::outcome::{self, Outcome};
use sbxm::task::pipeline::Prepared;
use sbxm::task::queue::{LOOP_HOOK, LockFile, lock_path};
use sbxm::task::record::{
    self, Kind, NewTask, PrBranch, Process, ProcessProbe, Record, ReviewResult, SandboxLife,
    SandboxRole, Stage, Status,
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
            workflow: None,
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
            workflow: None,
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

/// Issue 41 worked into a commit of `files` in its repo.git, gated, reviewed at its tip with no
/// must-fix finding and `ready`, then claimed by the loop for `task finish` (review M-3).
fn claimed_ready_with(f: &common::task_fixture::Fixture, files: &[&str]) -> Prepared {
    use common::task_fixture::{CLAUDE_DONE, Play, backend, ok, play, worked_task};
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: files.iter().map(|s| (*s).to_owned()).collect(),
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    let repo_git = prepared.meta.join("repo.git");
    let tip = common::git(&repo_git, &["rev-parse", "issue-41"]);
    r.review = Some(ReviewResult {
        commit: Some(tip.clone()),
        ..review(Level::Low)
    });
    r.hooks = serde_json::json!({ LOOP_HOOK: true });
    record::write(&prepared.meta, &prepared.record).unwrap();
    let base = common::git(&repo_git, &["rev-parse", "main"]);
    fs::write(
        prepared.meta.join("review.md"),
        format!(
            "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nScope: `{base}..{tip}`\n\n\
             Nothing found.\n"
        ),
    )
    .unwrap();
    prepared
}

const WORKFLOW: &str = ".github/workflows/ci.yml";

#[test]
fn a_task_parked_by_the_workflow_guard_names_the_files_and_the_push_by_hand_before_finish() {
    let f = common::task_fixture::fixture();
    let b = f.env.base_dir();
    let prepared = claimed_ready_with(&f, &["a.txt", WORKFLOW]);
    let repo_git = prepared.meta.join("repo.git");
    let github = FakeGitHub::default();
    // The guard refuses the finish, so the task stays parked.
    let refused = sbxm::task::finish::finish(&b, "issue-41", &github, &common::task_fixture::Probe);
    assert!(format!("{:#}", refused.unwrap_err()).contains(WORKFLOW));

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].status, "parked");
    let parked = rows[0].parked.clone().unwrap();
    assert_eq!(
        parked.workflow,
        Some(Workflow {
            files: vec![WORKFLOW.to_owned()],
            repo_git: repo_git.to_string_lossy().into_owned(),
            branch: "issue-41".to_owned(),
        })
    );
    assert_eq!(parked.next, "sbxm task finish --issue 41");
    let text = tasks::render(&rows, 0, None);
    let shell = sbxm::hint::Shell::HOST;
    let line = format!(
        "issue-41 is parked; next: read {WORKFLOW} in {}; if the change is meant, push the \
         branch yourself (`git -C {} push origin issue-41`), then `sbxm task finish --issue 41`",
        repo_git.display(),
        shell.quote(&repo_git.to_string_lossy())
    );
    assert!(text.contains(&line), "{text}");

    // Doing what the line says: the push by hand, then the finish, which now opens the PR.
    common::git(&repo_git, &["push", "--quiet", "origin", "issue-41"]);
    let done =
        sbxm::task::finish::finish(&b, "issue-41", &github, &common::task_fixture::Probe).unwrap();
    assert!(done.branch_adopted.is_some(), "{done:?}");
}

#[test]
fn a_parked_task_without_a_workflow_change_is_told_to_finish() {
    let f = common::task_fixture::fixture();
    let b = f.env.base_dir();
    claimed_ready_with(&f, &["a.txt"]);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    let parked = rows[0].parked.clone().unwrap();
    assert_eq!(parked.workflow, None);
    assert_eq!(parked.next, "sbxm task finish --issue 41");
}

#[test]
fn a_parked_row_is_followed_by_its_next_command() {
    let mut r = row("issue-5", Some("local"));
    r.stage = "ready";
    r.status = "parked".to_owned();
    r.parked = Some(Parked {
        next: "sbxm task finish --issue 5".to_owned(),
        checkout: None,
        workflow: None,
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
        workflow: None,
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
        workflow: None,
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
        dead: None,
        next_check: None,
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

/// A fix task pushed onto PR `100 + number` (a resolve or a log-fix), with its review's `risk`;
/// `finished` or still at `ready`.
fn fix_of(number: u32, head: char, risk: Level, finished: bool) -> Record {
    let id = record::fix_id(100 + number, &head.to_string().repeat(40)).unwrap();
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Fix,
            number: 100 + number,
            repo: REPO,
            title: "",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: Some(&id),
        },
        T0,
        Process::new(1, T0),
    );
    rec.stage = if finished {
        Stage::Finished
    } else {
        Stage::Ready
    };
    rec.status = Status::Ok;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + number));
    rec.review = Some(review(risk));
    rec
}

/// The risk the open-PR list shows for PR 102, given its opener's risk and its fixes.
fn shown_risk(opener: Level, fixes: &[Record]) -> String {
    shown_risk_of(Some(opener), fixes)
}

/// [`shown_risk`], with no opener record at all when `opener` is `None`.
fn shown_risk_of(opener: Option<Level>, fixes: &[Record]) -> String {
    let base = TestEnv::new();
    let b = base.base_dir();
    if let Some(opener) = opener {
        save(&b, &with_pr(2, Some(opener), true));
    }
    for fix in fixes {
        save(&b, fix);
    }
    let gh = FakeGitHub::default().with_pr(pr(102, PrState::Open));
    let text = section(&b, &gh);
    assert!(text.contains("  open task PRs: 1\n"), "{text}");
    text.lines()
        .find(|l| l.starts_with("    #102"))
        .unwrap_or_else(|| panic!("{text}"))
        .to_owned()
}

#[test]
fn a_finished_high_risk_fix_raises_a_low_risk_openers_pr() {
    // Fix ids before and after the opener's in name order, so neither read order decides.
    for head in ['0', 'f'] {
        let line = shown_risk(Level::Low, &[fix_of(2, head, Level::High, true)]);
        assert!(line.contains("— high risk"), "{line}");
    }
}

#[test]
fn a_later_low_risk_fix_does_not_clear_a_high_risk_openers_pr() {
    for head in ['0', 'f'] {
        let line = shown_risk(Level::High, &[fix_of(2, head, Level::Low, true)]);
        assert!(line.contains("— high risk"), "{line}");
    }
}

#[test]
fn a_fix_not_finished_yet_does_not_count_toward_its_prs_risk() {
    let line = shown_risk(Level::Low, &[fix_of(2, 'a', Level::High, false)]);
    assert!(line.contains("— low risk"), "{line}");
}

/// An issue task `number` that continued PR 102 (decision 169), with its review's `risk`;
/// `finished` or still at `ready`.
fn continuation(number: u32, risk: Level, finished: bool) -> Record {
    let mut rec = task(number);
    rec.stage = if finished {
        Stage::Finished
    } else {
        Stage::Ready
    };
    rec.status = Status::Ok;
    rec.continues = Some(PrBranch {
        pr: 102,
        branch: "b102".to_owned(),
        base: "0".repeat(40),
    });
    rec.pr = Some(format!("https://github.com/{REPO}/pull/102"));
    rec.review = Some(review(risk));
    rec
}

#[test]
fn a_finished_continuation_alone_shows_its_own_risk() {
    let line = shown_risk_of(None, &[continuation(3, Level::High, true)]);
    assert!(line.contains("— high risk"), "{line}");
}

#[test]
fn a_finished_high_risk_continuation_raises_a_low_risk_openers_pr() {
    // Ids before and after the opener's (`issue-2`) in name order.
    for number in [1, 9] {
        let line = shown_risk(Level::Low, &[continuation(number, Level::High, true)]);
        assert!(line.contains("— high risk"), "{line}");
    }
}

#[test]
fn a_low_risk_continuation_does_not_lower_a_high_risk_openers_pr() {
    for number in [1, 9] {
        let line = shown_risk(Level::High, &[continuation(number, Level::Low, true)]);
        assert!(line.contains("— high risk"), "{line}");
    }
}

#[test]
fn a_continuation_not_finished_yet_does_not_count_toward_its_prs_risk() {
    let line = shown_risk(Level::Low, &[continuation(3, Level::High, false)]);
    assert!(line.contains("— low risk"), "{line}");
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

// ---- 4. a dead loop and the tasks it left -----------------------------------------------------

/// When the loop in these fixtures started.
const LOOP_START: u64 = T0 - 1_000;

/// The loop's process: pid 1.
fn loop_process() -> Process {
    Process::new(1, LOOP_START)
}

/// A child the loop started: pid 2.
fn child() -> Process {
    Process::new(2, T0)
}

/// Which pids are alive, each with its start time.
struct Alive(Vec<(u32, u64)>);

impl ProcessProbe for Alive {
    fn start_time(&self, pid: u32) -> Option<u64> {
        self.0.iter().find(|(p, _)| *p == pid).map(|(_, t)| *t)
    }
}

fn write_loop_lock(base: &Path) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: loop_process(),
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: Vec::new(),
        idle_check: None,
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
}

/// A task the loop's child runs: the loop recorded as its owner.
fn loops_child(number: u32, stage: Stage, status: Status) -> Record {
    let mut rec = task(number);
    rec.stage = stage;
    rec.status = status;
    rec.process = Some(child());
    rec.loop_owner = Some(loop_process());
    rec
}

fn quiet_disk() -> sbxm::status::footer::Disk {
    sbxm::status::footer::Disk {
        free_space: |_| Ok(40 * 1024 * 1024 * 1024),
        same_drive: |_, _| true,
        state_dir: Some(std::path::PathBuf::from("/state")),
        min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
    }
}

fn env(now: u64) -> Env {
    Env {
        placement: Default::default(),
        now: Box::new(move || now),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 80,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: quiet_disk(),
    }
}

/// One frame's Loop section, up to the disk footer.
fn loop_section(base: &Path, probe: &dyn ProcessProbe, now: u64) -> String {
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
        &env(now),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let section = text.split("\nLoop:").nth(1).expect("a Loop section");
    format!("Loop:{}", section.split("\nFree space:").next().unwrap())
}

/// The tasks a dead loop could leave: a child still running, one at `ready` whose last command
/// failed, one at `ready` that went well, and a running child of another loop.
fn leave_tasks(base: &Path) {
    save(base, &loops_child(11, Stage::Working, Status::Running));
    let failed = loops_child(12, Stage::Ready, Status::Ok);
    save(base, &failed);
    outcome::write(
        &record::task_dir(base, &failed.id),
        &Outcome::new(outcome::Kind::Failed, None, "finish refused\nmore"),
    )
    .unwrap();
    let fine = loops_child(13, Stage::Ready, Status::Ok);
    save(base, &fine);
    outcome::write(
        &record::task_dir(base, &fine.id),
        &Outcome::new(outcome::Kind::Ready, None, ""),
    )
    .unwrap();
    let mut other = loops_child(14, Stage::Working, Status::Running);
    other.loop_owner = Some(Process::new(7, LOOP_START));
    save(base, &other);
}

#[test]
fn no_loop_ever_run_leaves_nothing() {
    let base = TestEnv::new();
    let b = base.base_dir();
    leave_tasks(&b);

    let text = loop_section(&b, &Alive(vec![(2, T0)]), T0);

    assert!(text.starts_with("Loop: not running\n"), "{text}");
    assert!(!text.contains("left:"), "{text}");
}

#[test]
fn a_running_loop_has_no_orphans() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_loop_lock(&b);
    leave_tasks(&b);

    let text = loop_section(&b, &Alive(vec![(1, LOOP_START), (2, T0)]), T0);

    assert!(text.starts_with("Loop: running (process 1"), "{text}");
    assert!(!text.contains("left:"), "{text}");
    assert!(!text.contains("last seen"), "{text}");
}

#[test]
fn a_dead_loop_says_when_it_was_last_seen_and_lists_the_tasks_it_left() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_loop_lock(&b);
    leave_tasks(&b);
    let written = fs::metadata(lock_path(&b))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let text = loop_section(&b, &Alive(vec![(2, T0)]), T0);

    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap(),
        format!(
            "Loop: not running (stale lock: process 1, started {} is gone; last seen {})",
            format_timestamp(LOOP_START),
            format_timestamp(written)
        ),
        "{text}"
    );
    assert_eq!(
        lines.next().unwrap(),
        "  left: issue-11 (title 11) at working, local: still running (process 2)",
        "{text}"
    );
    assert_eq!(
        lines.next().unwrap(),
        "  left: issue-12 (title 12) at ready, local: its last command failed (finish refused)",
        "{text}"
    );
    assert!(!text.contains("issue-13"), "{text}");
    assert!(!text.contains("issue-14"), "{text}");
}

/// Sets `path`'s modified time to `secs`.
fn touch(path: &Path, secs: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn a_dead_loop_was_last_seen_at_its_logs_last_write_when_that_is_later() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_loop_lock(&b);
    touch(&lock_path(&b), LOOP_START + 10);
    let log = sbxm::task::loop_log::path(&b, LOOP_START);
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "| row |\n").unwrap();
    touch(&log, LOOP_START + 500);

    let text = loop_section(&b, &Alive(Vec::new()), T0);

    assert!(
        text.contains(&format!(
            "is gone; last seen {})",
            format_timestamp(LOOP_START + 500)
        )),
        "{text}"
    );
}

#[test]
fn a_child_the_dead_loop_had_adopted_is_left_too() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let earlier = Process::new(7, LOOP_START - 50);
    let path = lock_path(&b);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: loop_process(),
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: vec![sbxm::task::queue::AdoptedChild {
            issue: 15,
            owner: earlier.clone(),
        }],
        idle_check: None,
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
    let mut adopted = loops_child(15, Stage::Reviewing, Status::Running);
    adopted.loop_owner = Some(earlier);
    save(&b, &adopted);

    let text = loop_section(&b, &Alive(vec![(2, T0)]), T0);

    assert!(
        text.contains(
            "  left: issue-15 (title 15) at reviewing, local: still running (process 2)\n"
        ),
        "{text}"
    );
}

#[test]
fn a_dead_loops_child_that_died_too_is_not_a_live_child() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_loop_lock(&b);
    save(&b, &loops_child(11, Stage::Working, Status::Running));

    let text = loop_section(&b, &Alive(Vec::new()), T0);

    assert!(text.starts_with("Loop: not running (stale lock"), "{text}");
    assert!(!text.contains("left:"), "{text}");
}

// ---- 5. when an idle loop next looks for work -------------------------------------------------

/// The loop's lock, its process alive, saying it began an idle wait at `began`.
fn write_idle_lock(base: &Path, began: Option<u64>) {
    let path = lock_path(base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = LockFile {
        process: loop_process(),
        issues: Vec::new(),
        cap: None,
        progress: None,
        adopted: Vec::new(),
        idle_check: began.map(format_timestamp),
    };
    fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
}

const POLL: u64 = sbxm::commands::task_loop::PAUSE_POLL.as_secs();

#[test]
fn an_idle_loop_says_when_it_next_looks_for_work() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_idle_lock(&b, Some(T0 - 60));

    let text = loop_section(&b, &Alive(vec![(1, LOOP_START)]), T0);

    assert!(
        text.contains(&format!(
            "\n  idle: next check for work in 4m (at {})\n",
            format_timestamp(T0 - 60 + POLL)
        )),
        "{text}"
    );
}

#[test]
fn an_idle_loop_at_its_poll_time_checks_now() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_idle_lock(&b, Some(T0 - POLL));

    let text = loop_section(&b, &Alive(vec![(1, LOOP_START)]), T0);

    assert!(
        text.contains("\n  idle: next check for work now\n"),
        "{text}"
    );
}

#[test]
fn an_idle_loop_one_second_before_its_poll_time_checks_in_1s() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_idle_lock(&b, Some(T0 - POLL + 1));

    let text = loop_section(&b, &Alive(vec![(1, LOOP_START)]), T0);

    assert!(
        text.contains("  idle: next check for work in 1s (at "),
        "{text}"
    );
}

#[test]
fn an_idle_loop_past_its_poll_time_checks_now() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_idle_lock(&b, Some(T0 - POLL - 30));

    let text = loop_section(&b, &Alive(vec![(1, LOOP_START)]), T0);

    assert!(
        text.contains("\n  idle: next check for work now\n"),
        "{text}"
    );
}

#[test]
fn a_working_loop_or_a_dead_one_shows_no_next_check() {
    let base = TestEnv::new();
    let b = base.base_dir();
    write_idle_lock(&b, None);
    let working = loop_section(&b, &Alive(vec![(1, LOOP_START)]), T0);
    write_idle_lock(&b, Some(T0 - 60));
    let dead = loop_section(&b, &Alive(Vec::new()), T0);

    assert!(!working.contains("idle:"), "{working}");
    assert!(!dead.contains("idle:"), "{dead}");
}

/// The loop's steps, none expected but the queued `rm-merged` job's, which notes what the lock
/// says while it runs.
struct IdleSteps {
    base: std::path::PathBuf,
    during_job: std::cell::RefCell<Vec<Option<String>>>,
}

fn unexpected(what: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("unexpected {what}"))
}

impl task_loop::Steps for IdleSteps {
    fn run(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        unexpected("run")
    }

    fn resume(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        unexpected("resume")
    }

    fn finish(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        unexpected("finish")
    }

    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        unexpected("file-findings")
    }

    fn fix_pr(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        unexpected("resolve")
    }

    fn fix_check(
        &self,
        _: u32,
        _: u64,
        _: &mut dyn Write,
        _: &mut dyn Write,
    ) -> anyhow::Result<()> {
        unexpected("log-fix")
    }

    fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> anyhow::Result<()> {
        let running = sbxm::task::jobs::running_dir(&self.base);
        if fs::read_dir(&running).is_ok_and(|mut entries| entries.next().is_some()) {
            self.during_job.borrow_mut().push(idle_check(&self.base));
        }
        Ok(())
    }

    fn room(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn idle_check(base: &Path) -> Option<String> {
    let text = fs::read_to_string(lock_path(base)).unwrap();
    serde_json::from_str::<LockFile>(&text).unwrap().idle_check
}

#[test]
fn a_waiting_loop_writes_when_it_began_each_idle_wait_and_clears_it_to_work() {
    idle_waits_are_written(None);
}

/// The same with `cloud_slots` set, which runs the loop's other scheduler (review M-1).
#[test]
fn a_waiting_loop_with_cloud_slots_writes_its_idle_wait_too() {
    idle_waits_are_written(Some(2));
}

/// A `task loop --wait` on an empty queue, with `cloud_slots` as given: at each idle sleep its
/// lock says when the wait began, and while it runs the job queued meanwhile it says none.
fn idle_waits_are_written(cloud_slots: Option<u32>) {
    let env = TestEnv::new();
    if let Some(n) = cloud_slots {
        common::cloud_slot::set_cloud_slots(&env, n);
    }
    let b = env.base_dir();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_open_issues(Vec::new());
    let steps = IdleSteps {
        base: b.clone(),
        during_job: Default::default(),
    };
    let clock = std::cell::Cell::new(T0);
    let seen = std::cell::RefCell::new(Vec::new());
    let stop = std::cell::Cell::new(false);
    let now = || clock.get();
    let sleep = |d: std::time::Duration| {
        // What the lock says while the loop sleeps, and when it began the wait.
        seen.borrow_mut().push((idle_check(&b), clock.get()));
        clock.set(clock.get() + d.as_secs());
        match seen.borrow().len() {
            1 => {
                let queue = sbxm::task::jobs::queue_dir(&b);
                fs::create_dir_all(&queue).unwrap();
                fs::write(queue.join("a.json"), r#"{"kind": "rm-merged"}"#).unwrap();
            }
            _ => stop.set(true),
        }
    };
    let stop_requested = || stop.get();
    let loop_env = task_loop::Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop_requested,
    };
    let opts = task_loop::Options {
        repo_root,
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: true,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    task_loop::run(
        &env.config_dir(),
        &opts,
        &steps,
        &gh,
        &common::task_fixture::Probe,
        &loop_env,
        &mut out,
        &mut warn,
    )
    .unwrap();

    let out = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 2, "{out}");
    for (said, began) in &seen {
        assert_eq!(
            said.as_deref(),
            Some(format_timestamp(*began).as_str()),
            "{out}"
        );
    }
    assert_eq!(steps.during_job.into_inner(), [None], "{out}");
}
