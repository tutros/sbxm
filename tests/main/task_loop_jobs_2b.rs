//! Issue 524 (slice 2b of #476, decision 309): two more job kinds for `sbxm task loop`'s queue
//! folder. `{"kind": "template", "profile": P}` (optional `harness` and `warm`) runs `template
//! build`; `{"kind": "spec", "file": F}` runs `task run --spec <specs_dir>/F` and finishes it as
//! the loop finishes its issues. Every shape outside them is refused with a result file and runs
//! nothing. The phases are scripted; the PRs are `FakeGitHub`'s.

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
use sbxm::github::{PrInfo, PrState};
use sbxm::harness::Harness;
use sbxm::task::jobs::{self, Job, JobHarness};
use sbxm::task::pipeline::{HarnessSignedOut, MissingSecret, NotSignedIn};
use sbxm::task::queue;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;
use serde_json::Value;

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const READY: &str = "ready-for-worker";
/// The PR a spec task's finish opens.
const SPEC_PR: u32 = 700;

const CLEAN: &str = "Must-fix findings: 0\n";
const S_ONE: &str = "Must-fix findings: 0\n\n## Should fix\n\n\
    ### S-1 - a.txt:2 a small slip.\n\n**Where:** a.txt:2\n**Size:** S (one line)\n";

/// How a spec task's `task run` ends.
#[derive(Clone, Copy)]
enum SpecEnd {
    /// Ready, 0 must-fix, at this risk.
    Clean(Level),
    /// Ready with must-fix findings left.
    MustFix,
    /// Short of ready: its gates failed.
    Short,
}

/// Plays the loop's phases, recording each call; the phases named in `fail` fail.
struct Script {
    base: PathBuf,
    calls: RefCell<Vec<String>>,
    clock: Cell<u64>,
    fail: Vec<String>,
    spec: SpecEnd,
    /// The review a spec task's run leaves.
    review: &'static str,
    /// The phases that fail with a lapsed sign-in, and how it shows.
    signed_out: Vec<(String, SignedOut)>,
    /// The phases that fail on a missing secret.
    secret_missing: Vec<String>,
}

/// How a phase's lapsed sign-in shows (issue 445).
#[derive(Clone, Copy)]
enum SignedOut {
    /// `sbx`'s, as a typed error.
    Sbx,
    /// A harness's, as a typed error.
    Harness,
    /// A harness's, only in the phase's output.
    Printed,
}

impl Script {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            calls: RefCell::default(),
            clock: Cell::new(START),
            fail: Vec::new(),
            spec: SpecEnd::Clean(Level::Low),
            review: CLEAN,
            signed_out: Vec::new(),
            secret_missing: Vec::new(),
        }
    }

    fn signing_out(mut self, call: &str, how: SignedOut) -> Self {
        self.signed_out.push((call.to_owned(), how));
        self
    }

    /// [`Self::call`] for a phase that can fail with a lapsed sign-in, saying it on `out`.
    fn call_out(&self, call: String, out: &mut dyn Write) -> Result<()> {
        let how = self
            .signed_out
            .iter()
            .find(|(f, _)| call.starts_with(f.as_str()))
            .map(|(_, how)| *how);
        let Some(how) = how else {
            return self.call(call);
        };
        self.calls.borrow_mut().push(call.clone());
        match how {
            SignedOut::Sbx => Err(NotSignedIn::new("Not authenticated to Docker").into()),
            SignedOut::Harness => Err(HarnessSignedOut("OAuth session expired".into()).into()),
            SignedOut::Printed => {
                writeln!(
                    out,
                    "worker failed: OAuth session expired and could not be refreshed"
                )?;
                bail!("{call} failed")
            }
        }
    }

    fn missing_secret(mut self, call: &str) -> Self {
        self.secret_missing.push(call.to_owned());
        self
    }

    fn failing(mut self, call: &str) -> Self {
        self.fail.push(call.to_owned());
        self
    }

    fn spec_ends(mut self, end: SpecEnd) -> Self {
        self.spec = end;
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
        if self
            .secret_missing
            .iter()
            .any(|f| call.starts_with(f.as_str()))
        {
            return Err(MissingSecret::new("the secret `anthropic` is not set").into());
        }
        if failing {
            bail!("{call} failed");
        }
        Ok(())
    }
}

