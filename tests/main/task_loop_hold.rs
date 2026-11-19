//! Issue 727, decision 331(3): a loop task whose changed files overlap other open work once its
//! gates pass is held before its review, then released, merges the base, runs its gates again
//! and is reviewed once. One test per boundary case of the issue.

use crate::common;

use std::fs;
use std::path::Path;

use common::git;
use common::task_fixture::Probe;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrFile, PrFiles, PrInfo, PrState};
use sbxm::run::results::format_timestamp;
use sbxm::task::hold::{self, Check, Checker, Wait};
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Stamp, Status};

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;

/// The process the fake probe says is alive.
fn alive() -> Process {
    Process::new(1, 0)
}

/// A task `issue-<n>` in `base` whose branch changes `files` past `main`, at `stage` and
/// `status`, its gates started at `gates_at`.
fn task(
    base: &Path,
    n: u32,
    files: &[&str],
    stage: Stage,
    status: Status,
    gates_at: u64,
) -> Record {
    let id = format!("issue-{n}");
    let dir = record::task_dir(base, &id);
    let repo_git = dir.join("repo.git");
    fs::create_dir_all(&repo_git).unwrap();
    let scratch = base.join(format!("scratch-{n}"));
    fs::create_dir_all(&scratch).unwrap();
    git(&scratch, &["init", "-q", "-b", "main"]);
    fs::write(scratch.join("README.md"), "base\n").unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "base"]);
    git(&scratch, &["checkout", "-q", "-b", &id]);
    for file in files {
        let path = scratch.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{id}\n")).unwrap();
    }
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "work"]);
    git(&repo_git, &["init", "-q", "--bare"]);
    git(
        &scratch,
        &[
            "push",
            "-q",
            repo_git.to_str().unwrap(),
            "main:refs/heads/main",
            &format!("{id}:refs/heads/{id}"),
        ],
    );
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: n,
            repo: REPO,
            title: "",
            base: "main",
            branch: &id,
            config_hash: "h",
            id: None,
        },
        START,
        alive(),
    );
    rec.stage = stage;
    rec.status = status;
    rec.stages.push(Stamp {
        stage: Stage::Gating,
        at: format_timestamp(gates_at),
    });
    if stage != Stage::Gating {
        rec.stages.push(Stamp {
            stage,
            at: format_timestamp(gates_at + 60),
        });
    }
    record::write(&dir, &rec).unwrap();
    rec
}

/// A finished loop task `issue-<n>` that opened PR `pr`.
fn loop_pr(base: &Path, n: u32, pr: u32) {
    let id = format!("issue-{n}");
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: n,
            repo: REPO,
            title: "",
            base: "main",
            branch: &id,
            config_hash: "h",
            id: None,
        },
        START,
        alive(),
    );
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.process = None;
    rec.pr = Some(format!("https://github.com/o/r/pull/{pr}"));
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    record::write(&record::task_dir(base, &id), &rec).unwrap();
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("issue-{number}"),
        head_oid: "0123456789abcdef0123456789abcdef01234567".into(),
        is_cross_repository: false,
        head_repo: REPO.into(),
        state,
        title: format!("PR {number}"),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    }
}

fn files(paths: &[&str]) -> PrFiles {
    PrFiles {
        changed_files: paths.len() as u32,
        files: paths
            .iter()
            .map(|p| PrFile {
                filename: (*p).to_owned(),
                previous_filename: None,
            })
            .collect(),
    }
}

fn waits(base: &Path, github: &FakeGitHub, own: &Record) -> Vec<Wait> {
    let repo_git = record::task_dir(base, &own.id).join("repo.git");
    let paths = own.changed_paths(&repo_git).unwrap();
    Checker {
        base_dir: base,
        repo: REPO,
        github,
        probe: &Probe,
    }
    .waits(own, &paths)
    .unwrap()
}

