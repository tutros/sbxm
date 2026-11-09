//! Issue 524 (slice 2b of #476, decision 309): two more job kinds for `sbxm task loop`'s queue
//! folder. `{"kind": "template", "profile": P}` (optional `harness` and `warm`) runs `template
//! build`; `{"kind": "spec", "file": F}` runs `task run --spec <specs_dir>/F` and finishes it as
//! the loop finishes its issues. Every shape outside them is refused with a result file and runs
//! nothing. The phases are scripted; the PRs are `FakeGitHub`'s.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use common::Env as TestEnv;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::harness::Harness;
use sbxm::task::jobs::{self, Job, JobHarness};
use serde_json::Value;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// Plays the loop's phases, recording each call; the phases named in `fail` fail.
struct Script {
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    fail: Vec<String>,
}

impl Script {
    fn new() -> Self {
        Self {
            calls: RefCell::default(),
            clock: Cell::new(START),
            fail: Vec::new(),
        }
    }

    fn failing(mut self, call: &str) -> Self {
        self.fail.push(call.to_owned());
        self
    }

    /// Every call but the loop's own `rm --merged` before each step.
    fn calls(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| *c != "rm-merged")
            .cloned()
            .collect()
    }

    fn call(&self, call: String) -> Result<()> {
        self.clock.set(self.clock.get() + 60);
        let failing = self.fail.iter().any(|f| call.starts_with(f.as_str()));
        self.calls.borrow_mut().push(call.clone());
        if failing {
            bail!("{call} failed");
        }
        Ok(())
    }
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
        self.calls.borrow_mut().push("rm-merged".to_owned());
        Ok(())
    }

    fn template_build(
        &self,
        profile: &str,
        harness: Harness,
        warm: Option<&Path>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<String> {
        let warm = warm.map_or_else(|| "cold".to_owned(), |p| format!("warm {}", p.display()));
        self.call(format!("template {profile} {} {warm}", harness.as_str()))?;
        Ok(format!("sbxm-tpl-{profile}-{}:1", harness.as_str()))
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
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
    env.write_profile("sbxm-dev", "[template]\nallow = []\n");
    Fixture { env, repo_root }
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn queue(&self) -> PathBuf {
        jobs::queue_dir(&self.base())
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
        }
    }

    /// Writes job file `name` with `text`, modified `age` seconds before [`START`].
    fn queue_job(&self, name: &str, text: &str, age: u64) -> PathBuf {
        fs::create_dir_all(self.queue()).unwrap();
        let path = self.queue().join(name);
        fs::write(&path, text).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(START - age))
            .unwrap();
        path
    }

    /// The result file `done/<name>`, parsed.
    fn result(&self, name: &str) -> Value {
        let path = jobs::done_dir(&self.base()).join(name);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no result {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    fn reason(&self, name: &str) -> String {
        self.result(name)["reason"].as_str().unwrap().to_owned()
    }
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// GitHub with no open issue and the open PRs `prs`.
fn github(prs: &[u32]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    for &n in prs {
        gh = gh.with_pr(pr(n, PrState::Open));
    }
    gh
}

struct Run {
    result: Result<()>,
    out: String,
}

fn run_loop(f: &Fixture, script: &Script, gh: &FakeGitHub) -> Run {
    let now = || script.clock.get();
    let sleep = |d: Duration| script.clock.set(script.clock.get() + d.as_secs());
    let stop = || false;
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
        &common::task_fixture::Probe,
        &env,
        &mut out,
        &mut warn,
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

// --- The template job ---

#[test]
fn a_template_job_parses_with_its_defaults() {
    assert_eq!(
        jobs::parse(r#"{"kind": "template", "profile": "sbxm-dev"}"#),
        Ok(Job::Template {
            profile: "sbxm-dev".into(),
            harness: JobHarness::Claude,
            warm: false,
        })
    );
    assert_eq!(
        jobs::parse(
            r#"{"kind": "template", "profile": "sbxm-dev", "harness": "pi", "warm": true}"#
        ),
        Ok(Job::Template {
            profile: "sbxm-dev".into(),
            harness: JobHarness::Pi,
            warm: true,
        })
    );
    assert_eq!(
        jobs::parse(r#"{"kind": "template", "profile": "sbxm-dev", "harness": "codex"}"#),
        Ok(Job::Template {
            profile: "sbxm-dev".into(),
            harness: JobHarness::Codex,
            warm: false,
        })
    );
}

#[test]
fn a_template_job_outside_its_shape_is_refused() {
    for text in [
        r#"{"kind": "template", "profile": "sbxm-dev", "harness": "agy"}"#,
        r#"{"kind": "template", "profile": "sbxm-dev", "harness": "antigravity"}"#,
        r#"{"kind": "template", "profile": "sbxm-dev", "warm": "H:/repo"}"#,
        r#"{"kind": "template", "profile": "sbxm-dev", "ignore_disk": true}"#,
        r#"{"kind": "template", "profile": "sbxm-dev", "no_wait": true}"#,
        r#"{"kind": "template", "profile": "sbxm-dev", "repo": "H:/repo"}"#,
        r#"{"kind": "template"}"#,
        r#"{"kind": "template", "profile": 3}"#,
    ] {
        assert!(jobs::parse(text).is_err(), "{text} parsed");
    }
}

#[test]
fn a_template_job_of_a_profile_with_a_template_table_builds_and_names_its_tag() {
    let f = fixture();
    f.queue_job(
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        10,
    );
    let script = Script::new();
    let run = run_loop(&f, &script, &github(&[]));
    run.result.unwrap();
    assert_eq!(script.calls(), ["template sbxm-dev claude cold"]);
    let result = f.result("t.json");
    assert_eq!(result["outcome"], "done", "{result}");
    assert!(
        f.reason("t.json").contains("sbxm-tpl-sbxm-dev-claude:1"),
        "{result}"
    );
    assert!(
        run.out.contains("job template sbxm-dev: done"),
        "{}",
        run.out
    );
}

#[test]
fn a_warm_template_job_warms_from_the_loops_checkout_with_its_harness() {
    let f = fixture();
    f.queue_job(
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev", "harness": "pi", "warm": true}"#,
        10,
    );
    let script = Script::new();
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert_eq!(
        script.calls(),
        [format!(
            "template sbxm-dev pi warm {}",
            f.repo_root.display()
        )]
    );
    assert_eq!(f.result("t.json")["outcome"], "done");
}

#[test]
fn a_template_job_of_a_profile_without_a_template_table_or_missing_is_refused() {
    let f = fixture();
    f.queue_job(
        "plain.json",
        r#"{"kind": "template", "profile": "default"}"#,
        30,
    );
    f.queue_job(
        "missing.json",
        r#"{"kind": "template", "profile": "nope"}"#,
        20,
    );
    f.queue_job("bad.json", r#"{"kind": "template", "profile": "../x"}"#, 10);
    let script = Script::new();
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    for name in ["plain.json", "missing.json", "bad.json"] {
        assert_eq!(f.result(name)["outcome"], "refused", "{name}");
    }
    assert!(
        f.reason("plain.json").contains("no [template] table"),
        "{}",
        f.reason("plain.json")
    );
    assert!(
        f.reason("missing.json").contains("not found"),
        "{}",
        f.reason("missing.json")
    );
    assert!(
        f.reason("bad.json").contains("invalid profile name"),
        "{}",
        f.reason("bad.json")
    );
}

#[test]
fn a_template_build_that_fails_is_failed_and_the_next_job_runs() {
    let f = fixture();
    f.env.write_profile("other", "[template]\nallow = []\n");
    f.queue_job(
        "a.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        20,
    );
    f.queue_job("b.json", r#"{"kind": "template", "profile": "other"}"#, 10);
    let script = Script::new().failing("template sbxm-dev");
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert_eq!(
        script.calls(),
        [
            "template sbxm-dev claude cold",
            "template other claude cold"
        ]
    );
    assert_eq!(f.result("a.json")["outcome"], "failed");
    assert!(
        f.reason("a.json").contains("failed"),
        "{}",
        f.reason("a.json")
    );
    assert_eq!(f.result("b.json")["outcome"], "done");
}
