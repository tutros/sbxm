//! Issue 633 (milestone 5 slice 5, decisions 317(7), (8), 321): with `[resources] cloud_slots`
//! set, the loop's resolves, log-fixes and queued jobs run as child `sbxm` processes it supervises
//! next to its task children, one per PR at a time, ahead of new issues; `rm-merged` stays in the
//! loop's process.

mod common;

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
