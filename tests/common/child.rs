//! Issue 612: the loop's children, played in tests. [`InProcess`] runs a child's `task run` or
//! `task resume` in this process with the test's fakes, on a thread of its own while the caller's
//! thread polls as the real [`ChildSpawner`](sbxm::commands::task_loop::child::ChildSpawner) does,
//! and writes its outcome where the real child would. [`NoChild`] starts nothing.
#![allow(dead_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use sbxm::backend::{Location, SandboxBackend};
use sbxm::commands::task_loop::child::{self, Child, Exit, Spawner};
use sbxm::commands::{task_resume, task_run};
use sbxm::github::GitHubBackend;
use sbxm::task::gates::HostRunner;
use sbxm::task::outcome;
use sbxm::task::record::ProcessProbe;

/// Starts nothing: a child that exits 1 at once, writing no outcome.
#[derive(Default)]
pub struct NoChild {
    pub started: RefCell<Vec<Child>>,
}

impl Spawner for NoChild {
    fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
        self.started.borrow_mut().push(child.clone());
        Ok(Exit {
            success: false,
            how: "exited with code 1".into(),
            errors: String::new(),
        })
    }

    fn stop(&self) {}
}

/// Runs each child's command here, with these fakes.
pub struct InProcess<'a> {
    fakes: Fakes<'a>,
    pub started: RefCell<Vec<Child>>,
}

/// What a child's command runs with.
struct Fakes<'a> {
    config_dir: PathBuf,
    backend: &'a (dyn SandboxBackend + Sync),
    github: &'a (dyn GitHubBackend + Sync),
    probe: &'a (dyn ProcessProbe + Sync),
    host: &'a (dyn HostRunner + Sync),
    disk: sbxm::disk::Guard,
}

impl<'a> InProcess<'a> {
    pub fn new(
        config_dir: PathBuf,
        backend: &'a (dyn SandboxBackend + Sync),
        github: &'a (dyn GitHubBackend + Sync),
        probe: &'a (dyn ProcessProbe + Sync),
        host: &'a (dyn HostRunner + Sync),
        disk: sbxm::disk::Guard,
    ) -> Self {
        Self {
            fakes: Fakes {
                config_dir,
                backend,
                github,
                probe,
                host,
                disk,
            },
            started: RefCell::default(),
        }
    }

    /// The arguments of each child started.
    pub fn args(&self) -> Vec<Vec<String>> {
        self.started
            .borrow()
            .iter()
            .map(|c| c.args.clone())
            .collect()
    }
}

impl Fakes<'_> {
    fn command(&self, child: &Child, out: &mut Vec<u8>, warn: &mut Vec<u8>) -> Result<()> {
        let env = |key: &str| {
            child
                .env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| PathBuf::from(v))
        };
        let disk = child::obeying_loop_stop(self.disk.clone(), env(child::STOP_FILE_ENV));
        let elsewhere = env(outcome::LOOP_FILE_ENV);
        let args: Vec<&str> = child.args.iter().map(String::as_str).collect();
        outcome::recorded_with(elsewhere, out, warn, |_, out, warn| match args.as_slice() {
            ["task", "run", "--issue", n, rest @ ..] => {
                let mut location = None;
                let mut repo = None;
                let mut rest = rest.iter();
                while let Some(arg) = rest.next() {
                    match *arg {
                        "--cloud" => location = Some(Location::Cloud),
                        "--local" => location = Some(Location::Local),
                        "--repo" => repo = rest.next().map(|r| (*r).to_owned()),
                        other => bail!("unexpected argument {other}"),
                    }
                }
                task_run::run(
                    &self.config_dir,
                    &task_run::Options {
                        repo_root: child.dir.clone(),
                        issue: n.parse()?,
                        worker_harness: None,
                        worker_model: None,
                        reviewer_harness: None,
                        reviewer_model: None,
                        time_limit: None,
                        reviewer_time_limit: None,
                        profile: None,
                        base: None,
                        repo,
                        clone_source: None,
                        identity: None,
                        disk: disk.clone(),
                        location,
                    },
                    None,
                    self.backend,
                    self.github,
                    self.probe,
                    self.host,
                    out,
                    warn,
                )
            }
            ["task", "resume", "--issue", n, rest @ ..] => {
                let should_fix = rest == ["--rounds", "1", "--should-fix"];
                if !should_fix && !rest.is_empty() {
                    bail!("unexpected arguments {rest:?}");
                }
                task_resume::run(
                    &self.config_dir,
                    &task_resume::Options {
                        repo_root: child.dir.clone(),
                        target: task_resume::Target::Issue(n.parse()?),
                        rounds: should_fix.then_some(1),
                        should_fix,
                        disk: disk.clone(),
                        identity: None,
                        forget_ambiguous: false,
                    },
                    self.backend,
                    self.github,
                    self.probe,
                    self.host,
                    out,
                    warn,
                )
            }
            other => bail!("unexpected child {other:?}"),
        })
    }
}

impl Spawner for InProcess<'_> {
    fn run(&self, child: &Child, tick: &dyn Fn()) -> Result<Exit> {
        self.started.borrow_mut().push(child.clone());
        let fakes = &self.fakes;
        let (result, out) = std::thread::scope(|scope| {
            let running = scope.spawn(move || {
                let (mut out, mut warn) = (Vec::new(), Vec::new());
                let result = fakes.command(child, &mut out, &mut warn);
                out.extend(warn);
                (result.map_err(|e| format!("{e:#}")), out)
            });
            while !running.is_finished() {
                tick();
                std::thread::sleep(Duration::from_millis(5));
            }
            running.join().expect("the child's thread")
        });
        let _ = out;
        Ok(match result {
            Ok(()) => Exit {
                success: true,
                how: "exited with code 0".into(),
                errors: String::new(),
            },
            Err(e) => Exit {
                success: false,
                how: "exited with code 1".into(),
                errors: format!("Error: {e}\n"),
            },
        })
    }

    fn stop(&self) {}
}
