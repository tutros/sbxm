//! Issue 615 (milestone 5 slice 4, decisions 317(4), (6), 321): the loop keeps up to
//! `sandbox_slots` local and `cloud_slots` cloud children running at once, each an issue task end
//! to end; its log row names the slot a task ran in.

use std::fs;

use sbxm::backend::Location;
use sbxm::task::queue::{Row, append_row};

// --- the log's Slot column ---

fn row(issue: u32, slot: Option<Location>) -> Row {
    Row {
        issue,
        title: "Fix it".into(),
        size: Some("S".into()),
        slot,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds: None,
        must_fix: None,
        outcome: "parked at working".into(),
        stops: Vec::new(),
        idle_before: None,
    }
}

#[test]
fn a_rows_slot_column_names_where_its_task_ran() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");

    append_row(&path, "o/r", &row(1, Some(Location::Local))).unwrap();
    append_row(&path, "o/r", &row(2, Some(Location::Cloud))).unwrap();
    append_row(&path, "o/r", &row(3, None)).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("| # | Issue | Size | Slot | Started | Ready (time to ready) |"),
        "{text}"
    );
    assert!(
        text.contains("| 1 | #1 Fix it | S | local | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 2 | #2 Fix it | S | cloud | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 3 | #3 Fix it | S | - | 14:59:18 | - |"),
        "{text}"
    );
}

// --- `sbxm status` lists every running child ---

mod status_children {
    use std::fs;
    use std::path::Path;

    use sbxm::backend::{FakeBackend, Location};
    use sbxm::commands::status::{self, Env, LoopSource};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::task::queue::{LockFile, lock_path};
    use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

    const T0: u64 = 1_790_000_000;
    const LOOP_START: u64 = T0 - 1_000;

    struct Alive;

    impl ProcessProbe for Alive {
        fn start_time(&self, pid: u32) -> Option<u64> {
            Some(if pid == 1 { LOOP_START } else { T0 })
        }
    }

    /// Issue `number`'s task at `stage` in `slot`, run by a child (pid 10 + number) of the loop.
    fn child_task(base: &Path, number: u32, stage: Stage, slot: Location) {
        let mut rec = Record::new(
            &NewTask {
                kind: Kind::Issue,
                number,
                repo: "o/r",
                title: &format!("title {number}"),
                base: "main",
                branch: "b",
                config_hash: "h",
                id: None,
            },
            T0,
            Process::new(10 + number, T0),
        );
        rec.stage = stage;
        rec.status = Status::Running;
        rec.location = slot;
        rec.process = Some(Process::new(10 + number, T0));
        rec.loop_owner = Some(Process::new(1, LOOP_START));
        record::write(&record::task_dir(base, &rec.id), &rec).unwrap();
    }

    fn loop_section(base: &Path) -> String {
        listed_section(base, &[])
    }

    /// The Loop section for a loop started with `--issue` over `issues` (none: the labeled
    /// queue), its section up to the jobs line.
    fn listed_section(base: &Path, issues: &[u32]) -> String {
        let path = lock_path(base);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = LockFile {
            process: Process::new(1, LOOP_START),
            issues: issues.to_vec(),
            cap: None,
            progress: None,
        };
        fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
        let gh = FakeGitHub::default();
        let source = LoopSource {
            github: &gh,
            repo: Ok("o/r".into()),
        };
        let env = Env {
            now: Box::new(|| T0),
            sleep: Box::new(|_| {}),
            terminal: false,
            width: 120,
            iterations: None,
            monotonic: Box::new(|| std::time::Duration::ZERO),
            slots: None,
            cloud_slots: None,
            disk: sbxm::status::footer::Disk {
                free_space: |_| Ok(40 * 1024 * 1024 * 1024),
                same_drive: |_, _| true,
                state_dir: Some(std::path::PathBuf::from("/state")),
                min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
            },
        };
        let mut out = Vec::new();
        status::run_with(
            base,
            &FakeBackend::default(),
            &Alive,
            &env,
            None,
            Some(&source),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let section = text.split("Loop:").nth(1).expect("a Loop section");
        section.split("\n  done:").next().unwrap().to_owned()
    }

    #[test]
    fn status_lists_every_running_child_with_its_slot_and_stage() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 3, Stage::Working, Location::Local);
        child_task(&base, 4, Stage::Reviewing, Location::Cloud);
        child_task(&base, 5, Stage::Working, Location::Cloud);