/// A task record of `kind` in `base`, ready, with a clean review at `risk`.
fn write_task(base: &Path, kind: Kind, number: u32, id: &str, risk: Level) -> PathBuf {
    let dir = record::task_dir(base, id);
    let mut rec = Record::new(
        &NewTask {
            kind,
            number,
            repo: REPO,
            title: &format!("Task {id}"),
            base: "main",
            branch: &format!("sbxm/{id}"),
            config_hash: "h",
            id: Some(id),
        },
        START,
        Process::new(1, 0),
    );
    rec.stage = Stage::Ready;
    rec.status = Status::Ok;
    rec.review = Some(ReviewResult {
        round: 1,
        commit: Some(HEAD.to_owned()),
        must_fix: 0,
        risk,
        not_run: Some(Vec::new()),
        ..ReviewResult::default()
    });
    record::write(&dir, &rec).unwrap();
    dir
}

fn spec_id(spec: &Path) -> String {
    record::spec_id(spec).unwrap().0
}

fn file_name(spec: &Path) -> String {
    spec.file_name().unwrap().to_string_lossy().into_owned()
}

/// Sets `rec`'s stage to finished, with PR `pr`.
fn finished(dir: &Path, pr: u32) {
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
    record::write(dir, &rec).unwrap();
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("run {issue}"))?;
        write_task(
            &self.base,
            Kind::Issue,
            issue,
            &format!("issue-{issue}"),
            Level::Low,
        );
        Ok(())
    }

    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("finish {issue}"))?;
        finished(
            &record::task_dir(&self.base, &format!("issue-{issue}")),
            100 + issue,
        );
        Ok(())
    }

    fn run_spec(&self, spec: &Path, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("spec {}", file_name(spec)))?;
        let risk = match self.spec {
            SpecEnd::Clean(risk) => risk,
            _ => Level::Low,
        };
        let dir = write_task(&self.base, Kind::Spec, 0, &spec_id(spec), risk);
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        match self.spec {
            SpecEnd::Clean(_) => {}
            SpecEnd::MustFix => rec.review.as_mut().unwrap().must_fix = 2,
            SpecEnd::Short => {
                rec.stage = Stage::Gating;
                rec.status = Status::GatesFailed;
            }
        }
        record::write(&dir, &rec).unwrap();
        fs::write(dir.join("review.md"), self.review).unwrap();
        Ok(())
    }

    fn should_fix_spec(
        &self,
        spec: &Path,
        out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        self.call_out(format!("should-fix spec {}", file_name(spec)), out)?;
        let dir = record::task_dir(&self.base, &spec_id(spec));
        let mut rec = record::read(&dir.join("task.json")).unwrap();
        rec.should_fix_round = true;
        record::write(&dir, &rec).unwrap();
        fs::write(dir.join("review.md"), CLEAN).unwrap();
        Ok(())
    }

    fn finish_spec(&self, spec: &Path, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.call(format!("finish spec {}", file_name(spec)))?;
        finished(&record::task_dir(&self.base, &spec_id(spec)), SPEC_PR);
        Ok(())
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
        out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<String> {
        let warm = warm.map_or_else(|| "cold".to_owned(), |p| format!("warm {}", p.display()));
        self.call_out(
            format!("template {profile} {} {warm}", harness.as_str()),
            out,
        )?;
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
    fixture_with("")
}

/// A fixture whose `sbxm-task.toml` ends with `extra`.
fn fixture_with(extra: &str) -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        format!("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n{extra}"),
    )
    .unwrap();
    env.write_profile("sbxm-dev", "[template]\nallow = []\n");
    Fixture { env, repo_root }
}

