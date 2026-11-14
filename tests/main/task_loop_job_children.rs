//! Issue 633 (milestone 5 slice 5, decisions 317(7), (8), 321): with `[resources] cloud_slots`
//! set, the loop's resolves, log-fixes and queued jobs run as child `sbxm` processes it supervises
//! next to its task children, one per PR at a time, ahead of new issues; `rm-merged` stays in the
//! loop's process.

use crate::common;

// --- each job's child command ---

mod commands {
    use std::cell::RefCell;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    use anyhow::Result;
    use sbxm::backend::{FakeBackend, Location};
    use sbxm::commands::task_loop::child::{Child, Exit, Running, Spawner};
    use sbxm::commands::task_loop::{JobStep, PhaseEnd, Started, Steps, TaskSteps};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::github::{PrInfo, PrState};
    use sbxm::harness::Harness;
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::outcome::{self, Kind, Outcome};

    use super::common;
    use super::common::task_fixture::Probe;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    /// Children that exit at once, having written a `ready` outcome where the loop named it.
    #[derive(Default)]
    struct Instant {
        started: RefCell<Vec<Child>>,
    }

    struct Done;

    impl Running for Done {
        fn poll(&mut self) -> Result<Option<Exit>> {
            Ok(Some(Exit {
                success: true,
                how: "exited with code 0".into(),
                errors: String::new(),
            }))
        }
    }

    impl Spawner for Instant {
        fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
            panic!("a job child is never waited on: {child:?}")
        }

        fn start(&self, child: &Child) -> Result<Box<dyn Running + '_>> {
            let file = child
                .env
                .iter()
                .find(|(k, _)| k == outcome::LOOP_FILE_ENV)
                .map(|(_, v)| PathBuf::from(v))
                .expect("the loop names an outcome file");
            outcome::write_file(&file, &Outcome::new(Kind::Ready, None, "")).unwrap();
            self.started.borrow_mut().push(child.clone());
            Ok(Box::new(Done))
        }

        fn stop(&self) {}
    }

    fn pr(number: u32) -> PrInfo {
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

    /// Starts `job` with the real steps and the instant spawner: the child's arguments, the
    /// window's text, and how it ended.
    fn start(job: &JobStep, slot: Location) -> (Vec<String>, String, PhaseEnd) {
        let env = common::Env::new();
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let gh = FakeGitHub::default().with_pr(pr(150));
        let (backend, host) = (FakeBackend::default(), FakeHostRunner::default());
        let spawner = Instant::default();
        let config_dir = env.config_dir();
        let steps = TaskSteps {
            config_dir: &config_dir,
            repo_root,
            repo: Some("o/r".into()),
            backend: &backend,
            github: &gh,
            probe: &Probe,
            host: &host,
            disk: sbxm::disk::Guard::default(),
            stop: None,
            spawner: &spawner,
        };
        let mut out = Vec::new();
        let ended = {
            let mut started = steps.start_job(job, slot, &mut out, &mut Vec::new());
            poll_to_end(started.as_mut(), &mut out)
        };
        let args = spawner.started.borrow()[0].args.clone();
        (args, String::from_utf8(out).unwrap(), ended)
    }

    fn poll_to_end(started: &mut dyn Started, out: &mut dyn Write) -> PhaseEnd {
        for _ in 0..10 {
            if let Some(ended) = started.poll(out) {
                return ended;
            }
        }
        panic!("the child never ended")
    }

    fn ready() -> PhaseEnd {
        PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, ""))
    }

    #[test]
    fn a_fix_runs_as_a_task_fix_child_and_its_outcome_is_read() {
        let job = JobStep::Fix {
            pr: 150,
            check_run: None,
        };

        let (args, out, ended) = start(&job, Location::Cloud);

        assert_eq!(args, ["task", "fix", "--pr", "150", "--repo", "o/r"]);
        assert!(
            out.contains("[fix-150-0123456789ab cloud] task fix started"),
            "{out}"
        );
        assert!(
            out.contains("[fix-150-0123456789ab cloud] task fix ended: ready"),
            "{out}"
        );
        assert_eq!(ended, ready());
    }

    #[test]
    fn a_log_fix_runs_as_a_task_fix_child_with_its_check_run() {
        let job = JobStep::Fix {
            pr: 150,
            check_run: Some(77),
        };

        let (args, _, ended) = start(&job, Location::Local);

        assert_eq!(
            args,
            [
                "task",
                "fix",
                "--pr",
                "150",
                "--check-run",
                "77",
                "--repo",
                "o/r"
            ]
        );
        assert_eq!(ended, ready());
    }

    #[test]
    fn a_review_runs_as_a_task_review_child() {
        let (args, out, ended) = start(&JobStep::Review { pr: 150 }, Location::Local);

        assert_eq!(args, ["task", "review", "--pr", "150", "--repo", "o/r"]);
        assert!(out.contains("[pr-150 local] task review started"), "{out}");
        assert_eq!(ended, ready());
    }

    #[test]
    fn a_spec_runs_as_a_task_run_child_and_its_should_fix_round_as_a_resume_child() {
        let tmp = tempfile::TempDir::new().unwrap();
        let spec = tmp.path().join("tidy.md");
        fs::write(&spec, "# Tidy\n").unwrap();
        let shown = spec.display().to_string();

        let (args, out, ended) = start(&JobStep::RunSpec(spec.clone()), Location::Local);
        assert_eq!(args, ["task", "run", "--spec", &shown, "--repo", "o/r"]);
        assert!(out.contains("[spec-tidy-"), "{out}");
        assert_eq!(ended, ready());

        let (args, _, ended) = start(&JobStep::ShouldFixSpec(spec), Location::Local);
        assert_eq!(
            args,
            [
                "task",
                "resume",
                "--spec",
                &shown,
                "--rounds",
                "1",
                "--should-fix"
            ]
        );
        assert_eq!(ended, ready());
    }

    #[test]
    fn a_template_build_of_a_profile_name_that_isnt_one_starts_nothing_and_removes_nothing() {
        let env = common::Env::new();
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        // Where a name climbing out of the outcome folder would point.
        let planted = env.base_dir().join(".sbxm").join("x.json");
        fs::create_dir_all(planted.parent().unwrap()).unwrap();
        fs::write(&planted, "keep").unwrap();
        let gh = FakeGitHub::default();
        let (backend, host) = (FakeBackend::default(), FakeHostRunner::default());
        let spawner = Instant::default();
        let config_dir = env.config_dir();
        let steps = TaskSteps {
            config_dir: &config_dir,
            repo_root,
            repo: None,
            backend: &backend,
            github: &gh,
            probe: &Probe,
            host: &host,
            disk: sbxm::disk::Guard::default(),
            stop: None,
            spawner: &spawner,
        };
        let job = JobStep::Template {
            profile: "../x".into(),
            harness: Harness::Claude,
            warm: None,
        };
        let mut out = Vec::new();

        let ended = {
            let mut started = steps.start_job(&job, Location::Local, &mut out, &mut Vec::new());
            poll_to_end(started.as_mut(), &mut out)
        };

        let PhaseEnd::Outcome(o) = &ended else {
            panic!("{ended:?}")
        };
        assert_eq!(o.end, Kind::Failed, "{o:?}");
        assert!(o.detail.contains("invalid profile name"), "{o:?}");
        assert!(spawner.started.borrow().is_empty());
        assert_eq!(fs::read_to_string(&planted).unwrap(), "keep");
    }

    #[test]
    fn a_template_build_runs_as_a_template_build_child() {
        let warm = PathBuf::from("checkout");
        let job = JobStep::Template {
            profile: "rust".into(),
            harness: Harness::Codex,
            warm: Some(warm.clone()),
        };

        let (args, out, ended) = start(&job, Location::Local);

        let warm = warm.display().to_string();
        assert_eq!(
            args,
            [
                "template",
                "build",
                "rust",
                "--harness",
                "codex",
                "--warm",
                &warm
            ]
        );
        assert!(
            out.contains("[template-rust-codex local] template build started"),
            "{out}"
        );
        assert_eq!(ended, ready());
    }
}

