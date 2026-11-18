//! Issue 711: an in-process job's phase (`Loop::phase`, spec and template jobs) finds a lapsed
//! sign-in with `outcome.rs`'s classifier, the one a child's `outcome.json` is written with. A
//! spec job whose `task run` only prints that a harness is signed out stops the loop with exit 3
//! and leaves the next job queued.

use crate::common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use common::Env as TestEnv;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::jobs;
use serde_json::Value;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// Plays the loop's phases: a spec's `task run` prints a harness's lapsed sign-in and fails.
struct Script {
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of {issue}"))
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of {issue}"))
    }

    fn run_spec(&self, spec: &Path, out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        let name = spec.file_name().unwrap().to_string_lossy();
        self.calls.borrow_mut().push(format!("spec {name}"));
        writeln!(
            out,
            "worker failed: OAuth session expired and could not be refreshed"
        )?;
        bail!("the task's run failed")
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
        Err(anyhow!("unexpected fix of {pr}"))
    }

    fn fix_check(
        &self,
        pr: u32,
        _run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected fix of {pr}"))
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

/// Writes job file `name` in `queue` with `text`, modified `age` seconds before [`START`].
fn queue_job(queue: &Path, name: &str, text: &str, age: u64) -> PathBuf {
    fs::create_dir_all(queue).unwrap();
    let path = queue.join(name);
    fs::write(&path, text).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(START - age))
        .unwrap();
    path
}

#[test]
fn task_loop_job_sign_in_printed_by_a_spec_run_stops_the_loop_with_exit_3() {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    env.write_profile("sbxm-dev", "[template]\nallow = []\n");
    let specs = env.tmp.path().join("specs");
    fs::create_dir_all(&specs).unwrap();
    fs::write(specs.join("retro.md"), "# Retro\n").unwrap();
    let value = toml::Value::String(specs.to_str().unwrap().to_owned());
    env.set_config(&format!("specs_dir = {value}"));
    let base = env.base_dir();
    let queue = jobs::queue_dir(&base);
    queue_job(
        &queue,
        "s.json",
        r#"{"kind": "spec", "file": "retro.md"}"#,
        20,
    );
    let later = queue_job(
        &queue,
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        10,
    );

    let script = Script {
        calls: RefCell::default(),
        clock: Cell::new(START),
    };
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let stop = || false;
    let loop_env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let opts = Options {
        repo_root,
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &env.config_dir(),
        &opts,
        &script,
        &gh,
        &common::task_fixture::Probe,
        &loop_env,
        &mut out,
        &mut warn,
    );

    assert_eq!(task_loop::NEEDS_USER, 3);
    assert_eq!(task_loop::exit_code(&result), task_loop::NEEDS_USER);
    let e = result.expect_err("the loop goes on after a lapsed sign-in");
    assert!(format!("{e:#}").contains("sign"), "{e:#}");
    assert_eq!(*script.calls.borrow(), ["spec retro.md"]);
    let path = jobs::done_dir(&base).join("s.json");
    let text = fs::read_to_string(&path).unwrap();
    let job: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(job["outcome"], "stopped", "{job}");
    assert!(later.is_file(), "the later job was taken");
}