#[test]
fn task_loop_hold_no_overlap_is_not_held() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Open))
        .with_pr_files(30, files(&["src/other.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    assert_eq!(waits(base, &github, &own), Vec::<Wait>::new());
}

#[test]
fn task_loop_hold_one_shared_file_with_an_open_loop_pr_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Open))
        .with_pr_files(30, files(&["docs/x.md", "src/a.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    let on = waits(base, &github, &own);
    assert_eq!(
        on,
        vec![Wait::Pr {
            number: 30,
            armed: false
        }]
    );
    assert_eq!(
        hold::waits_line(&on),
        "waits on #30 (PR, awaits your merge)"
    );
}

#[test]
fn task_loop_hold_a_shared_file_with_a_task_in_review_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    task(
        base,
        8,
        &["src/a.rs"],
        Stage::Reviewing,
        Status::Running,
        START,
    );
    let github = FakeGitHub::default();
    let own = task(
        base,
        9,
        &["src/a.rs"],
        Stage::Gating,
        Status::Passed,
        START + 60,
    );
    let on = waits(base, &github, &own);
    assert_eq!(
        on,
        vec![Wait::Task {
            task: "issue-8".into(),
            stage: Stage::Reviewing
        }]
    );
    assert_eq!(hold::waits_line(&on), "waits on issue-8 (in review)");
}

#[test]
fn task_loop_hold_a_shared_file_only_with_a_merged_pr_is_not_held() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    loop_pr(base, 3, 30);
    let github = FakeGitHub::default()
        .with_pr(pr(30, PrState::Merged))
        .with_pr_files(30, files(&["src/a.rs"]));
    let own = task(base, 9, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    assert_eq!(waits(base, &github, &own), Vec::<Wait>::new());
}

#[test]
fn task_loop_hold_two_tasks_whose_gates_pass_together_the_first_holds_the_second_only() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let github = FakeGitHub::default();
    let first = task(base, 8, &["src/a.rs"], Stage::Gating, Status::Passed, START);
    let second = task(
        base,
        9,
        &["src/a.rs"],
        Stage::Gating,
        Status::Passed,
        START + 1,
    );
    // Both see the other with its gates passed: only the later one waits.
    assert_eq!(waits(base, &github, &first), Vec::<Wait>::new());
    assert_eq!(
        waits(base, &github, &second),
        vec![Wait::Task {
            task: "issue-8".into(),
            stage: Stage::Gating
        }]
    );
    // Once the second is held, it holds nothing: the first goes on to its review.
    let mut held = second.clone();
    held.stage = Stage::Held;
    held.status = Status::Ok;
    hold::start(&mut held, START + 2, vec![]);
    record::write(&record::task_dir(base, &held.id), &held).unwrap();
    assert_eq!(waits(base, &github, &first), Vec::<Wait>::new());
}

// The hold point in the pipeline: after the gates pass, before the first review.

/// A [`Check`] that answers `waits` and counts its calls.
struct FakeCheck {
    waits: Vec<Wait>,
    calls: std::cell::Cell<u32>,
}

impl FakeCheck {
    fn new(waits: Vec<Wait>) -> Self {
        Self {
            waits,
            calls: std::cell::Cell::new(0),
        }
    }
}

impl Check for FakeCheck {
    fn waits(&self, _task: &Record, paths: &[String]) -> anyhow::Result<Vec<Wait>> {
        self.calls.set(self.calls.get() + 1);
        assert!(!paths.is_empty(), "the task's changed paths are checked");
        Ok(self.waits.clone())
    }
}

mod pipeline_hold {
    use super::*;
    use common::task_fixture::{
        CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks,
        worked_task,
    };
    use sbxm::backend::FakeBackend;
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::outcome;
    use sbxm::task::pipeline::{self, TaskEnv};

    pub const CLEAN: &str = "Must-fix findings: 0\nRisk: low\n\nNothing.\n";

    pub fn config() -> Fixture {
        fixture_with(
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
             [reviewer]\nharness = \"codex\"\n",
        )
    }