/// A fixture with `specs_dir` set and the spec file `retro.md` in it.
fn spec_fixture(extra: &str) -> Fixture {
    let f = fixture_with(extra);
    let specs = f.specs();
    fs::create_dir_all(&specs).unwrap();
    fs::write(specs.join("retro.md"), "# Retro\n").unwrap();
    let value = toml::Value::String(specs.to_str().unwrap().to_owned());
    f.env.set_config(&format!("specs_dir = {value}"));
    f
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn specs(&self) -> PathBuf {
        self.env.tmp.path().join("specs")
    }

    /// `retro.md`'s path as the loop resolves it, and its task's record.
    fn retro(&self) -> (PathBuf, Option<Record>) {
        let spec = self.specs().join("retro.md");
        let rec =
            record::read(&record::task_dir(&self.base(), &spec_id(&spec)).join("task.json")).ok();
        (spec, rec)
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
            wait: false,
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
    let script = Script::new(&f.base());
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
    let script = Script::new(&f.base());
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
    let script = Script::new(&f.base());
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
    let script = Script::new(&f.base()).failing("template sbxm-dev");
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

// --- The spec job ---

fn spec_job(file: &str) -> String {
    serde_json::json!({"kind": "spec", "file": file}).to_string()
}

#[test]
fn a_spec_job_parses_with_a_plain_file_name() {
    assert_eq!(
        jobs::parse(&spec_job("retro-2026-11-08.md")),
        Ok(Job::Spec {
            file: "retro-2026-11-08.md".into()
        })
    );
}

#[test]
fn a_spec_job_whose_file_is_not_a_plain_name_is_refused() {
    let bs = char::from(92);
    for file in [
        "sub/x.md".to_owned(),
        format!("sub{bs}x.md"),
        format!("..{bs}x.md"),
        "../x.md".to_owned(),
        "..".to_owned(),
        "C:x.md".to_owned(),
        format!("I:{bs}x.md"),
        String::new(),
        "x\n.md".to_owned(),
    ] {
        let parsed = jobs::parse(&spec_job(&file));
        assert!(parsed.is_err(), "{file:?} parsed");
        assert!(parsed.unwrap_err().contains("not a job"), "{file:?}");
    }
    for text in [
        r#"{"kind": "spec"}"#,
        r#"{"kind": "spec", "file": 3}"#,
        r#"{"kind": "spec", "file": "x.md", "path": "H:/x.md"}"#,
    ] {
        assert!(jobs::parse(text).is_err(), "{text} parsed");
    }
}

#[test]
fn every_refused_spec_file_name_gets_a_result_file_and_runs_nothing() {
    let f = spec_fixture("");
    let bs = char::from(92);
    let names = [
        "sub/x.md".to_owned(),
        format!("sub{bs}x.md"),
        format!("..{bs}x.md"),
        "C:x.md".to_owned(),
        format!("I:{bs}x.md"),
        String::new(),
    ];
    for (i, file) in names.iter().enumerate() {
        f.queue_job(&format!("s{i}.json"), &spec_job(file), 100 - i as u64);
    }
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    for i in 0..names.len() {
        let name = format!("s{i}.json");
        assert_eq!(f.result(&name)["outcome"], "refused", "{name}");
    }
}

#[test]
fn a_spec_job_of_a_file_not_in_specs_dir_is_refused() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("nope.md"), 10);
    fs::create_dir_all(f.specs().join("sub")).unwrap();
    f.queue_job("d.json", &spec_job("sub"), 5);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert_eq!(f.result("s.json")["outcome"], "refused");
    assert!(
        f.reason("s.json").contains("nope.md"),
        "{}",
        f.reason("s.json")
    );
    assert_eq!(f.result("d.json")["outcome"], "refused");
    assert!(
        f.reason("d.json").contains("isn't a plain file"),
        "{}",
        f.reason("d.json")
    );
}

#[test]
fn a_spec_job_without_specs_dir_is_refused_naming_the_setting() {
    let f = fixture();
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    assert_eq!(f.result("s.json")["outcome"], "refused");
    assert!(
        f.reason("s.json").contains("specs_dir"),
        "{}",
        f.reason("s.json")
    );
}

#[test]
fn a_clean_low_risk_spec_task_is_finished_and_armed() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base());
    let gh = github(&[SPEC_PR]);
    let run = run_loop(&f, &script, &gh);
    run.result.unwrap();
    assert_eq!(script.calls(), ["spec retro.md", "finish spec retro.md"]);
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "done", "{result}");
    let (_, rec) = f.retro();
    let rec = rec.unwrap();
    assert_eq!(result["task"], rec.id.as_str());
    assert_eq!(
        result["url"],
        format!("https://github.com/{REPO}/pull/{SPEC_PR}")
    );
    assert!(result["next"].is_null(), "{result}");
    assert!(f.reason("s.json").contains("auto-merge armed"), "{result}");
    assert!(
        gh.calls()
            .iter()
            .any(|c| format!("{c:?}").contains("PrArmAutoMerge")),
        "{:?}",
        gh.calls()
    );
    assert!(queue::opened_by_loop(&rec));
}

#[test]
fn a_clean_medium_risk_spec_task_is_finished_not_armed_and_counts_toward_the_cap() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base()).spec_ends(SpecEnd::Clean(Level::Medium));
    let gh = github(&[SPEC_PR]);
    run_loop(&f, &script, &gh).result.unwrap();
    assert_eq!(script.calls(), ["spec retro.md", "finish spec retro.md"]);
    assert_eq!(f.result("s.json")["outcome"], "done");
    assert!(
        !gh.calls()
            .iter()
            .any(|c| format!("{c:?}").contains("PrArmAutoMerge")),
        "{:?}",
        gh.calls()
    );
    let prs = queue::loop_prs(&f.base(), REPO).unwrap();
    assert!(prs.iter().any(|p| p.number == Some(SPEC_PR)), "{prs:?}");
}