        let section = loop_section(&base);

        assert!(
            section.ends_with(
                "\n  task: issue-3 (title 3) at working, local\
                 \n  task: issue-4 (title 4) at reviewing, cloud\
                 \n  task: issue-5 (title 5) at working, cloud"
            ),
            "{section}"
        );
    }

    #[test]
    fn status_names_a_single_childs_slot_too() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 4, Stage::Working, Location::Cloud);

        let section = loop_section(&base);

        assert!(
            section.ends_with("\n  task: issue-4 (title 4) at working, cloud"),
            "{section}"
        );
    }

    #[test]
    fn a_listed_loop_counts_its_other_children_as_running_not_pending() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 3, Stage::Working, Location::Local);
        child_task(&base, 4, Stage::Working, Location::Cloud);

        let text = listed_section(&base, &[3, 4, 5]);

        assert!(
            text.contains(
                "\n  task: issue-3 (title 3) at working, local, task 1 of 3\
                 \n  task: issue-4 (title 4) at working, cloud"
            ),
            "{text}"
        );
    }

    fn common_env() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("base")).unwrap();
        tmp
    }
}

// --- a child started without waiting for it ---

#[cfg(unix)]
mod started_child {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use sbxm::commands::task_loop::child::{Child, ChildSpawner, Spawner};

    fn sh(script: &str, dir: &std::path::Path) -> Child {
        Child {
            args: vec!["-c".into(), script.into()],
            dir: dir.to_owned(),
            env: Vec::new(),
        }
    }

    #[test]
    fn a_started_child_is_polled_until_it_exits() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));

        let mut running = spawner
            .start(&sh("sleep 1; echo 'Error: late' >&2; exit 2", tmp.path()))
            .unwrap();

        assert_eq!(running.poll().unwrap(), None, "still running");
        let started = Instant::now();
        let exit = loop {
            if let Some(exit) = running.poll().unwrap() {
                break exit;
            }
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(!exit.success);
        assert_eq!(exit.how, "exited with code 2");
        assert!(exit.errors.contains("Error: late"), "{exit:?}");
    }

    #[test]
    fn two_children_run_at_once_and_a_stop_ends_both() {
        let tmp = tempfile::tempdir().unwrap();
        let spawner = ChildSpawner::new(PathBuf::from("/bin/sh"));
        let mut first = spawner.start(&sh("sleep 30", tmp.path())).unwrap();
        let mut second = spawner.start(&sh("sleep 30", tmp.path())).unwrap();
        assert_eq!(first.poll().unwrap(), None);
        assert_eq!(second.poll().unwrap(), None);
        let started = Instant::now();

        spawner.stop();

        let first = first.poll().unwrap().expect("stopped");
        let second = second.poll().unwrap().expect("stopped");
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(!first.success && !second.success);
        assert_eq!(first.how, "was stopped");
        assert_eq!(second.how, "was stopped");
    }
}

// --- the loop with several children ---

mod common;