    /// The worker commits `a.txt`; every review is clean.
    pub fn backend(f: &Fixture) -> FakeBackend {
        let worker = play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        );
        let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
        FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_exec_output_matching("claude", ok(CLAUDE_DONE))
            .with_exec_output_matching("codex", ok(CODEX_DONE))
            .with_exec_hook(move |sandbox, spec| {
                worker(sandbox, spec);
                reviewer(sandbox, spec);
            })
    }

    pub fn env<'a>(
        f: &'a Fixture,
        backend: &'a FakeBackend,
        hold: Option<&'a dyn Check>,
    ) -> TaskEnv<'a> {
        let host: &'static FakeHostRunner = Box::leak(Box::default());
        TaskEnv {
            progress: &pipeline::quiet,
            config_dir: Box::leak(Box::new(f.env.config_dir())),
            config: &f.config,
            backend,
            probe: &Probe,
            host,
            sleep: &std::thread::sleep,
            scoped_secrets: &[],
            disk: Box::leak(Box::default()),
            identity: Some(&f.identity),
            clone_source: None,
            hold,
        }
    }

    pub fn saved(f: &Fixture) -> Record {
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap()
    }

    pub fn reviews_run(backend: &FakeBackend) -> usize {
        backend
            .execs()
            .iter()
            .filter(|(sandbox, spec)| {
                sandbox.contains("-review-") && spec.argv.iter().any(|a| a == "codex")
            })
            .count()
    }

    #[test]
    fn task_loop_hold_an_overlap_after_the_gates_holds_the_task_before_its_review() {
        let f = config();
        let backend = backend(&f);
        let mut prepared = worked_task(&f, &backend);
        let check = FakeCheck::new(vec![Wait::Pr {
            number: 30,
            armed: false,
        }]);
        let err =
            pipeline::review_issue(&env(&f, &backend, Some(&check)), &mut prepared).unwrap_err();
        assert!(err.is::<pipeline::OnHold>(), "{err:#}");
        assert_eq!(outcome::kind_of(&err, ""), outcome::Kind::OnHold);
        assert!(
            format!("{err:#}").contains("waits on #30 (PR, awaits your merge)"),
            "{err:#}"
        );
        let rec = saved(&f);
        assert_eq!((rec.stage, rec.status), (Stage::Held, Status::Ok));
        assert_eq!(rec.holds.len(), 1);
        assert_eq!(rec.holds[0].on, check.waits);
        assert_eq!(rec.holds[0].released, None);
        assert_eq!(check.calls.get(), 1);
        assert_eq!(reviews_run(&backend), 0, "no review while held");
    }

    #[test]
    fn task_loop_hold_no_overlap_after_the_gates_goes_on_to_the_review() {
        let f = config();
        let backend = backend(&f);
        let mut prepared = worked_task(&f, &backend);
        let check = FakeCheck::new(vec![]);
        pipeline::review_issue(&env(&f, &backend, Some(&check)), &mut prepared).unwrap();
        let rec = saved(&f);
        assert_eq!(rec.stage, Stage::Ready);
        assert!(rec.holds.is_empty());
        assert_eq!(check.calls.get(), 1);
        assert_eq!(reviews_run(&backend), 1);
    }

    #[test]
    fn task_loop_hold_a_task_run_by_hand_is_never_held() {
        let f = config();
        let backend = backend(&f);
        let mut prepared = worked_task(&f, &backend);
        pipeline::review_issue(&env(&f, &backend, None), &mut prepared).unwrap();
        assert_eq!(saved(&f).stage, Stage::Ready);
    }
}

// The release: the base merged in by the worker, the gates again, then the one review.

mod release {
    use super::pipeline_hold::{CLEAN, config, reviews_run, saved};
    use super::*;
    use common::task_fixture::{CLAUDE_DONE, CODEX_DONE, Fixture, is_bundle, is_headless, ok};
    use common::task_fixture::{play_reviews, worked_task};
    use sbxm::backend::FakeBackend;
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::pipeline::{self, TaskEnv};
    use sbxm::task::record::ProcessProbe;
    use sbxm::task::resume;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// No recorded process is alive: the held task's command has ended.
    struct Gone;

    impl ProcessProbe for Gone {
        fn start_time(&self, _pid: u32) -> Option<u64> {
            None
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Merge {
        /// The agent merges the bundled base, keeping both sides of a conflict.
        Merges,
        /// The agent hands the merge back without making it.
        HandsBack,
    }

    fn env<'a>(
        f: &'a Fixture,
        backend: &'a FakeBackend,
        hold: Option<&'a dyn Check>,
    ) -> TaskEnv<'a> {
        let host: &'static FakeHostRunner = Box::leak(Box::default());
        TaskEnv {
            progress: &pipeline::quiet,
            config_dir: Box::leak(Box::new(f.env.config_dir())),
            config: &f.config,
            backend,
            probe: &Gone,
            host,
            sleep: &std::thread::sleep,
            scoped_secrets: &[],
            disk: Box::leak(Box::default()),
            identity: Some(&f.identity),
            clone_source: None,
            hold,
        }
    }