#[test]
fn a_spec_task_ready_with_must_fix_left_is_stopped_not_finished() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base()).spec_ends(SpecEnd::MustFix);
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert_eq!(script.calls(), ["spec retro.md"]);
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "stopped", "{result}");
    assert!(f.reason("s.json").contains("2 must-fix"), "{result}");
    let (spec, rec) = f.retro();
    assert_eq!(rec.unwrap().stage, Stage::Ready);
    assert_eq!(
        result["next"],
        format!("sbxm task finish --spec {}", spec.display())
    );
}

#[test]
fn a_spec_task_short_of_ready_is_stopped_naming_its_resume() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base()).spec_ends(SpecEnd::Short);
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert_eq!(script.calls(), ["spec retro.md"]);
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "stopped", "{result}");
    let (spec, _) = f.retro();
    assert_eq!(
        result["next"],
        format!("sbxm task resume --spec {}", spec.display())
    );
}

#[test]
fn a_spec_task_whose_run_fails_is_failed_and_the_next_job_runs() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 20);
    f.queue_job(
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        10,
    );
    let script = Script::new(&f.base()).failing("spec retro.md");
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert_eq!(
        script.calls(),
        ["spec retro.md", "template sbxm-dev claude cold"]
    );
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "failed", "{result}");
    assert!(f.reason("s.json").contains("failed"), "{result}");
}

#[test]
fn a_spec_task_whose_finish_fails_is_failed_naming_its_finish() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let script = Script::new(&f.base()).failing("finish spec");
    run_loop(&f, &script, &github(&[])).result.unwrap();
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "failed", "{result}");
    let (spec, _) = f.retro();
    assert_eq!(
        result["next"],
        format!("sbxm task finish --spec {}", spec.display())
    );
}

#[test]
fn a_clean_spec_task_with_small_should_fix_findings_gets_its_round_first() {
    let f = spec_fixture("\n[loop]\nshould_fix = \"fix\"\n");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let mut script = Script::new(&f.base());
    script.review = S_ONE;
    run_loop(&f, &script, &github(&[SPEC_PR])).result.unwrap();
    assert_eq!(
        script.calls(),
        [
            "spec retro.md",
            "should-fix spec retro.md",
            "finish spec retro.md"
        ]
    );
    assert_eq!(f.result("s.json")["outcome"], "done");
}

#[test]
fn without_should_fix_on_a_clean_spec_task_goes_straight_to_its_finish() {
    let f = spec_fixture("");
    f.queue_job("s.json", &spec_job("retro.md"), 10);
    let mut script = Script::new(&f.base());
    script.review = S_ONE;
    run_loop(&f, &script, &github(&[SPEC_PR])).result.unwrap();
    assert_eq!(script.calls(), ["spec retro.md", "finish spec retro.md"]);
}

// --- Order ---

#[test]
fn a_spec_and_a_template_job_run_before_the_next_issue_oldest_first() {
    let f = spec_fixture("");
    f.queue_job(
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        50,
    );
    f.queue_job("s.json", &spec_job("retro.md"), 100);
    let gh = github(&[SPEC_PR, 621])
        .with_issue_text(common::task_fixture::issue_text(521))
        .with_open_issues(vec![common::task_fixture::open_issue(
            521,
            &[READY, "size:S"],
            "",
        )]);
    let script = Script::new(&f.base());
    run_loop(&f, &script, &gh).result.unwrap();
    assert_eq!(
        script.calls()[..4],
        [
            "spec retro.md",
            "finish spec retro.md",
            "template sbxm-dev claude cold",
            "run 521"
        ]
    );
    assert_eq!(f.result("s.json")["outcome"], "done");
    assert_eq!(f.result("t.json")["outcome"], "done");
}

// --- Interrupted ---

#[test]
fn an_interrupted_spec_job_names_the_resume_of_its_real_spec_path() {
    let f = spec_fixture("");
    let (spec, _) = f.retro();
    let id = spec_id(&spec);
    let dir = write_task(&f.base(), Kind::Spec, 0, &id, Level::Low);
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Working;
    rec.status = Status::Running;
    record::write(&dir, &rec).unwrap();
    let running = jobs::running_dir(&f.base());
    fs::create_dir_all(&running).unwrap();
    let text = serde_json::json!({
        "job": {"kind": "spec", "file": "retro.md"},
        "started_at": "2026-11-09T10:00:00Z",
        "task": id,
    });
    fs::write(running.join("s.json"), text.to_string()).unwrap();
    let script = Script::new(&f.base());
    run_loop(&f, &script, &github(&[])).result.unwrap();
    assert!(script.calls().is_empty(), "{:?}", script.calls());
    let result = f.result("s.json");
    assert_eq!(result["outcome"], "interrupted", "{result}");
    assert_eq!(
        result["next"],
        format!("sbxm task resume --spec {}", spec.display())
    );
}