mod several {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Result, anyhow};
    use sbxm::backend::Location;
    use sbxm::commands::task_loop::{self, Env, Options, Phase, PhaseEnd, Started, Steps};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::github::{Mergeable, PrInfo, PrState};
    use sbxm::task::outcome::{Kind, Outcome};
    use sbxm::task::record::{self, NewTask, Process, Record, ReviewResult, Stage, Status};

    use super::common;
    use super::common::task_fixture::{Probe, issue_text, open_issue};

    pub const REPO: &str = "o/r";
    const READY: &str = "ready-for-worker";
    /// 2026-11-02T10:00:00Z.
    pub const START: u64 = 1_793_613_600;
    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    /// How a played phase ends.
    #[derive(Clone, Copy, Debug)]
    pub enum Act {
        /// Reaches `ready`, clean.
        Ready,
        /// Fails, leaving the record at `working`.
        Fail,
        /// Stops on a usage limit, leaving the record at `working`.
        UsageLimit,
        /// Killed after its record was written: no outcome.
        Crash,
    }

    /// Plays each issue's phases as children that end after a number of polls, writing the
    /// record a real child would; `events` says what started and ended, in order.
    pub struct Play<'a> {
        base: PathBuf,
        /// Each issue's phases, in order: polls before it ends, and how.
        script: RefCell<HashMap<u32, VecDeque<(u32, Act)>>>,
        pub events: RefCell<Vec<String>>,
        running: Cell<usize>,
        pub most: Cell<usize>,
        /// Run as each phase starts: GitHub changing meanwhile.
        on_start: Box<dyn Fn(u32) + 'a>,
    }

    impl<'a> Play<'a> {
        pub fn new(base: &Path) -> Self {
            Self {
                base: base.to_owned(),
                script: RefCell::default(),
                events: RefCell::default(),
                running: Cell::new(0),
                most: Cell::new(0),
                on_start: Box::new(|_| {}),
            }
        }

        pub fn plays(self, issue: u32, phases: &[(u32, Act)]) -> Self {
            self.script
                .borrow_mut()
                .insert(issue, phases.iter().copied().collect());
            self
        }

        pub fn on_start(mut self, f: impl Fn(u32) + 'a) -> Self {
            self.on_start = Box::new(f);
            self
        }

        pub fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }

        /// The index of `event` among the events; panics when it is not there.
        pub fn at(&self, event: &str) -> usize {
            self.events()
                .iter()
                .position(|e| e == event)
                .unwrap_or_else(|| panic!("no {event:?} in {:?}", self.events()))
        }

        fn next(&self, issue: u32) -> (u32, Act) {
            self.script
                .borrow_mut()
                .get_mut(&issue)
                .and_then(VecDeque::pop_front)
                .unwrap_or((0, Act::Ready))
        }
    }

    /// Issue `issue`'s record as a child leaves it after `act`, in `location`.
    pub fn write_record(base: &Path, issue: u32, act: Act, location: Location) {
        let dir = record::task_dir(base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json")).unwrap_or_else(|_| {
            let mut rec = Record::new(
                &NewTask {
                    kind: record::Kind::Issue,
                    number: issue,
                    repo: REPO,
                    title: &format!("Fix {issue}"),
                    base: "main",
                    branch: &format!("sbxm/issue-{issue}"),
                    config_hash: "h",
                    id: None,
                },
                START,
                Process::new(1, 0),
            );
            rec.location = location;
            rec
        });
        match act {
            Act::Ready => {
                rec.stage = Stage::Ready;
                rec.status = Status::Ok;
                rec.review = Some(ReviewResult {
                    round: 1,
                    must_fix: 0,
                    not_run: Some(Vec::new()),
                    ..ReviewResult::default()
                });
            }
            Act::Fail | Act::UsageLimit | Act::Crash => {
                rec.stage = Stage::Working;
                rec.status = Status::Failed;
            }
        }
        record::write(&dir, &rec).unwrap();
    }

    struct Playing<'p, 'a> {
        play: &'p Play<'a>,
        issue: u32,
        polls: u32,
        act: Act,
        location: Location,
    }

    impl Started for Playing<'_, '_> {
        fn poll(&mut self, _out: &mut dyn Write) -> Option<PhaseEnd> {
            if self.polls > 0 {
                self.polls -= 1;
                return None;
            }
            write_record(&self.play.base, self.issue, self.act, self.location);
            self.play.running.set(self.play.running.get() - 1);
            self.play
                .events
                .borrow_mut()
                .push(format!("end {} {:?}", self.issue, self.act));
            Some(match self.act {
                Act::Ready => PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, "")),
                Act::Fail => PhaseEnd::Outcome(Outcome::new(Kind::Failed, None, "gates failed")),
                Act::UsageLimit => PhaseEnd::Outcome(Outcome::new(
                    Kind::UsageLimit,
                    Some("claude".into()),
                    "usage limit reached",
                )),
                Act::Crash => PhaseEnd::Missing("the task's child sbxm was killed".into()),
            })
        }
    }

    impl Steps for Play<'_> {
        fn start_task(
            &self,
            issue: u32,
            phase: Phase,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Box<dyn Started + '_> {
            let location = match phase {
                Phase::Run(location) => location.unwrap_or_default(),
                Phase::Resume | Phase::ShouldFix => {
                    let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
                    record::read(&dir.join("task.json")).unwrap().location
                }
            };
            let what = match phase {
                Phase::Run(_) => "run",
                Phase::Resume => "resume",
                Phase::ShouldFix => "should-fix",
            };
            self.events
                .borrow_mut()
                .push(format!("start {issue} {what} {}", location.as_str()));
            self.running.set(self.running.get() + 1);
            self.most.set(self.most.get().max(self.running.get()));
            (self.on_start)(issue);
            let (polls, act) = self.next(issue);
            Box::new(Playing {
                play: self,
                issue,
                polls,
                act,
                location,
            })
        }

        // The loop with one task at a time runs each phase to its end here.
        fn run_at(
            &self,
            issue: u32,
            location: Option<Location>,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Result<()> {
            let at = location.unwrap_or_default();
            self.events
                .borrow_mut()
                .push(format!("start {issue} run {}", at.as_str()));
            let (_, act) = self.next(issue);
            write_record(&self.base, issue, act, at);
            self.events
                .borrow_mut()
                .push(format!("end {issue} {act:?}"));
            match act {
                Act::Ready => Ok(()),
                _ => Err(anyhow!("played failure")),
            }
        }

        fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
            self.run_at(issue, None, out, warn)
        }

        fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
            self.run_at(issue, None, out, warn)
        }

        fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            self.events.borrow_mut().push(format!("finish {issue}"));
            let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
            let mut rec = record::read(&dir.join("task.json"))?;
            rec.stage = Stage::Finished;
            rec.status = Status::Ok;
            rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
            record::write(&dir, &rec)
        }

        fn file_findings(
            &self,
            _issue: u32,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Result<()> {
            Ok(())
        }

        fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            self.events.borrow_mut().push(format!("resolve {pr}"));
            Ok(())
        }

        fn fix_check(
            &self,
            pr: u32,
            _run: u64,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Result<()> {
            Err(anyhow!("unexpected log-fix of PR #{pr}"))
        }

        fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            Ok(())
        }

        fn room(&self) -> Result<()> {
            Ok(())
        }
    }

    pub struct Fixture {
        pub env: common::Env,
        pub repo_root: PathBuf,
    }

    impl Fixture {
        pub fn base(&self) -> PathBuf {
            self.env.base_dir()
        }

        pub fn log_text(&self) -> String {
            fs::read_to_string(
                self.base()
                    .join(".sbxm/loop-logs/2026-11-02-100000-loop.md"),
            )
            .unwrap_or_default()
        }
    }

    /// `sandbox_slots` unset (one local), and `cloud_slots` as given.
    pub fn fixture(cloud_slots: Option<u32>) -> Fixture {
        let env = common::Env::new();
        if let Some(n) = cloud_slots {
            common::cloud_slot::set_cloud_slots(&env, n);
        }
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        fs::write(
            repo_root.join("sbxm-task.toml"),
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\nusage_wait = \
             \"10m\"\nusage_wait_total = \"1h\"\n",
        )
        .unwrap();
        Fixture { env, repo_root }
    }

    /// GitHub with these issues open and labeled ready, each sized S, with `extra` labels.
    pub fn github(issues: &[(u32, &[&str])]) -> FakeGitHub {
        let mut gh = FakeGitHub::default()
            .with_user("me")
            .with_default_branch("main")
            .with_branch_head("main", HEAD);
        let mut open = Vec::new();
        for (n, extra) in issues {
            gh = gh.with_issue_text(issue_text(*n));
            let mut labels = vec![READY, "size:S"];
            labels.extend_from_slice(extra);
            open.push(open_issue(*n, &labels, "Why\n"));
        }
        gh.with_open_issues(open)
    }

    pub fn pr(number: u32) -> PrInfo {
        PrInfo {
            number,
            head_ref: format!("sbxm/pr-{number}"),
            head_oid: HEAD.into(),
            is_cross_repository: false,
            head_repo: String::new(),
            state: PrState::Open,
            title: format!("PR {number}"),
            body: String::new(),
            closing_issues: Vec::new(),
            base_moved: false,
        }
    }

    /// A finished task of issue `issue` the loop opened PR `pr` for, in `location`.
    pub fn loop_pr(base: &Path, issue: u32, pr: u32, location: Location) {
        write_record(base, issue, Act::Ready, location);
        let dir = record::task_dir(base, &format!("issue-{issue}"));
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.stage = Stage::Finished;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
        rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
        record::write(&dir, &rec).unwrap();
    }

    pub struct Run {
        pub result: Result<()>,
        pub out: String,
    }

    /// Runs the loop over the labeled queue to its stop, its clock moving with its sleeps;
    /// `stop` is asked whenever the loop looks for a stop request.
    pub fn run_loop(
        f: &Fixture,
        steps: &dyn Steps,
        gh: &FakeGitHub,
        stop: &dyn Fn() -> bool,
    ) -> Run {
        let clock = Cell::new(START);
        let env = Env {
            now: &|| clock.get(),
            sleep: &|d: Duration| clock.set(clock.get() + d.as_secs()),
            stop_requested: stop,
        };
        let mut out = Vec::new();
        let result = task_loop::run(
            &f.env.config_dir(),
            &Options {
                repo_root: f.repo_root.clone(),
                issues: Vec::new(),
                cap: None,
                repo: Some(REPO.into()),
                wait: false,
            },
            steps,
            gh,
            &Probe,
            &env,
            &mut out,
            &mut Vec::new(),
        );
        Run {
            result,
            out: String::from_utf8(out).unwrap(),
        }
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn one_local_and_two_cloud_children_run_at_once_and_a_fourth_waits_for_a_free_slot() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[]), (3, &[]), (4, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(10, Act::Ready)])
            .plays(2, &[(3, Act::Ready)])
            .plays(3, &[(10, Act::Ready)])
            .plays(4, &[(1, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        let starts: Vec<String> = play
            .events()
            .into_iter()
            .filter(|e| e.starts_with("start"))
            .collect();
        assert_eq!(
            starts,
            [
                "start 1 run local",
                "start 2 run cloud",
                "start 3 run cloud",
                "start 4 run cloud"
            ],
            "{}",
            run.out
        );
        assert_eq!(play.most.get(), 3, "{:?}", play.events());
        // The 4th starts in the slot the first to end freed, once it ended.
        assert!(play.at("end 2 Ready") < play.at("start 4 run cloud"));
        assert!(play.at("start 4 run cloud") < play.at("end 1 Ready"));
        for issue in 1..=4 {
            play.at(&format!("finish {issue}"));
        }
    }

    #[test]
    fn with_cloud_slots_unset_the_loop_runs_one_task_at_a_time_locally() {
        let f = fixture(None);
        let gh = github(&[(1, &[]), (2, &[]), (3, &[])]);
        let play = Play::new(&f.base());

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert_eq!(
            play.events(),
            [
                "start 1 run local",
                "end 1 Ready",
                "finish 1",
                "start 2 run local",
                "end 2 Ready",
                "finish 2",
                "start 3 run local",
                "end 3 Ready",
                "finish 3",
            ],
            "{}",
            run.out
        );
    }

    #[test]
    fn a_run_cloud_issue_that_does_not_fit_keeps_its_place_and_the_next_that_fits_starts_locally() {
        let f = fixture(Some(2));
        let gh = github(&[
            (1, &["run-cloud"]),
            (2, &["run-cloud"]),
            (3, &["run-cloud"]),
            (4, &[]),
        ]);
        let play = Play::new(&f.base())
            .plays(1, &[(3, Act::Ready)])
            .plays(2, &[(10, Act::Ready)])
            .plays(4, &[(10, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        let starts: Vec<String> = play
            .events()
            .into_iter()
            .filter(|e| e.starts_with("start"))
            .collect();
        assert_eq!(
            starts,
            [
                "start 1 run cloud",
                "start 2 run cloud",
                "start 4 run local",
                "start 3 run cloud"
            ],
            "{}",
            run.out
        );
        assert!(play.at("end 1 Ready") < play.at("start 3 run cloud"));
    }

    #[test]
    fn a_run_local_issue_whose_slot_is_full_waits_and_the_next_unpinned_starts_in_the_cloud() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &["run-local"]), (3, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(3, Act::Ready)])
            .plays(3, &[(10, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        let starts: Vec<String> = play
            .events()
            .into_iter()
            .filter(|e| e.starts_with("start"))
            .collect();
        assert_eq!(
            starts,
            [
                "start 1 run local",
                "start 3 run cloud",
                "start 2 run local"
            ],
            "{}",
            run.out
        );
        assert!(play.at("end 1 Ready") < play.at("start 2 run local"));
    }

    #[test]
    fn awaiting_prs_plus_running_children_reaching_five_hold_new_starts() {
        let f = fixture(Some(2));
        // Three PRs the loop opened await the user.
        let mut gh = github(&[(1, &[]), (2, &[]), (3, &[])]);
        for (issue, number) in [(50, 150), (51, 151), (52, 152)] {
            loop_pr(&f.base(), issue, number, Location::Local);
            gh = gh.with_pr(pr(number));
        }
        // The PRs of the loop's own tasks get auto-merge armed: they await no one.
        let gh = gh
            .with_pr(pr(101))
            .with_pr(pr(102))
            .with_pr(pr(103))
            .with_auto_merge(101)
            .with_auto_merge(102)
            .with_auto_merge(103);
        let play = Play::new(&f.base())
            .plays(1, &[(20, Act::Ready)])
            .plays(2, &[(3, Act::Ready)])
            .plays(3, &[(1, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        // 3 awaiting and 2 running: the third waits, though a cloud slot is free.
        assert!(
            run.out
                .contains("paused: 3 PRs await you and 2 tasks run, 5 in all"),
            "{}",
            run.out
        );
        assert!(play.at("finish 2") < play.at("start 3 run cloud"));
        assert!(play.at("start 3 run cloud") < play.at("end 1 Ready"));
    }

    #[test]
    fn a_resolve_whose_slot_a_child_holds_is_postponed_then_run_once_it_is_free() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Local);
        let gh = github(&[(1, &[])]).with_pr(pr(150)).with_pr(pr(101));
        // The PR starts conflicting once the child runs.
        let play = Play::new(&f.base())
            .plays(1, &[(400, Act::Ready)])
            .on_start(|_| gh.set_mergeable(150, Mergeable::Conflicting));

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert!(
            run.out
                .contains("its resolve waits for a free local slot; tried again at a later pass"),
            "{}",
            run.out
        );
        assert!(
            play.at("end 1 Ready") < play.at("resolve 150"),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_usage_limit_in_one_child_holds_new_starts_while_the_others_run_to_their_ends() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[]), (3, &[]), (4, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(1, Act::UsageLimit), (1, Act::Ready)])
            .plays(2, &[(3, Act::Ready)])
            .plays(3, &[(5, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert!(
            run.out
                .contains("no new task starts while #1 (Fix 1) waits out a usage limit"),
            "{}",
            run.out
        );
        // The others run on to their ends; the 4th starts only once #1 is past its limit.
        let fourth = play.at("start 4 run local");
        assert!(play.at("end 2 Ready") < fourth);
        assert!(play.at("end 3 Ready") < fourth);
        assert!(play.at("end 1 Ready") < fourth);
        assert!(
            f.log_text().contains("usage limit, 10m"),
            "{}",
            f.log_text()
        );
    }

    #[test]
    fn two_children_ending_in_one_pass_are_finished_one_after_the_other_with_their_rows() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(2, Act::Ready)])
            .plays(2, &[(2, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        let events = play.events();
        let ends: Vec<&str> = events[2..].iter().map(String::as_str).collect();
        assert_eq!(
            ends,
            ["end 1 Ready", "finish 1", "end 2 Ready", "finish 2"],
            "{}",
            run.out
        );
        let log = f.log_text();
        assert!(log.contains("| 1 | #1 Fix 1 | S | local |"), "{log}");
        assert!(log.contains("| 2 | #2 Fix 2 | S | cloud |"), "{log}");
    }

    #[test]
    fn a_crashed_child_is_resumed_once_in_the_slot_it_had() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(5, Act::Ready)])
            .plays(2, &[(1, Act::Crash), (1, Act::Ready)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        play.at("start 2 resume cloud");
        assert!(run.out.contains("resuming it once"), "{}", run.out);
        // Its resume ends first: its row comes first, in the slot it had.
        let log = f.log_text();
        assert!(log.contains("| 1 | #2 Fix 2 | S | cloud |"), "{log}");
        assert!(log.contains("| 2 | #1 Fix 1 | S | local |"), "{log}");
        assert_eq!(play.most.get(), 2, "{:?}", play.events());
    }

    #[test]
    fn a_child_failing_twice_is_parked_and_the_others_go_on() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(5, Act::Ready)])
            .plays(2, &[(1, Act::Fail), (1, Act::Fail)]);

        let run = run_loop(&f, &play, &gh, &never);

        run.result.unwrap();
        assert!(run.out.contains("parked at working"), "{}", run.out);
        play.at("finish 1");
    }

    #[test]
    fn a_first_stop_while_three_children_run_starts_nothing_new_and_the_loop_exits_when_they_end() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &[]), (2, &[]), (3, &[]), (4, &[])]);
        let play = Play::new(&f.base())
            .plays(1, &[(5, Act::Ready)])
            .plays(2, &[(3, Act::Ready)])
            .plays(3, &[(8, Act::Ready)]);
        // The stop comes once all three run.
        let stop = || play.most.get() == 3;

        let run = run_loop(&f, &play, &gh, &stop);

        run.result.unwrap();
        assert!(
            !play.events().iter().any(|e| e.starts_with("start 4")),
            "{:?}",
            play.events()
        );
        for issue in 1..=3 {
            play.at(&format!("finish {issue}"));
        }
        assert!(run.out.ends_with("stopped on request\n"), "{}", run.out);
    }

    // --- the real steps, their children played by a spawner whose children run until polled ---

    mod real_children {
        use std::cell::RefCell;
        use std::path::PathBuf;

        use anyhow::Result;
        use sbxm::backend::{FakeBackend, Location};
        use sbxm::commands::task_loop::TaskSteps;
        use sbxm::commands::task_loop::child::{Child, Exit, Running, Spawner};
        use sbxm::task::gates::FakeHostRunner;
        use sbxm::task::outcome::{self, Kind, Outcome};
        use sbxm::task::record;

        use super::*;

        /// Children that each run for three polls, then reach `ready` as a real child would:
        /// their record, then their outcome.
        struct Slow {
            base: PathBuf,
            started: RefCell<Vec<String>>,
            ended: RefCell<Vec<String>>,
        }

        struct SlowChild<'a> {
            spawner: &'a Slow,
            child: Child,
            polls: u32,
        }

        impl Running for SlowChild<'_> {
            fn poll(&mut self) -> Result<Option<Exit>> {
                if self.polls > 0 {
                    self.polls -= 1;
                    return Ok(None);
                }
                let args = self.child.args.join(" ");
                let issue: u32 = self.child.args[3].parse().unwrap();
                let location = if args.contains("--cloud") {
                    Location::Cloud
                } else {
                    Location::Local
                };
                write_record(&self.spawner.base, issue, Act::Ready, location);
                let dir = record::task_dir(&self.spawner.base, &format!("issue-{issue}"));
                outcome::write(&dir, &Outcome::new(Kind::Ready, None, "")).unwrap();
                self.spawner.ended.borrow_mut().push(args);
                Ok(Some(Exit {
                    success: true,
                    how: "exited with code 0".into(),
                    errors: String::new(),
                }))
            }
        }

        impl Spawner for Slow {
            fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
                panic!("a loop with several children never waits on one: {child:?}")
            }

            fn start(&self, child: &Child) -> Result<Box<dyn Running + '_>> {
                self.started.borrow_mut().push(child.args.join(" "));
                Ok(Box::new(SlowChild {
                    spawner: self,
                    child: child.clone(),
                    polls: 3,
                }))
            }

            fn stop(&self) {}
        }

        /// The real steps for a task's phases, the loop's other steps played.
        struct Mixed<'a> {
            real: TaskSteps<'a>,
            play: Play<'a>,
        }

        impl Steps for Mixed<'_> {
            fn start_task(
                &self,
                issue: u32,
                phase: Phase,
                out: &mut dyn Write,
                warn: &mut dyn Write,
            ) -> Box<dyn Started + '_> {
                self.real.start_task(issue, phase, out, warn)
            }
            fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
                self.real.run(issue, out, warn)
            }
            fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
                self.real.resume(issue, out, warn)
            }
            fn finish(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
                self.play.finish(issue, out, warn)
            }
            fn file_findings(
                &self,
                issue: u32,
                out: &mut dyn Write,
                warn: &mut dyn Write,
            ) -> Result<()> {
                self.play.file_findings(issue, out, warn)
            }
            fn fix_pr(&self, pr: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
                self.play.fix_pr(pr, out, warn)
            }
            fn fix_check(
                &self,
                pr: u32,
                run: u64,
                out: &mut dyn Write,
                warn: &mut dyn Write,
            ) -> Result<()> {
                self.play.fix_check(pr, run, out, warn)
            }
            fn rm_merged(&self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
                self.play.rm_merged(out, warn)
            }
            fn room(&self) -> Result<()> {
                Ok(())
            }
        }

        #[test]
        fn the_real_steps_run_a_local_and_a_cloud_child_at_once_each_line_naming_its_slot() {
            let f = fixture(Some(1));
            let gh = github(&[(1, &[]), (2, &[])]);
            let (backend, host) = (FakeBackend::default(), FakeHostRunner::default());
            let spawner = Slow {
                base: f.base(),
                started: RefCell::default(),
                ended: RefCell::default(),
            };
            let config_dir = f.env.config_dir();
            let steps = Mixed {
                real: TaskSteps {
                    config_dir: &config_dir,
                    repo_root: f.repo_root.clone(),
                    repo: Some(REPO.into()),
                    backend: &backend,
                    github: &gh,
                    probe: &Probe,
                    host: &host,
                    disk: sbxm::disk::Guard::default(),
                    stop: None,
                    spawner: &spawner,
                },
                play: Play::new(&f.base()),
            };

            let run = run_loop(&f, &steps, &gh, &never);

            run.result.unwrap();
            assert_eq!(
                *spawner.started.borrow(),
                [
                    "task run --issue 1 --local --repo o/r",
                    "task run --issue 2 --cloud --repo o/r"
                ],
                "{}",
                run.out
            );
            // Both started before either ended.
            assert_eq!(spawner.ended.borrow().len(), 2);
            let out = &run.out;
            let second_start = out.find("[issue-2 cloud] task run started").unwrap();
            let first_end = out.find("[issue-1 local] task run ended: ready").unwrap();
            assert!(second_start < first_end, "{out}");
            assert!(
                out.contains("[issue-2 cloud] task run ended: ready"),
                "{out}"
            );
        }
    }
}