    /// The worker of issue 41: its first run commits `a.txt`; a merge round merges the base from
    /// `.sbxm-task/base.bundle` as the prompt says (or not, with `HandsBack`). Every review is
    /// clean. Counts the merge rounds.
    fn backend(f: &Fixture, merge: Merge, merges: Arc<AtomicUsize>) -> FakeBackend {
        let workspace = f.env.base_dir().join("tasks").join("issue-41");
        let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
        FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_exec_output_matching("claude", ok(CLAUDE_DONE))
            .with_exec_output_matching("codex", ok(CODEX_DONE))
            .with_exec_hook(move |sandbox, spec| {
                reviewer(sandbox, spec);
                if sandbox.contains("-review-") {
                    return;
                }
                if is_headless(spec) {
                    let prompt = workspace.join(".sbxm-task").join("fix-prompt.md");
                    let merging =
                        fs::read_to_string(&prompt).is_ok_and(|p| p.contains("Merge the base"));
                    if !merging {
                        fs::write(workspace.join("a.txt"), "the task's line\n").unwrap();
                        git(&workspace, &["add", "-A"]);
                        git(&workspace, &["commit", "-q", "-m", "Add a"]);
                        return;
                    }
                    merges.fetch_add(1, Ordering::SeqCst);
                    if merge == Merge::HandsBack {
                        return;
                    }
                    git(
                        &workspace,
                        &[
                            "fetch",
                            "-q",
                            ".sbxm-task/base.bundle",
                            "+refs/heads/main:refs/remotes/origin/main",
                        ],
                    );
                    let merged = Command::new("git")
                        .args(["merge", "--no-edit", "-q", "origin/main"])
                        .current_dir(&workspace)
                        .env("GIT_CONFIG_GLOBAL", "/dev/null")
                        .env("GIT_AUTHOR_NAME", "t")
                        .env("GIT_AUTHOR_EMAIL", "t@t")
                        .env("GIT_COMMITTER_NAME", "t")
                        .env("GIT_COMMITTER_EMAIL", "t@t")
                        .output()
                        .unwrap();
                    if !merged.status.success() {
                        // Both sides kept, one after the other.
                        let text = fs::read_to_string(workspace.join("a.txt")).unwrap();
                        let resolved: String = text
                            .lines()
                            .filter(|l| {
                                !l.starts_with("<<<<<<<")
                                    && !l.starts_with("=======")
                                    && !l.starts_with(">>>>>>>")
                            })
                            .map(|l| format!("{l}\n"))
                            .collect();
                        fs::write(workspace.join("a.txt"), resolved).unwrap();
                        git(&workspace, &["add", "a.txt"]);
                        git(&workspace, &["commit", "-q", "--no-edit"]);
                    }
                } else if is_bundle(spec) {
                    git(
                        &workspace,
                        &[
                            "bundle",
                            "create",
                            ".sbxm-task/branch.bundle",
                            "issue-41",
                            "^origin/main",
                        ],
                    );
                }
            })
    }