// --- the real binary as a job's child ---

mod real_binary {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use sbxm::backend::{FakeBackend, Location};
    use sbxm::commands::task_loop::child::ChildSpawner;
    use sbxm::commands::task_loop::{JobStep, PhaseEnd, Steps, TaskSteps};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::harness::Harness;
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::outcome::{self, Kind};

    use super::common;
    use super::common::task_fixture::Probe;

    #[test]
    fn a_template_build_child_writes_its_outcome_where_the_loop_said() {
        let env = common::Env::new();
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let (gh, backend, host) = (
            FakeGitHub::default(),
            FakeBackend::default(),
            FakeHostRunner::default(),
        );
        let spawner = ChildSpawner::new(PathBuf::from(env!("CARGO_BIN_EXE_sbxm")));
        let config_dir = env.config_dir();
        let steps = TaskSteps {
            config_dir: &config_dir,
            repo_root,
            repo: Some("o/r".into()),
            backend: &backend,
            github: &gh,
            probe: &Probe,
            host: &host,
            disk: sbxm::disk::Guard::default(),
            stop: None,
            spawner: &spawner,
        };
        let job = JobStep::Template {
            profile: "no-such-profile".into(),
            harness: Harness::Claude,
            warm: None,
        };
        let mut out = Vec::new();

        let mut started = steps.start_job(&job, Location::Local, &mut out, &mut Vec::new());
        let begun = Instant::now();
        let ended = loop {
            if let Some(ended) = started.poll(&mut out) {
                break ended;
            }
            assert!(begun.elapsed() < Duration::from_secs(60));
            std::thread::sleep(Duration::from_millis(50));
        };

        // A build refused before it made anything: no task, so `refused`, read from the file.
        let PhaseEnd::Outcome(o) = &ended else {
            panic!("{ended:?}")
        };
        assert_eq!(o.end, Kind::Refused, "{o:?}");
        assert!(o.detail.contains("no-such-profile"), "{o:?}");
        assert!(
            !outcome::loop_file(&env.base_dir(), "template-no-such-profile-claude").exists(),
            "read, then removed"
        );
    }
}

// --- the loop with job children, played ---