// --- A lapsed sign-in stops the loop (issue 445), in every phase of a queued job ---

#[test]
fn a_lapsed_sign_in_in_a_spec_should_fix_round_stops_the_loop_leaving_the_next_job_queued() {
    for how in [SignedOut::Sbx, SignedOut::Harness, SignedOut::Printed] {
        let f = spec_fixture("\n[loop]\nshould_fix = \"fix\"\n");
        f.queue_job("s.json", &spec_job("retro.md"), 20);
        let later = f.queue_job(
            "t.json",
            r#"{"kind": "template", "profile": "sbxm-dev"}"#,
            10,
        );
        let mut script = Script::new(&f.base()).signing_out("should-fix spec", how);
        script.review = S_ONE;
        let run = run_loop(&f, &script, &github(&[SPEC_PR]));
        // A stop that needs the user (decision 310).
        assert_eq!(task_loop::exit_code(&run.result), task_loop::NEEDS_USER);
        let e = run
            .result
            .expect_err("the loop goes on after a lapsed sign-in");
        assert_eq!(
            script.calls(),
            ["spec retro.md", "should-fix spec retro.md"]
        );
        let result = f.result("s.json");
        assert_eq!(result["outcome"], "stopped", "{result}");
        let (spec, _) = f.retro();
        let resume = format!("sbxm task resume --spec {}", spec.display());
        assert!(format!("{e:#}").contains(&resume), "{e:#}");
        assert!(f.reason("s.json").contains(&resume), "{result}");
        assert!(later.is_file(), "the later job was taken");
    }
}

// --- A missing secret in a queued job stops the loop as one that needs the user (decision 310) ---

#[test]
fn a_missing_secret_in_a_template_build_stops_the_loop_as_needing_the_user() {
    let f = fixture();
    f.queue_job(
        "a.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        20,
    );
    let script = Script::new(&f.base()).missing_secret("template sbxm-dev");
    let run = run_loop(&f, &script, &github(&[]));
    assert_eq!(task_loop::exit_code(&run.result), task_loop::NEEDS_USER);
    assert_eq!(f.result("a.json")["outcome"], "failed");
}

#[test]
fn an_ordinary_should_fix_failure_is_failed_and_the_next_job_runs() {
    let f = spec_fixture("\n[loop]\nshould_fix = \"fix\"\n");
    f.queue_job("s.json", &spec_job("retro.md"), 20);
    f.queue_job(
        "t.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        10,
    );
    let mut script = Script::new(&f.base()).failing("should-fix spec");
    script.review = S_ONE;
    run_loop(&f, &script, &github(&[SPEC_PR])).result.unwrap();
    assert_eq!(
        script.calls(),
        [
            "spec retro.md",
            "should-fix spec retro.md",
            "template sbxm-dev claude cold"
        ]
    );
    assert_eq!(f.result("s.json")["outcome"], "failed");
    assert_eq!(f.result("t.json")["outcome"], "done");
}

#[test]
fn a_lapsed_sign_in_in_a_template_build_stops_the_loop_leaving_the_next_job_queued() {
    let f = fixture();
    f.env.write_profile("other", "[template]\nallow = []\n");
    f.queue_job(
        "a.json",
        r#"{"kind": "template", "profile": "sbxm-dev"}"#,
        20,
    );
    let later = f.queue_job("b.json", r#"{"kind": "template", "profile": "other"}"#, 10);
    let script = Script::new(&f.base()).signing_out("template sbxm-dev", SignedOut::Sbx);
    let run = run_loop(&f, &script, &github(&[]));
    assert_eq!(task_loop::exit_code(&run.result), task_loop::NEEDS_USER);
    let e = run
        .result
        .expect_err("the loop goes on after a lapsed sign-in");
    assert!(format!("{e:#}").contains("sbx login"), "{e:#}");
    assert_eq!(script.calls(), ["template sbxm-dev claude cold"]);
    assert_eq!(f.result("a.json")["outcome"], "stopped");
    assert!(later.is_file(), "the later job was taken");
}