    /// `main` on the stand-in origin gets a commit writing `file`; returns its tip.
    fn move_main(f: &Fixture, file: &str, text: &str) -> String {
        let dir = f
            .env
            .tmp
            .path()
            .join(format!("main-{}", file.replace('.', "-")));
        git(
            f.env.tmp.path(),
            &[
                "clone",
                "-q",
                f.origin.to_str().unwrap(),
                dir.to_str().unwrap(),
            ],
        );
        fs::write(dir.join(file), text).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "main moves"]);
        git(&dir, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        git(&dir, &["rev-parse", "HEAD"])
    }

    /// Issue 41 worked, then held before its review on PR #30.
    fn held(f: &Fixture, backend: &FakeBackend) -> pipeline::Prepared {
        let mut prepared = worked_task(f, backend);
        let check = FakeCheck::new(vec![Wait::Pr {
            number: 30,
            armed: false,
        }]);
        let err =
            pipeline::review_issue(&env(f, backend, Some(&check)), &mut prepared).unwrap_err();
        assert!(err.is::<pipeline::OnHold>(), "{err:#}");
        assert_eq!(saved(f).stage, Stage::Held);
        prepared
    }

    fn branch_has(f: &Fixture, commit: &str) -> bool {
        let repo_git = record::task_dir(&f.env.base_dir(), "issue-41").join("repo.git");
        Command::new("git")
            .args(["merge-base", "--is-ancestor", commit, "refs/heads/issue-41"])
            .current_dir(&repo_git)
            .status()
            .unwrap()
            .success()
    }

    fn gate_runs(backend: &FakeBackend) -> usize {
        backend
            .execs()
            .iter()
            .filter(|(sandbox, spec)| {
                !sandbox.contains("-review-") && spec.argv.iter().any(|a| a.contains("cargo test"))
            })
            .count()
    }

    #[test]
    fn task_loop_hold_released_after_the_pr_merged_merges_the_base_runs_the_gates_then_reviews() {
        let f = config();
        let merges = Arc::new(AtomicUsize::new(0));
        let backend = backend(&f, Merge::Merges, merges.clone());
        let mut prepared = held(&f, &backend);
        let gates_before = gate_runs(&backend);
        let main = move_main(&f, "b.txt", "the merged PR's file\n");
        resume::resume(&env(&f, &backend, None), &mut prepared, None).unwrap();
        let rec = saved(&f);
        assert_eq!((rec.stage, rec.status), (Stage::Ready, Status::Ok));
        assert_eq!(merges.load(Ordering::SeqCst), 1, "one merge round");
        assert!(branch_has(&f, &main), "the branch has main's new tip");
        assert_eq!(gate_runs(&backend), gates_before + 1, "the gates ran again");
        assert_eq!(reviews_run(&backend), 1, "one review");
        assert!(rec.holds[0].released.is_some());
        assert_eq!(rec.holds[0].conflict, Some(false));
        assert!(rec.fix_runs.is_empty(), "a merge is no fix round");
    }

    #[test]
    fn task_loop_hold_released_after_the_pr_closed_unmerged_reviews_with_nothing_to_merge() {
        let f = config();
        let merges = Arc::new(AtomicUsize::new(0));
        let backend = backend(&f, Merge::Merges, merges.clone());
        let mut prepared = held(&f, &backend);
        let gates_before = gate_runs(&backend);
        resume::resume(&env(&f, &backend, None), &mut prepared, None).unwrap();
        let rec = saved(&f);
        assert_eq!(rec.stage, Stage::Ready);
        assert_eq!(merges.load(Ordering::SeqCst), 0, "nothing to merge");
        assert_eq!(gate_runs(&backend), gates_before + 1);
        assert_eq!(reviews_run(&backend), 1);
        assert_eq!(rec.holds[0].conflict, Some(false));
    }

    #[test]
    fn task_loop_hold_a_conflicting_merge_is_resolved_by_the_agent_then_gated_and_reviewed() {
        let f = config();
        let merges = Arc::new(AtomicUsize::new(0));
        let backend = backend(&f, Merge::Merges, merges.clone());
        let mut prepared = held(&f, &backend);
        let main = move_main(&f, "a.txt", "main's own line\n");
        resume::resume(&env(&f, &backend, None), &mut prepared, None).unwrap();
        let rec = saved(&f);
        assert_eq!(rec.stage, Stage::Ready);
        assert_eq!(merges.load(Ordering::SeqCst), 1);
        assert!(branch_has(&f, &main));
        assert_eq!(rec.holds[0].conflict, Some(true));
        assert_eq!(reviews_run(&backend), 1);
    }

    #[test]
    fn task_loop_hold_a_merge_the_agent_hands_back_stops_the_task_for_the_user() {
        let f = config();
        let merges = Arc::new(AtomicUsize::new(0));
        let backend = backend(&f, Merge::HandsBack, merges.clone());
        let mut prepared = held(&f, &backend);
        move_main(&f, "a.txt", "main's own line\n");
        let err = resume::resume(&env(&f, &backend, None), &mut prepared, None).unwrap_err();
        assert!(
            format!("{err:#}").contains("wasn't made, so issue-41 stops here for you"),
            "{err:#}"
        );
        let rec = saved(&f);
        assert_eq!((rec.stage, rec.status), (Stage::Held, Status::Failed));
        assert_eq!(rec.holds[0].conflict, Some(true));
        assert_eq!(reviews_run(&backend), 0, "no review without the merge");
    }
}

// The loop: a held task ends its phase without a row, is checked again at each pass, and is
// released (from its record alone, after a restart too) once nothing overlaps.

mod looping {
    use super::*;
    use anyhow::{Result, anyhow};
    use common::Env as TestEnv;
    use common::task_fixture::{issue_text, ready_open_issue};
    use sbxm::commands::task_loop::{self, Env, Options, Steps};
    use sbxm::task::pipeline::OnHold;
    use std::cell::{Cell, RefCell};
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Duration;