/// Helpers shared by the loop's tests below; not every test uses every one.
#[allow(dead_code)]
mod play {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Result, anyhow};
    use sbxm::backend::Location;
    use sbxm::commands::task_loop::{self, Env, JobStep, Options, Phase, PhaseEnd, Started, Steps};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::github::{PrInfo, PrState};
    use sbxm::task::outcome::{Kind, Outcome};
    use sbxm::task::record::{self, NewTask, Process, Record, ReviewResult, Stage, Status};

    use super::common;
    use super::common::task_fixture::{Probe, issue_text, open_issue};

    pub const REPO: &str = "o/r";
    const READY: &str = "ready-for-worker";
    /// 2026-11-02T10:00:00Z.
    pub const START: u64 = 1_793_613_600;
    pub const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A played job's name in the events: `fix 150`, `log-fix 150`, `review 150`, `spec tidy.md`,
    /// `should-fix tidy.md`, `template rust`.
    pub fn job_name(job: &JobStep) -> String {
        match job {
            JobStep::Fix {
                pr,
                check_run: None,
            } => format!("fix {pr}"),
            JobStep::Fix {
                pr,
                check_run: Some(_),
            } => format!("log-fix {pr}"),
            JobStep::Review { pr } => format!("review {pr}"),
            JobStep::RunSpec(spec) => {
                format!("spec {}", spec.file_name().unwrap().to_string_lossy())
            }
            JobStep::ShouldFixSpec(spec) => {
                format!("should-fix {}", spec.file_name().unwrap().to_string_lossy())
            }
            JobStep::Template { profile, .. } => format!("template {profile}"),
        }
    }

    /// Plays each issue's phases and each job as children that end after a number of polls;
    /// `events` says what started and ended, in order.
    pub struct Play<'a> {
        base: PathBuf,
        /// Each issue's phases, in order: polls before it ends.
        issues: RefCell<HashMap<u32, VecDeque<(u32, Killed)>>>,
        /// Each job's runs by its name, in order: polls before it ends, and how.
        jobs: RefCell<HashMap<String, VecDeque<(u32, Kind)>>>,
        pub events: RefCell<Vec<String>>,
        running: Cell<usize>,
        pub most: Cell<usize>,
        /// Run as each job starts, by its name: GitHub changing meanwhile.
        on_job: Box<dyn Fn(&str) + 'a>,
        /// Run as each issue's phase starts.
        on_issue: Box<dyn Fn(u32) + 'a>,
        /// Run as each job ends, by its name.
        on_job_end: Box<dyn Fn(&str) + 'a>,
        /// The review a spec task's run leaves.
        pub spec_review: &'static str,
    }

    impl<'a> Play<'a> {
        pub fn new(base: &Path) -> Self {
            Self {
                base: base.to_owned(),
                issues: RefCell::default(),
                jobs: RefCell::default(),
                events: RefCell::default(),
                running: Cell::new(0),
                most: Cell::new(0),
                on_job: Box::new(|_| {}),
                on_issue: Box::new(|_| {}),
                on_job_end: Box::new(|_| {}),
                spec_review: CLEAN,
            }
        }

        pub fn issue(self, issue: u32, polls: u32) -> Self {
            self.issue_ending(issue, polls, Killed::No)
        }

        /// An issue's phase that ends after `polls` polls, killed as `killed` says.
        pub fn issue_ending(self, issue: u32, polls: u32, killed: Killed) -> Self {
            self.issues
                .borrow_mut()
                .entry(issue)
                .or_default()
                .push_back((polls, killed));
            self
        }

        pub fn job(self, name: &str, polls: u32, kind: Kind) -> Self {
            self.jobs
                .borrow_mut()
                .entry(name.to_owned())
                .or_default()
                .push_back((polls, kind));
            self
        }

        pub fn on_job(mut self, f: impl Fn(&str) + 'a) -> Self {
            self.on_job = Box::new(f);
            self
        }

        pub fn on_issue(mut self, f: impl Fn(u32) + 'a) -> Self {
            self.on_issue = Box::new(f);
            self
        }

        pub fn on_job_end(mut self, f: impl Fn(&str) + 'a) -> Self {
            self.on_job_end = Box::new(f);
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

        pub fn has(&self, event: &str) -> bool {
            self.events().iter().any(|e| e == event)
        }

        fn started(&self, event: String) {
            self.events.borrow_mut().push(event);
            self.running.set(self.running.get() + 1);
            self.most.set(self.most.get().max(self.running.get()));
        }
    }

    pub const CLEAN: &str = "Must-fix findings: 0\n";
    /// A clean review with one should-fix finding sized S.
    pub const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
        ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";
    /// The PR a spec task's finish opens.
    pub const SPEC_PR: u32 = 160;

    /// Spec `spec`'s task as its run leaves it: ready, clean, with `review`.
    fn write_spec(base: &Path, spec: &Path, review: &str) {
        let id = record::spec_id(spec).unwrap().0;
        let dir = record::task_dir(base, &id);
        let mut rec = Record::new(
            &NewTask {
                kind: record::Kind::Spec,
                number: 0,
                repo: REPO,
                title: "Retro",
                base: "main",
                branch: &format!("sbxm/{id}"),
                config_hash: "h",
                id: Some(&id),
            },
            START,
            Process::new(1, 0),
        );
        rec.stage = Stage::Ready;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            must_fix: 0,
            not_run: Some(Vec::new()),
            ..ReviewResult::default()
        });
        record::write(&dir, &rec).unwrap();
        fs::write(dir.join("review.md"), review).unwrap();
    }

    /// Issue `issue`'s record as a child leaves it, ready, in `location`.
    pub fn write_ready(base: &Path, issue: u32, location: Location) {
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
        rec.stage = Stage::Ready;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            must_fix: 0,
            not_run: Some(Vec::new()),
            ..ReviewResult::default()
        });
        record::write(&dir, &rec).unwrap();
    }

    /// How a played issue phase is killed before it writes its outcome.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Killed {
        No,
        /// Before its task's record exists.
        BeforeRecord,
        /// After its task's record was written.
        AfterRecord,
    }

    enum Who {
        Issue(u32, Location, Killed),
        Job(String, Kind, JobStep),
    }

    struct Playing<'p, 'a> {
        play: &'p Play<'a>,
        who: Who,
        polls: u32,
    }

    impl Started for Playing<'_, '_> {
        fn poll(&mut self, _out: &mut dyn Write) -> Option<PhaseEnd> {
            if self.polls > 0 {
                self.polls -= 1;
                return None;
            }
            self.play.running.set(self.play.running.get() - 1);
            Some(match &self.who {
                Who::Issue(issue, location, killed) => {
                    if *killed != Killed::BeforeRecord {
                        write_ready(&self.play.base, *issue, *location);
                    }
                    self.play.events.borrow_mut().push(format!("end {issue}"));
                    if *killed != Killed::No {
                        return Some(PhaseEnd::Missing(
                            "the task's child sbxm was killed and wrote no outcome".into(),
                        ));
                    }
                    PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, ""))
                }
                Who::Job(name, kind, job) => {
                    match job {
                        JobStep::RunSpec(spec) if *kind == Kind::Ready => {
                            write_spec(&self.play.base, spec, self.play.spec_review);
                        }
                        JobStep::ShouldFixSpec(spec) if *kind == Kind::Ready => {
                            let id = record::spec_id(spec).unwrap().0;
                            let dir = record::task_dir(&self.play.base, &id);
                            let mut rec = record::read(&dir.join("task.json")).unwrap();
                            rec.should_fix_round = true;
                            record::write(&dir, &rec).unwrap();
                            fs::write(dir.join("review.md"), CLEAN).unwrap();
                        }
                        _ => {}
                    }
                    self.play.events.borrow_mut().push(format!("end {name}"));
                    (self.play.on_job_end)(name);
                    // `Environment` stands for a child killed before it wrote an outcome.
                    if *kind == Kind::Environment {
                        return Some(PhaseEnd::Missing(
                            "the task's child sbxm was killed and wrote no outcome".into(),
                        ));
                    }
                    let detail = if *kind == Kind::Ready {
                        ""
                    } else {
                        "it failed"
                    };
                    PhaseEnd::Outcome(Outcome::new(*kind, None, detail))
                }
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
            self.started(format!("start {issue} {}", location.as_str()));
            (self.on_issue)(issue);
            let (polls, killed) = self
                .issues
                .borrow_mut()
                .get_mut(&issue)
                .and_then(VecDeque::pop_front)
                .unwrap_or((0, Killed::No));
            Box::new(Playing {
                play: self,
                who: Who::Issue(issue, location, killed),
                polls,
            })
        }

        fn start_job(
            &self,
            job: &JobStep,
            slot: Location,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Box<dyn Started + '_> {
            let name = job_name(job);
            self.started(format!("start {name} {}", slot.as_str()));
            (self.on_job)(&name);
            let (polls, kind) = self
                .jobs
                .borrow_mut()
                .get_mut(&name)
                .and_then(VecDeque::pop_front)
                .unwrap_or((0, Kind::Ready));
            Box::new(Playing {
                play: self,
                who: Job(name, kind, job.clone()),
                polls,
            })
        }

        fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            Err(anyhow!("#{issue} run in the loop's process"))
        }

        fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            Err(anyhow!("#{issue} resumed in the loop's process"))
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
            self.events
                .borrow_mut()
                .push(format!("in-process fix {pr}"));
            Ok(())
        }

        fn fix_check(
            &self,
            pr: u32,
            _run: u64,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Result<()> {
            self.events
                .borrow_mut()
                .push(format!("in-process log-fix {pr}"));
            Ok(())
        }

        fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
            Ok(())
        }

        fn finish_spec(
            &self,
            spec: &Path,
            _out: &mut dyn Write,
            _warn: &mut dyn Write,
        ) -> Result<()> {
            let name = spec.file_name().unwrap().to_string_lossy().into_owned();
            self.events.borrow_mut().push(format!("finish spec {name}"));
            let dir = record::task_dir(&self.base, &record::spec_id(spec)?.0);
            let mut rec = record::read(&dir.join("task.json"))?;
            rec.stage = Stage::Finished;
            rec.pr = Some(format!("https://github.com/{REPO}/pull/{SPEC_PR}"));
            record::write(&dir, &rec)
        }

        fn room(&self) -> Result<()> {
            Ok(())
        }
    }

    use Who::Job;

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

        /// Queues job file `name`; `order` sets its modified time, so jobs run in its order.
        pub fn queue(&self, name: &str, text: &str, order: u64) {
            let queue = sbxm::task::jobs::queue_dir(&self.base());
            fs::create_dir_all(&queue).unwrap();
            let path = queue.join(name);
            fs::write(&path, text).unwrap();
            let at = std::time::UNIX_EPOCH + Duration::from_secs(START + order);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }

        pub fn queued(&self, name: &str) -> bool {
            sbxm::task::jobs::queue_dir(&self.base())
                .join(name)
                .is_file()
        }
    }

    /// `sandbox_slots` unset (one local), and `cloud_slots` as given.
    pub fn fixture(cloud_slots: Option<u32>) -> Fixture {
        fixture_with(cloud_slots, "")
    }

    /// [`fixture`] with `extra` at the end of `sbxm-task.toml`.
    pub fn fixture_with(cloud_slots: Option<u32>, extra: &str) -> Fixture {
        let env = common::Env::new();
        if let Some(n) = cloud_slots {
            common::cloud_slot::set_cloud_slots(&env, n);
        }
        let repo_root = env.tmp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        fs::write(
            repo_root.join("sbxm-task.toml"),
            format!("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n{extra}"),
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
        write_ready(base, issue, location);
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

    /// Runs the loop over `issues` (none: the labeled queue) to its stop, its clock moving with
    /// its sleeps; `stop` is asked whenever the loop looks for a stop request.
    pub fn run_loop(
        f: &Fixture,
        steps: &dyn Steps,
        gh: &FakeGitHub,
        stop: &dyn Fn() -> bool,
        issues: &[u32],
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
                issues: issues.to_vec(),
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

    pub fn never() -> bool {
        false
    }

    pub fn starts(play: &Play) -> Vec<String> {
        play.events()
            .into_iter()
            .filter(|e| e.starts_with("start"))
            .collect()
    }
}

// --- resolves ---

mod resolves {
    use sbxm::backend::Location;
    use sbxm::github::Mergeable;
    use sbxm::task::outcome::Kind;

    use super::play::*;

    #[test]
    fn a_task_childs_end_is_acted_on_while_a_resolve_child_runs() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        let gh = github(&[(1, &[])]).with_pr(pr(150)).with_pr(pr(101));
        gh.set_mergeable(150, Mergeable::Conflicting);
        let play = Play::new(&f.base())
            .issue(1, 3)
            .job("fix 150", 30, Kind::Ready);

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            ["start fix 150 cloud", "start 1 local"],
            "{}",
            run.out
        );
        // Its finish, while the resolve still runs.
        assert!(play.at("finish 1") < play.at("end fix 150"), "{}", run.out);
        assert!(!play.has("in-process fix 150"), "{}", run.out);
        assert!(
            run.out
                .contains("conflicts with main; resolving it with `sbxm task fix --pr 150`"),
            "{}",
            run.out
        );
        let log = f.log_text();
        assert!(log.contains("resolve: conflicts with main"), "{log}");
        // #1's row, and its arming, as it ended.
        assert!(log.contains("| #1 Fix 1 | S | local |"), "{log}");
        assert!(
            run.out.contains("[issue-1 local] #1 (Fix 1): PR #101"),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_resolve_waiting_for_its_slot_takes_it_before_a_new_issue_for_that_place() {
        let f = fixture(Some(1));
        loop_pr(&f.base(), 50, 150, Location::Local);
        let gh = github(&[
            (1, &["run-local"]),
            (2, &["run-cloud"]),
            (3, &["run-local"]),
        ])
        .with_pr(pr(150))
        .with_pr(pr(101))
        .with_pr(pr(102))
        .with_pr(pr(103));
        // The PR starts conflicting once #1 holds the local slot.
        let play = Play::new(&f.base())
            .issue(1, 10)
            .issue(2, 2)
            .job("fix 150", 10, Kind::Ready)
            .on_issue(|issue| {
                if issue == 1 {
                    gh.set_mergeable(150, Mergeable::Conflicting);
                }
            });

        let run = run_loop(&f, &play, &gh, &never, &[1, 2, 3]);

        run.result.unwrap();
        assert!(
            run.out
                .contains("its resolve waits for a free local slot; tried again at a later pass"),
            "{}",
            run.out
        );
        // The cloud issue starts meanwhile; the resolve goes before #3 in the local slot.
        assert!(play.at("start 2 cloud") < play.at("end 1"), "{}", run.out);
        assert!(play.at("end 1") < play.at("start fix 150 local"));
        assert!(
            play.at("end fix 150") < play.at("start 3 local"),
            "{}",
            run.out
        );
    }
}

// --- log-fixes ---

mod log_fixes {
    use sbxm::backend::Location;
    use sbxm::github::{Mergeable, RunStatus, WorkflowRun};
    use sbxm::task::outcome::Kind;

    use super::play::*;

    /// PR 150's head, apart from `main`'s.
    const PR_HEAD: &str = "fedcba9876543210fedcba9876543210fedcba98";

    /// PR 150 at [`PR_HEAD`].
    fn pr150() -> sbxm::github::PrInfo {
        sbxm::github::PrInfo {
            head_oid: PR_HEAD.into(),
            ..pr(150)
        }
    }

    /// PR 150's run on [`PR_HEAD`], failed again after its re-run.
    fn failed_twice(gh: &sbxm::github::fake::FakeGitHub) {
        gh.set_runs(
            PR_HEAD,
            vec![WorkflowRun {
                id: 77,
                workflow: "ci".into(),
                attempt: 2,
                status: RunStatus::Failed,
                url: "https://github.com/o/r/actions/runs/77".into(),
                actor: "alice".into(),
            }],
        );
    }

    #[test]
    fn a_log_fix_runs_as_a_child_while_a_task_child_is_finished() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        let gh = github(&[(1, &[])]).with_pr(pr150()).with_pr(pr(101));
        failed_twice(&gh);
        let play = Play::new(&f.base())
            .issue(1, 3)
            .job("log-fix 150", 30, Kind::Ready);

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            ["start log-fix 150 cloud", "start 1 local"],
            "{}",
            run.out
        );
        assert!(
            play.at("finish 1") < play.at("end log-fix 150"),
            "{}",
            run.out
        );
        assert!(!play.has("in-process log-fix 150"), "{}", run.out);
        let log = f.log_text();
        assert!(log.contains("log-fix (task issue-50)"), "{log}");
    }

    #[test]
    fn a_log_fix_waits_while_a_resolve_runs_on_its_pr_then_runs() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        let gh = github(&[]).with_pr(pr150());
        gh.set_mergeable(150, Mergeable::Conflicting);
        failed_twice(&gh);
        let play = Play::new(&f.base()).job("fix 150", 5, Kind::Ready);

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert!(
            run.out
                .contains("its log-fix waits for the job running on it"),
            "{}",
            run.out
        );
        assert!(
            play.at("end fix 150") < play.at("start log-fix 150 cloud"),
            "{}",
            run.out
        );
        assert_eq!(play.most.get(), 1, "{:?}", play.events());
    }
}

// --- queued jobs ---

mod queued {
    use std::cell::RefCell;

    use sbxm::backend::Location;
    use sbxm::github::{GitHubBackend, Mergeable};
    use sbxm::task::outcome::Kind;

    use super::play::*;

    const NEW_HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn a_job_waiting_for_its_slot_goes_before_a_new_issue_there_and_the_other_place_starts() {
        let f = fixture(Some(1));
        loop_pr(&f.base(), 50, 150, Location::Local);
        let gh = github(&[
            (1, &["run-local"]),
            (2, &["run-cloud"]),
            (3, &["run-local"]),
        ])
        .with_pr(pr(150))
        .with_pr(pr(101))
        .with_pr(pr(102))
        .with_pr(pr(103));
        // The review is queued once #1 holds the local slot.
        let play = Play::new(&f.base())
            .issue(1, 10)
            .issue(2, 2)
            .job("review 150", 10, Kind::Ready)
            .on_issue(|issue| {
                if issue == 1 {
                    f.queue("review-150.json", r#"{"kind": "review", "pr": 150}"#, 1);
                }
            });

        let run = run_loop(&f, &play, &gh, &never, &[1, 2, 3]);

        run.result.unwrap();
        assert!(
            run.out.contains(
                "job review #150: waits for a free local slot; tried again at a later pass"
            ),
            "{}",
            run.out
        );
        assert!(play.at("start 2 cloud") < play.at("end 1"), "{}", run.out);
        // Once #1 ends, the review takes the local slot, not #3.
        assert!(play.at("end 1") < play.at("start review 150 local"));
        assert!(
            play.at("end review 150") < play.at("start 3 local"),
            "{}",
            run.out
        );
        assert!(
            run.out
                .contains("job review #150: done: `task review --pr 150` ended"),
            "{}",
            run.out
        );
    }

    #[test]
    fn two_jobs_on_one_pr_run_one_after_the_other_and_a_job_on_another_pr_runs_meanwhile() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        loop_pr(&f.base(), 51, 151, Location::Cloud);
        let gh = github(&[]).with_pr(pr(150)).with_pr(pr(151));
        f.queue("a-review-150.json", r#"{"kind": "review", "pr": 150}"#, 1);
        f.queue("b-fix-150.json", r#"{"kind": "fix", "pr": 150}"#, 2);
        f.queue("c-fix-151.json", r#"{"kind": "fix", "pr": 151}"#, 3);
        // The fix of PR 150 reads its head as it starts; the review moved it.
        let heads = RefCell::new(Vec::new());
        let play = Play::new(&f.base())
            .job("review 150", 10, Kind::Ready)
            .job("fix 151", 2, Kind::Ready)
            .on_job(|name| {
                if name == "fix 150" {
                    heads.borrow_mut().push(gh.pr("o/r", 150).unwrap().head_oid);
                }
            })
            .on_job_end(|name| {
                if name == "review 150" {
                    gh.set_pr_head(150, NEW_HEAD);
                }
            });

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert!(
            run.out.contains(
                "job fix #150: waits for the job running on PR #150; tried again at a later pass"
            ),
            "{}",
            run.out
        );
        assert!(play.at("start fix 151 cloud") < play.at("end review 150"));
        assert!(play.at("end review 150") < play.at("start fix 150 cloud"));
        assert_eq!(*heads.borrow(), [NEW_HEAD], "{}", run.out);
        assert!(!f.queued("b-fix-150.json"));
    }

    #[test]
    fn a_resolve_waiting_behind_a_fix_that_removed_the_conflict_is_dropped() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        let gh = github(&[]).with_pr(pr(150));
        f.queue("fix-150.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        // The PR conflicts while its queued fix runs, and the fix's merge of main resolves it.
        // It runs past the loop's next pass of its own steps.
        let play = Play::new(&f.base())
            .job("fix 150", 400, Kind::Ready)
            .on_job(|_| gh.set_mergeable(150, Mergeable::Conflicting))
            .on_job_end(|_| gh.set_mergeable(150, Mergeable::Mergeable));

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert!(
            run.out.contains(
                "its resolve waits for the job running on it; tried again at a later pass"
            ),
            "{}",
            run.out
        );
        assert_eq!(starts(&play), ["start fix 150 cloud"], "{}", run.out);
        assert!(!run.out.contains("resolving it with"), "{}", run.out);
    }

    #[test]
    fn a_job_child_killed_before_its_outcome_is_reported_interrupted_and_not_run_again() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        let gh = github(&[]).with_pr(pr(150));
        f.queue("fix-150.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        let play = Play::new(&f.base()).job("fix 150", 2, Kind::Environment);

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start fix 150 cloud"], "{}", run.out);
        assert!(
            run.out.contains(
                "job fix #150: interrupted: the task's child sbxm was killed and wrote no \
                 outcome; not run again"
            ),
            "{}",
            run.out
        );
    }

    #[test]
    fn fix_review_and_template_jobs_run_as_children_and_rm_merged_in_the_loop() {
        let f = fixture(Some(2));
        f.env.write_profile("rust", "[template]\nallow = []\n");
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        loop_pr(&f.base(), 51, 151, Location::Cloud);
        let gh = github(&[]).with_pr(pr(150)).with_pr(pr(151));
        f.queue("a.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        f.queue("b.json", r#"{"kind": "review", "pr": 151}"#, 2);
        f.queue("c.json", r#"{"kind": "template", "profile": "rust"}"#, 3);
        f.queue("d.json", r#"{"kind": "rm-merged"}"#, 4);
        let play = Play::new(&f.base()).job("fix 150", 3, Kind::Ready).job(
            "template rust",
            3,
            Kind::Ready,
        );

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            [
                "start fix 150 cloud",
                "start review 151 local",
                // The local slot is the review's: the template build waits for it.
                "start template rust local",
            ],
            "{}",
            run.out
        );
        assert!(play.at("end review 151") < play.at("start template rust local"));
        assert!(
            run.out
                .contains("job rm-merged: done: merged tasks removed"),
            "{}",
            run.out
        );
        assert!(
            run.out
                .contains("job template rust: done: built the template of rust"),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_stop_while_a_job_child_runs_lets_it_finish_and_leaves_the_queued_jobs_queued() {
        let f = fixture(Some(2));
        loop_pr(&f.base(), 50, 150, Location::Cloud);
        loop_pr(&f.base(), 51, 151, Location::Cloud);
        let gh = github(&[]).with_pr(pr(150)).with_pr(pr(151));
        f.queue("a.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        f.queue("b.json", r#"{"kind": "review", "pr": 150}"#, 2);
        f.queue("c.json", r#"{"kind": "fix", "pr": 151}"#, 3);
        let play = Play::new(&f.base()).job("fix 150", 10, Kind::Ready);
        let stop = || play.has("start fix 150 cloud");

        let run = run_loop(&f, &play, &gh, &stop, &[]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start fix 150 cloud"], "{}", run.out);
        play.at("end fix 150");
        assert!(run.out.contains("job fix #150: done"), "{}", run.out);
        assert!(f.queued("b.json") && f.queued("c.json"), "{}", run.out);
        assert!(run.out.ends_with("stopped on request\n"), "{}", run.out);
    }
}

// --- spec jobs ---

mod specs {
    use std::fs;

    use sbxm::task::outcome::Kind;

    use super::play::*;

    /// [`fixture_with`] whose `specs_dir` holds `retro.md`.
    fn spec_fixture(extra: &str) -> Fixture {
        let f = fixture_with(Some(2), extra);
        let specs = f.env.tmp.path().join("specs");
        fs::create_dir_all(&specs).unwrap();
        fs::write(specs.join("retro.md"), "# Retro\n").unwrap();
        let value = toml::Value::String(specs.to_str().unwrap().to_owned());
        f.env.set_config(&format!("specs_dir = {value}"));
        f
    }

    #[test]
    fn a_spec_job_runs_and_its_should_fix_round_runs_as_children_then_it_is_finished() {
        let f = spec_fixture("\n[loop]\nshould_fix = \"fix\"\n");
        f.queue("s.json", r#"{"kind": "spec", "file": "retro.md"}"#, 1);
        let gh = github(&[(1, &[])]).with_pr(pr(SPEC_PR)).with_pr(pr(101));
        let mut play = Play::new(&f.base())
            .issue(1, 3)
            .job("spec retro.md", 10, Kind::Ready)
            .job("should-fix retro.md", 10, Kind::Ready);
        play.spec_review = S_ONE;

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert_eq!(
            starts(&play),
            [
                "start spec retro.md local",
                "start 1 cloud",
                "start should-fix retro.md local"
            ],
            "{}",
            run.out
        );
        // #1 is finished while the spec job's children run.
        assert!(play.at("finish 1") < play.at("end spec retro.md"));
        assert!(play.at("end should-fix retro.md") < play.at("finish spec retro.md"));
        assert!(
            run.out.contains("job spec retro.md: done: PR #160, clean"),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_second_spec_job_of_the_same_file_waits_for_the_first_to_end() {
        let f = spec_fixture("");
        // Two local slots: only the rule of one child per task holds the second back.
        super::common::slot::set_slots(&f.env, 2);
        f.queue("s.json", r#"{"kind": "spec", "file": "retro.md"}"#, 1);
        f.queue("t.json", r#"{"kind": "spec", "file": "retro.md"}"#, 2);
        let gh = github(&[]).with_pr(pr(SPEC_PR));
        let play = Play::new(&f.base())
            .job("spec retro.md", 10, Kind::Ready)
            .job("spec retro.md", 1, Kind::Failed);

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert!(
            run.out.contains(
                "job spec retro.md: waits for the job running on it; tried again at a later pass"
            ),
            "{}",
            run.out
        );
        assert_eq!(play.most.get(), 1, "{:?}", play.events());
    }

    #[test]
    fn a_spec_job_whose_run_fails_ends_failed_without_its_finish() {
        let f = spec_fixture("");
        f.queue("s.json", r#"{"kind": "spec", "file": "retro.md"}"#, 1);
        let gh = github(&[]);
        let play = Play::new(&f.base()).job("spec retro.md", 2, Kind::Failed);

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert!(!play.has("finish spec retro.md"), "{}", run.out);
        assert!(
            run.out.contains("job spec retro.md: failed: it failed"),
            "{}",
            run.out
        );
    }
}

// --- run jobs ---

mod runs {
    use super::play::*;

    #[test]
    fn a_queued_run_job_runs_as_a_task_child_next_to_another_task_and_ends_with_its_result() {
        let f = fixture(Some(1));
        let gh = github(&[(1, &[]), (9, &["run-cloud"])])
            .with_pr(pr(101))
            .with_pr(pr(109));
        // Queued once #1 holds the local slot; #1 runs past the loop's next pass of its steps.
        let play = Play::new(&f.base())
            .issue(1, 400)
            .issue(9, 3)
            .on_issue(|issue| {
                if issue == 1 {
                    f.queue("run-9.json", r#"{"kind": "run", "issue": 9}"#, 1);
                }
            });

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert!(play.at("start 9 cloud") < play.at("end 1"), "{}", run.out);
        assert!(play.at("finish 9") < play.at("end 1"), "{}", run.out);
        assert!(
            run.out.contains("job run #9: done: PR #109, clean"),
            "{}",
            run.out
        );
        // The job's row, not a task's.
        let log = f.log_text();
        assert!(log.contains("job run #9"), "{log}");
        assert!(!log.contains("| #9 Fix 9 |"), "{log}");
        assert!(log.contains("#1 Fix 1"), "{log}");
    }
}

// --- the 5-PR pause ---

mod pr_cap {
    use std::cell::Cell;
    use std::fs;

    use sbxm::backend::Location;
    use sbxm::task::outcome::Kind;

    use super::play::*;

    /// Loop PRs 150 and up awaiting the user, `n` of them, on `gh`.
    fn awaiting(
        f: &Fixture,
        gh: sbxm::github::fake::FakeGitHub,
        n: u32,
    ) -> sbxm::github::fake::FakeGitHub {
        let mut gh = gh;
        for i in 0..n {
            loop_pr(&f.base(), 50 + i, 150 + i, Location::Cloud);
            gh = gh.with_pr(pr(150 + i));
        }
        gh
    }

    #[test]
    fn a_spec_job_child_counts_with_the_tasks_toward_five() {
        let f = fixture(Some(2));
        let specs = f.env.tmp.path().join("specs");
        fs::create_dir_all(&specs).unwrap();
        fs::write(specs.join("retro.md"), "# Retro\n").unwrap();
        let value = toml::Value::String(specs.to_str().unwrap().to_owned());
        f.env.set_config(&format!("specs_dir = {value}"));
        f.queue("s.json", r#"{"kind": "spec", "file": "retro.md"}"#, 1);
        let gh = awaiting(&f, github(&[(1, &[]), (2, &[])]), 3)
            .with_pr(pr(101))
            .with_pr(pr(102))
            .with_pr(pr(SPEC_PR))
            .with_auto_merge(101)
            .with_auto_merge(102);
        let play = Play::new(&f.base())
            .job("spec retro.md", 400, Kind::Ready)
            .issue(1, 10)
            .issue(2, 1);

        let run = run_loop(&f, &play, &gh, &never, &[1, 2]);

        run.result.unwrap();
        // 3 awaiting and the spec job: 4, so #1 starts; with #1, 5, so #2 waits for its end.
        assert!(
            run.out
                .contains("paused: 3 PRs await you and 2 tasks run, 5 in all"),
            "{}",
            run.out
        );
        assert!(play.at("start 1 cloud") < play.at("end 1"));
        assert!(
            play.at("finish 1") < play.at("start 2 cloud"),
            "{}",
            run.out
        );
        assert!(
            play.at("start 2 cloud") < play.at("end spec retro.md"),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_fix_job_child_does_not_count_so_an_issue_starts_at_four_awaiting() {
        let f = fixture(Some(2));
        f.queue("fix.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        let gh = awaiting(&f, github(&[(1, &[])]), 4)
            .with_pr(pr(101))
            .with_auto_merge(101);
        let play = Play::new(&f.base())
            .job("fix 150", 400, Kind::Ready)
            .issue(1, 3);

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert!(
            play.at("start 1 local") < play.at("end fix 150"),
            "{}",
            run.out
        );
    }

    #[test]
    fn at_five_awaiting_a_fix_job_starts_and_an_issue_does_not() {
        let f = fixture(Some(2));
        f.queue("fix.json", r#"{"kind": "fix", "pr": 150}"#, 1);
        let gh = awaiting(&f, github(&[(1, &[])]), 5);
        let ended = Cell::new(false);
        let play = Play::new(&f.base())
            .job("fix 150", 400, Kind::Ready)
            .on_job_end(|_| ended.set(true));
        let stop = || ended.get();

        let run = run_loop(&f, &play, &gh, &stop, &[1]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start fix 150 cloud"], "{}", run.out);
        assert!(
            run.out
                .contains("paused: 5 PRs await you and 0 tasks run, 5 in all"),
            "{}",
            run.out
        );
    }
}

// --- with `cloud_slots` unset, as before ---

mod one_at_a_time {
    use sbxm::backend::Location;
    use sbxm::github::Mergeable;

    use super::play::*;

    #[test]
    fn with_cloud_slots_unset_resolves_and_jobs_run_in_the_loops_process() {
        let f = fixture(None);
        loop_pr(&f.base(), 50, 150, Location::Local);
        loop_pr(&f.base(), 51, 151, Location::Local);
        let gh = github(&[]).with_pr(pr(150)).with_pr(pr(151));
        gh.set_mergeable(150, Mergeable::Conflicting);
        f.queue("fix-151.json", r#"{"kind": "fix", "pr": 151}"#, 1);
        let play = Play::new(&f.base());

        let run = run_loop(&f, &play, &gh, &never, &[]);

        run.result.unwrap();
        assert_eq!(
            play.events(),
            ["in-process fix 150", "in-process fix 151"],
            "{}",
            run.out
        );
    }
}

// --- review of issue 633 ---

mod review {
    use std::cell::Cell;

    use sbxm::backend::Location;
    use sbxm::task::outcome::Kind;

    use super::play::*;

    /// Loop PRs 150 and up awaiting the user, `n` of them, on `gh`.
    fn awaiting(
        f: &Fixture,
        gh: sbxm::github::fake::FakeGitHub,
        n: u32,
    ) -> sbxm::github::fake::FakeGitHub {
        let mut gh = gh;
        for i in 0..n {
            loop_pr(&f.base(), 50 + i, 150 + i, Location::Cloud);
            gh = gh.with_pr(pr(150 + i));
        }
        gh
    }

    // M-1: a queued job that may open a PR is admitted by the 5-PR pause too.

    #[test]
    fn at_four_awaiting_only_one_of_two_queued_runs_starts() {
        let f = fixture(Some(2));
        f.queue("run-8.json", r#"{"kind": "run", "issue": 8}"#, 1);
        f.queue("run-9.json", r#"{"kind": "run", "issue": 9}"#, 2);
        let gh = awaiting(&f, github(&[(8, &["run-cloud"]), (9, &["run-cloud"])]), 4)
            .with_pr(pr(108))
            .with_pr(pr(109))
            .with_auto_merge(108)
            .with_auto_merge(109);
        let play = Play::new(&f.base()).issue(8, 400).issue(9, 1);

        let run = run_loop(&f, &play, &gh, &never, &[8, 9]);

        run.result.unwrap();
        assert!(
            play.at("finish 8") < play.at("start 9 cloud"),
            "{}",
            run.out
        );
        assert!(
            run.out
                .contains("job run #9: waits for room: 4 PRs await you and 1 tasks run, 5 in all"),
            "{}",
            run.out
        );
    }

    #[test]
    fn at_five_awaiting_with_nothing_running_a_run_job_waits_and_a_fix_job_starts() {
        let f = fixture(Some(2));
        f.queue("run-9.json", r#"{"kind": "run", "issue": 9}"#, 1);
        f.queue("fix-150.json", r#"{"kind": "fix", "pr": 150}"#, 2);
        let gh = awaiting(&f, github(&[(9, &["run-cloud"])]), 5);
        let ended = Cell::new(false);
        let play = Play::new(&f.base())
            .job("fix 150", 5, Kind::Ready)
            .on_job_end(|_| ended.set(true));
        let stop = || ended.get();

        let run = run_loop(&f, &play, &gh, &stop, &[9]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start fix 150 cloud"], "{}", run.out);
        assert!(f.queued("run-9.json"), "{}", run.out);
    }

    // M-2: a queued run's child that wrote no outcome is interrupted, never run again.

    fn killed_run(killed: Killed) {
        let f = fixture(Some(2));
        f.queue("run-9.json", r#"{"kind": "run", "issue": 9}"#, 1);
        let gh = github(&[(9, &["run-cloud"])]).with_pr(pr(109));
        let play = Play::new(&f.base()).issue_ending(9, 2, killed);

        let run = run_loop(&f, &play, &gh, &never, &[9]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start 9 cloud"], "{}", run.out);
        assert!(!play.has("finish 9"), "{}", run.out);
        assert!(
            run.out.contains(
                "job run #9: interrupted: the task's child sbxm was killed and wrote no \
                 outcome; not run again"
            ),
            "{}",
            run.out
        );
    }

    #[test]
    fn a_queued_run_killed_before_its_record_is_interrupted_and_not_run_again() {
        killed_run(Killed::BeforeRecord);
    }

    #[test]
    fn a_queued_run_killed_after_its_record_is_interrupted_and_not_run_again() {
        killed_run(Killed::AfterRecord);
    }

    // M-3: an issue running as a queued run's task is never started again as new work.

    fn queued_and_ready(issues: &[u32]) {
        let f = fixture(Some(2));
        f.queue("run-1.json", r#"{"kind": "run", "issue": 1}"#, 1);
        let gh = github(&[(1, &["run-cloud"])]).with_pr(pr(101));
        let play = Play::new(&f.base()).issue(1, 5);

        let run = run_loop(&f, &play, &gh, &never, issues);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start 1 cloud"], "{}", run.out);
        assert!(run.out.contains("job run #1: done"), "{}", run.out);
        assert!(!f.log_text().contains("| #1 Fix 1 |"), "{}", f.log_text());
    }

    #[test]
    fn a_queued_runs_issue_is_not_picked_again_from_the_labeled_queue() {
        queued_and_ready(&[]);
    }

    #[test]
    fn a_queued_runs_issue_is_not_picked_again_from_the_issue_list() {
        queued_and_ready(&[1]);
    }

    #[test]
    fn a_queued_run_of_an_issue_running_as_a_task_waits_for_it_and_never_starts_it_twice() {
        let f = fixture(Some(2));
        let gh = github(&[(1, &["run-cloud"])]).with_pr(pr(101));
        let play = Play::new(&f.base()).issue(1, 400).on_issue(|issue| {
            if issue == 1 {
                f.queue("run-1.json", r#"{"kind": "run", "issue": 1}"#, 1);
            }
        });

        let run = run_loop(&f, &play, &gh, &never, &[1]);

        run.result.unwrap();
        assert_eq!(starts(&play), ["start 1 cloud"], "{}", run.out);
        assert!(
            run.out
                .contains("job run #1: waits for #1's running task; tried again at a later pass"),
            "{}",
            run.out
        );
    }
}