    const READY: &str = "ready-for-worker";

    /// Plays `task run` (the task is held on PR #30), `task resume` (ready) and `task finish`.
    struct Script<'a> {
        base: PathBuf,
        gh: &'a FakeGitHub,
        calls: RefCell<Vec<String>>,
        clock: Cell<u64>,
        /// Run after each `task run`: GitHub changing meanwhile.
        after_run: Box<dyn Fn(&FakeGitHub) + 'a>,
    }

    impl Steps for Script<'_> {
        fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            self.calls.borrow_mut().push(format!("run {issue}"));
            self.clock.set(self.clock.get() + 300);
            held_task(&self.base, issue, &["src/a.rs"], START, 30);
            (self.after_run)(self.gh);
            Err(OnHold::new(format!("issue-{issue} is held before its review")).into())
        }

        fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            self.calls.borrow_mut().push(format!("resume {issue}"));
            self.clock.set(self.clock.get() + 600);
            let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
            let mut rec = record::read(&dir.join("task.json")).unwrap();
            if hold::waiting(&rec).is_some() {
                return Err(anyhow!("resumed while still waiting"));
            }
            rec.stage = Stage::Ready;
            rec.status = Status::Ok;
            record::write(&dir, &rec).unwrap();
            Ok(())
        }

        fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            self.calls.borrow_mut().push(format!("finish {issue}"));
            let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
            let mut rec = record::read(&dir.join("task.json")).unwrap();
            rec.stage = Stage::Finished;
            rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
            record::write(&dir, &rec).unwrap();
            Ok(())
        }

        fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
            Ok(())
        }

        fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
            Err(anyhow!("unexpected fix of PR #{pr}"))
        }

        fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
            Err(anyhow!("unexpected log-fix of PR #{pr}"))
        }

        fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
            Ok(())
        }

        fn room(&self) -> Result<()> {
            Ok(())
        }
    }

    /// Issue task `n`'s record at `held`, waiting on PR `pr` since `since`, its branch changing
    /// `files`; no process (its command ended).
    fn held_task(base: &Path, n: u32, files: &[&str], since: u64, pr: u32) -> Record {
        let mut rec = task(base, n, files, Stage::Held, Status::Ok, since);
        rec.process = None;
        hold::start(
            &mut rec,
            since,
            vec![Wait::Pr {
                number: pr,
                armed: false,
            }],
        );
        record::write(&record::task_dir(base, &rec.id), &rec).unwrap();
        rec
    }

    struct Fixture {
        env: TestEnv,
        repo_root: PathBuf,
    }

    fn fixture() -> Fixture {
        let env = TestEnv::new();
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        fs::write(
            repo_root.join("sbxm-task.toml"),
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
        )
        .unwrap();
        Fixture { env, repo_root }
    }

    impl Fixture {
        fn base(&self) -> PathBuf {
            self.env.base_dir()
        }

        fn opts(&self) -> Options {
            Options {
                repo_root: self.repo_root.clone(),
                issues: Vec::new(),
                cap: None,
                repo: Some(REPO.into()),
                wait: false,
            }
        }

        fn script<'a>(&self, gh: &'a FakeGitHub) -> Script<'a> {
            Script {
                base: self.base(),
                gh,
                calls: RefCell::default(),
                clock: Cell::new(START + 3600),
                after_run: Box::new(|_| {}),
            }
        }

        fn saved(&self, n: u32) -> Record {
            record::read(&record::task_dir(&self.base(), &format!("issue-{n}")).join("task.json"))
                .unwrap()
        }
    }

    /// GitHub with open issues `issues` and PR #30 (issue 3's, a loop's) in `state`, changing
    /// `src/a.rs`.
    fn github(f: &Fixture, issues: &[u32], state: PrState) -> FakeGitHub {
        loop_pr(&f.base(), 3, 30);
        let mut gh = FakeGitHub::default()
            .with_user("me")
            .with_default_branch("main")
            .with_pr(pr(30, state))
            .with_pr_files(30, files(&["src/a.rs"]));
        let mut open = Vec::new();
        for &n in issues {
            gh = gh
                .with_issue_text(issue_text(n))
                .with_pr(pr(100 + n, PrState::Open));
            open.push(ready_open_issue(n, &[READY, "size:S"], ""));
        }
        gh.with_open_issues(open)
    }

    struct Run {
        result: Result<()>,
        out: String,
    }

    fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub) -> Run {
        let sleeps = Cell::new(0usize);
        let now = || script.clock.get();
        let sleep = |d: Duration| {
            sleeps.set(sleeps.get() + 1);
            script.clock.set(script.clock.get() + d.as_secs());
        };
        let stop = || sleeps.get() > 3;
        let env = Env {
            now: &now,
            sleep: &sleep,
            stop_requested: &stop,
        };
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let result = task_loop::run(
            &f.env.config_dir(),
            &f.opts(),
            script,
            gh,
            &Probe,
            &env,
            &mut out,
            &mut warn,
        );
        Run {
            result,
            out: String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap(),
        }
    }

    #[test]
    fn task_loop_hold_a_held_task_ends_its_phase_and_the_loop_moves_on() {
        let f = fixture();
        let gh = github(&f, &[9], PrState::Open);
        let script = f.script(&gh);
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        assert_eq!(*script.calls.borrow(), vec!["run 9"], "{}", run.out);
        assert!(
            run.out
                .contains("held before its review: waits on #30 (PR, awaits your merge)"),
            "{}",
            run.out
        );
        assert!(
            run.out.contains("is held before its review (waits on #30"),
            "the stop names it: {}",
            run.out
        );
        assert_eq!(f.saved(9).stage, Stage::Held);
    }

    #[test]
    fn task_loop_hold_the_pr_it_waits_on_merges_releases_it_to_its_review_and_finish() {
        let f = fixture();
        let gh = github(&f, &[9], PrState::Open);
        let mut script = f.script(&gh);
        script.after_run =
            Box::new(|gh| gh.set_merged(30, "cccccccccccccccccccccccccccccccccccccccc"));
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        assert_eq!(
            *script.calls.borrow(),
            vec!["run 9", "resume 9", "finish 9"],
            "{}",
            run.out
        );
        assert!(run.out.contains("released from its hold"), "{}", run.out);
        let rec = f.saved(9);
        assert!(rec.holds[0].released.is_some());
    }

    #[test]
    fn task_loop_hold_the_pr_closed_unmerged_releases_it_too() {
        let f = fixture();
        held_task(&f.base(), 9, &["src/a.rs"], START, 30);
        let gh = github(&f, &[], PrState::Closed);
        let script = f.script(&gh);
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        assert_eq!(*script.calls.borrow(), vec!["resume 9", "finish 9"]);
    }

    #[test]
    fn task_loop_hold_a_restarted_loop_checks_the_held_task_again_and_releases_it() {
        let f = fixture();
        // Held by an earlier loop; the PR merged since.
        held_task(&f.base(), 9, &["src/a.rs"], START, 30);
        let gh = github(&f, &[], PrState::Merged);
        let script = f.script(&gh);
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        assert_eq!(
            *script.calls.borrow(),
            vec!["resume 9", "finish 9"],
            "{}",
            run.out
        );
        // It says the minutes it was held, an hour, and so does its row in the loop's log.
        assert!(
            run.out.contains("released from its hold after 60 min"),
            "{}",
            run.out
        );
        let logs = f.base().join(".sbxm").join("loop-logs");
        let log: String = fs::read_dir(&logs)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect();
        let row = log.lines().find(|l| l.contains("#9")).unwrap_or_default();
        assert!(
            row.contains("held before review, waits on #30 (PR, awaits your merge)"),
            "{log}"
        );
    }

    #[test]
    fn task_loop_hold_a_restarted_loop_leaves_a_task_still_overlapping_held() {
        let f = fixture();
        held_task(&f.base(), 9, &["src/a.rs"], START, 30);
        let gh = github(&f, &[], PrState::Open);
        let script = f.script(&gh);
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        assert!(
            script.calls.borrow().is_empty(),
            "{:?}",
            script.calls.borrow()
        );
        assert_eq!(
            hold::waiting(&f.saved(9)).map(|h| h.released.clone()),
            Some(None)
        );
    }

    #[test]
    fn task_loop_hold_of_two_released_by_one_merge_the_first_held_goes_first() {
        let f = fixture();
        // Both wait on PR #30 and share src/a.rs; issue 9 was held first.
        held_task(&f.base(), 9, &["src/a.rs"], START, 30);
        held_task(&f.base(), 8, &["src/a.rs"], START + 60, 30);
        let gh = github(&f, &[], PrState::Merged).with_pr(pr(109, PrState::Open));
        let gh = gh.with_pr_files(109, files(&["src/a.rs"]));
        let script = f.script(&gh);
        let run = run_loop(&f, &script, &gh);
        assert!(run.result.is_ok(), "{:?}\n{}", run.result, run.out);
        // Issue 9 goes on to its PR (#109); issue 8 checks again against it and stays held.
        assert_eq!(
            *script.calls.borrow(),
            vec!["resume 9", "finish 9"],
            "{}",
            run.out
        );
        let eight = f.saved(8);
        assert!(hold::waiting(&eight).is_some(), "{}", run.out);
    }
}

// What shows the holds: `sbxm status`, the loop log's row (above) and `task facts`.

mod shown {
    use super::*;
    use common::Env as TestEnv;
    use sbxm::commands::task_facts::{self, Options};
    use sbxm::run::results::parse_timestamp;
    use serde_json::Value;

    #[test]
    fn task_loop_hold_status_shows_what_a_held_task_waits_on() {
        let env = TestEnv::new();
        let base = env.base_dir();
        let mut rec = task(&base, 9, &["src/a.rs"], Stage::Held, Status::Ok, START);
        rec.process = None;
        hold::start(
            &mut rec,
            START,
            vec![
                Wait::Pr {
                    number: 12,
                    armed: false,
                },
                Wait::Task {
                    task: "issue-8".into(),
                    stage: Stage::Reviewing,
                },
            ],
        );
        record::write(&record::task_dir(&base, &rec.id), &rec).unwrap();
        let rows = sbxm::status::tasks::rows(&base, &Probe).unwrap();
        let text = sbxm::status::tasks::render(&rows, START + 60, None);
        assert!(
            text.contains(
                "issue-9 held: waits on #12 (PR, awaits your merge), issue-8 (in review)"
            ),
            "{text}"
        );
    }

    #[test]
    fn task_loop_hold_task_facts_reports_hold_minutes_and_hold_conflicts() {
        let env = TestEnv::new();
        let base = env.base_dir();
        let start = parse_timestamp("2026-11-10T08:00:00Z").unwrap();
        let mut rec = task(&base, 9, &["src/a.rs"], Stage::Held, Status::Ok, start);
        rec.stages[0].at = format_timestamp(start);
        hold::start(&mut rec, start + 600, vec![]);
        hold::release(&mut rec, start + 600 + 45 * 60);
        rec.holds[0].conflict = Some(true);
        rec.stage = Stage::Ready;
        record::write(&record::task_dir(&base, &rec.id), &rec).unwrap();
        let mut other = task(&base, 8, &["src/b.rs"], Stage::Ready, Status::Ok, start + 1);
        other.stages[0].at = format_timestamp(start + 1);
        record::write(&record::task_dir(&base, &other.id), &other).unwrap();

        let run = |json: bool| {
            let (mut out, mut warn) = (Vec::new(), Vec::new());
            task_facts::run(
                &env.config_dir(),
                &Options {
                    since: "2026-11-10".into(),
                    until: None,
                    json,
                },
                parse_timestamp("2026-11-15T12:00:00Z").unwrap(),
                &mut out,
                &mut warn,
            )
            .unwrap();
            String::from_utf8(out).unwrap()
        };
        let rows: Vec<Value> = serde_json::from_str(&run(true)).unwrap();
        let of = |id: &str| rows.iter().find(|r| r["task"] == id).unwrap().clone();
        assert_eq!(of("issue-9")["held_min"], 45);
        assert_eq!(of("issue-9")["hold_conflict"], true);
        assert_eq!(of("issue-8")["held_min"], Value::Null);
        assert_eq!(of("issue-8")["hold_conflict"], Value::Null);

        let table = run(false);
        assert!(
            table
                .lines()
                .next()
                .unwrap()
                .ends_with("| PR | held (min) | hold conflict |"),
            "{table}"
        );
        let nine = table.lines().find(|l| l.starts_with("| issue-9 ")).unwrap();
        assert!(nine.ends_with("| - | 45 | yes |"), "{nine}");
        let eight = table.lines().find(|l| l.starts_with("| issue-8 ")).unwrap();
        assert!(eight.ends_with("| - | - | - |"), "{eight}");
    }
}
