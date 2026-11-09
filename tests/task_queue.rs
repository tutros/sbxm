//! Issue 529: `sbxm task queue <kind> ...` writes one job file to `<base>/.sbxm/queue/`, after the
//! checks the loop makes when it takes the job, refusing at once with the reason and writing
//! nothing. The file is `<UTC time>-<kind>-<target>.json`, written as `.json.tmp` and renamed,
//! and the loop's own parser reads it back as the same job. A job of the same kind and target
//! waiting in `queue/` or `queue/running/` is refused.

mod common;

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Steps};
use sbxm::commands::task_queue::{self, Options};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, PrInfo, PrState};
use sbxm::task::jobs::{self, Job, JobHarness};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const START: u64 = 1_793_613_600;
const STAMP: &str = "20261102T100000Z";
const ANSWERED: &str = "Why\n\n## Answered (the user)\n\n1. A.\n\n**Depends on:** none.\n";

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
    let specs = env.tmp.path().join("specs");
    fs::create_dir_all(specs.join("sub")).unwrap();
    fs::write(specs.join("retro.md"), "# Retro\n").unwrap();
    fs::write(specs.join("sub").join("x.md"), "# X\n").unwrap();
    let value = toml::Value::String(specs.to_str().unwrap().to_owned());
    env.set_config(&format!("specs_dir = {value}"));
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
            repo: Some(REPO.into()),
        }
    }

    /// Queues `job` at [`START`], the clock moving one second per sleep: the path written, the
    /// output and the sleeps.
    fn add(&self, job: &Job, gh: &FakeGitHub) -> (Result<PathBuf>, String, u32) {
        let clock = Cell::new(START);
        let sleeps = Cell::new(0);
        let mut out = Vec::new();
        let result = task_queue::add(
            &self.env.config_dir(),
            &self.opts(),
            job,
            gh,
            &|| clock.get(),
            &|d: Duration| {
                sleeps.set(sleeps.get() + 1);
                clock.set(clock.get() + d.as_secs());
            },
            &mut out,
        );
        (result, String::from_utf8(out).unwrap(), sleeps.get())
    }

    /// The names of the files in `dir`, sorted; none when it is missing.
    fn names(&self, dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Writes job file `name` in `dir` with `text`.
    fn put(&self, dir: &Path, name: &str, text: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}

/// A task record of `kind` for `number` of `repo`, whose PR is `pr`.
fn write_task(base: &Path, kind: Kind, number: u32, repo: &str, pr: u32) {
    let id = match kind {
        Kind::Issue => format!("issue-{number}"),
        _ => format!("pr-{number}"),
    };
    let dir = record::task_dir(base, &id);
    let mut rec = Record::new(
        &NewTask {
            kind,
            number,
            repo,
            title: &format!("Fix {number}"),
            base: "main",
            branch: &format!("sbxm/{id}"),
            config_hash: "h",
            id: None,
        },
        START,
        Process::new(1, 0),
    );
    rec.pr = Some(format!("https://github.com/{repo}/pull/{pr}"));
    record::write(&dir, &rec).unwrap();
}

fn pr(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: "0123456789abcdef0123456789abcdef01234567".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// GitHub with the open issues `issues` and the PRs `prs`.
fn github(issues: Vec<Issue>, prs: &[(u32, PrState)]) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for issue in &issues {
        gh = gh.with_issue_text(issue_text(issue.number));
    }
    for &(n, state) in prs {
        gh = gh.with_pr(pr(n, state));
    }
    gh.with_open_issues(issues)
}

fn ready(number: u32) -> Issue {
    open_issue(number, &[READY, "size:S"], ANSWERED)
}

/// GitHub where #5 and #6 would be taken and PR #40, which issue 4's task opened, is open.
fn every_kind(f: &Fixture) -> FakeGitHub {
    write_task(&f.base(), Kind::Issue, 4, REPO, 40);
    github(vec![ready(5), ready(6)], &[(40, PrState::Open)])
}

fn spec(file: &str) -> Job {
    Job::Spec { file: file.into() }
}

fn template(profile: &str, harness: JobHarness, warm: bool) -> Job {
    Job::Template {
        profile: profile.into(),
        harness,
        warm,
    }
}

// --- each kind round-trips through the loop's parser ---

#[test]
fn each_kind_writes_a_file_the_loops_parser_reads_back_as_the_same_job() {
    let cases = [
        (Job::Run { issue: 5 }, "run-5"),
        (Job::Review { pr: 40 }, "review-40"),
        (
            Job::Fix {
                pr: 40,
                check_run: None,
            },
            "fix-40",
        ),
        (
            Job::Fix {
                pr: 40,
                check_run: Some(77),
            },
            "fix-40",
        ),
        (Job::RmMerged {}, "rm-merged"),
        (spec("retro.md"), "spec-retro"),
        (
            template("sbxm-dev", JobHarness::Claude, false),
            "template-sbxm-dev-claude",
        ),
        (
            template("sbxm-dev", JobHarness::Pi, true),
            "template-sbxm-dev-pi",
        ),
    ];
    for (job, target) in cases {
        let f = fixture();
        let gh = every_kind(&f);

        let (result, out, sleeps) = f.add(&job, &gh);

        let path = result.unwrap_or_else(|e| panic!("{job:?}: {e:#}"));
        assert_eq!(path, f.queue().join(format!("{STAMP}-{target}.json")));
        assert_eq!(sleeps, 0);
        assert!(out.contains(&path.display().to_string()), "{out}");
        assert_eq!(jobs::parse(&fs::read_to_string(&path).unwrap()), Ok(job));
        // The loop sees it, and nothing else is left in the queue folder.
        assert_eq!(
            jobs::pending(&f.base()).unwrap(),
            std::slice::from_ref(&path)
        );
        assert_eq!(f.names(&f.queue()), [format!("{STAMP}-{target}.json")]);
    }
}

#[test]
fn a_missing_queue_folder_is_created() {
    let f = fixture();
    assert!(!f.base().join(".sbxm").exists());

    let (result, _, _) = f.add(&Job::RmMerged {}, &github(Vec::new(), &[]));

    result.unwrap();
    assert_eq!(f.names(&f.queue()), [format!("{STAMP}-rm-merged.json")]);
}

// --- run: `task loop --check`'s verdict ---

/// Queues `run --issue <number>` with `gh`; it must be refused: its error.
fn refused(f: &Fixture, job: &Job, gh: &FakeGitHub) -> String {
    let (result, out, _) = f.add(job, gh);
    let e = format!("{:#}", result.expect_err(&out));
    assert!(f.names(&f.queue()).is_empty(), "{e}: wrote a file");
    e
}

#[test]
fn a_run_the_check_would_skip_is_refused_with_its_reason() {
    let f = fixture();
    let gh = github(
        vec![
            open_issue(3, &["size:S"], ANSWERED),
            open_issue(7, &[READY, "size:S"], "## Open points\n\n1. A or B?\n"),
            open_issue(10, &[READY, "size:S"], "**Depends on:** #11\n"),
            open_issue(11, &["size:M"], ""),
            ready(14),
        ],
        &[],
    )
    .with_branch_head("issue-14", "abc");
    let cases = [
        (3, "not labeled ready-for-worker"),
        (7, "open points not answered"),
        (10, "depends on #11 (an open issue), not merged"),
        (14, "branch issue-14 is already on origin"),
        (99, "#99 is not an open issue of o/r"),
    ];
    for (issue, reason) in cases {
        let e = refused(&f, &Job::Run { issue }, &gh);
        assert!(e.contains(reason), "#{issue}: {e}");
    }
}

// --- review and fix: a PR sbxm opened, still open ---

#[test]
fn a_review_or_fix_of_a_pr_no_task_of_this_repo_opened_is_refused() {
    let f = fixture();
    write_task(&f.base(), Kind::Issue, 8, "other/repo", 41);
    let gh = github(Vec::new(), &[(41, PrState::Open), (42, PrState::Open)]);
    for job in [
        Job::Review { pr: 41 },
        Job::Fix {
            pr: 42,
            check_run: None,
        },
    ] {
        let e = refused(&f, &job, &gh);
        assert!(e.contains("no task record of o/r names PR"), "{e}");
    }
}

#[test]
fn a_review_or_fix_of_a_merged_or_closed_pr_is_refused() {
    let f = fixture();
    write_task(&f.base(), Kind::Issue, 4, REPO, 40);
    write_task(&f.base(), Kind::Issue, 5, REPO, 50);
    let gh = github(Vec::new(), &[(40, PrState::Merged), (50, PrState::Closed)]);
    let e = refused(&f, &Job::Review { pr: 40 }, &gh);
    assert!(e.contains("PR #40 is merged"), "{e}");
    let e = refused(
        &f,
        &Job::Fix {
            pr: 50,
            check_run: Some(3),
        },
        &gh,
    );
    assert!(e.contains("PR #50 is closed"), "{e}");
}

// --- spec and template ---

#[test]
fn a_spec_outside_specs_dir_or_missing_is_refused() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let e = refused(&f, &spec("sub/x.md"), &gh);
    assert!(e.contains("path separator"), "{e}");
    let e = refused(&f, &spec("gone.md"), &gh);
    assert!(e.contains("cannot be read"), "{e}");
    let e = refused(&f, &spec("sub"), &gh);
    assert!(e.contains("isn't a plain file"), "{e}");
}

#[test]
fn a_template_of_a_profile_without_a_template_table_or_unknown_is_refused() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let e = refused(&f, &template("default", JobHarness::Claude, false), &gh);
    assert!(e.contains("has no [template] table"), "{e}");
    let e = refused(&f, &template("nope", JobHarness::Codex, false), &gh);
    assert!(e.contains("nope"), "{e}");
}

#[test]
fn a_job_outside_the_parsers_shape_is_refused() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let e = refused(&f, &Job::Run { issue: 0 }, &gh);
    assert!(e.contains("issue numbers start at 1"), "{e}");
}

// --- duplicates ---

#[test]
fn the_same_job_waiting_in_the_queue_is_refused_naming_its_file() {
    let f = fixture();
    let gh = every_kind(&f);
    let waiting = f.put(&f.queue(), "by-hand.json", r#"{"kind": "run", "issue": 5}"#);

    let e = refused_besides(&f, &Job::Run { issue: 5 }, &gh, &["by-hand.json"]);

    assert!(e.contains(&waiting.display().to_string()), "{e}");
}

/// [`refused`] where the queue folder already holds `names`.
fn refused_besides(f: &Fixture, job: &Job, gh: &FakeGitHub, names: &[&str]) -> String {
    let (result, out, _) = f.add(job, gh);
    let e = format!("{:#}", result.expect_err(&out));
    assert_eq!(f.names(&f.queue()), names, "{e}");
    e
}

#[test]
fn the_same_job_running_is_refused_naming_its_file() {
    let f = fixture();
    let gh = every_kind(&f);
    // As the loop marks a job it took.
    let running = f.put(
        &jobs::running_dir(&f.base()),
        "x.json",
        r#"{"job": {"kind": "review", "pr": 40}, "started_at": "2026-11-02T09:00:00Z"}"#,
    );

    let e = refused_besides(&f, &Job::Review { pr: 40 }, &gh, &["running"]);

    assert!(e.contains(&running.display().to_string()), "{e}");
}

#[test]
fn a_fix_with_another_check_run_is_the_same_target() {
    let f = fixture();
    let gh = every_kind(&f);
    f.put(&f.queue(), "a.json", r#"{"kind": "fix", "pr": 40}"#);

    let job = Job::Fix {
        pr: 40,
        check_run: Some(9),
    };
    refused_besides(&f, &job, &gh, &["a.json"]);
}

#[test]
fn a_different_target_or_kind_is_written() {
    let f = fixture();
    let gh = every_kind(&f);
    f.put(&f.queue(), "a.json", r#"{"kind": "run", "issue": 6}"#);
    f.put(&f.queue(), "b.json", r#"{"kind": "review", "pr": 40}"#);
    f.put(&f.queue(), "c.json", "not a job");

    for job in [
        Job::Run { issue: 5 },
        Job::Fix {
            pr: 40,
            check_run: None,
        },
    ] {
        let (result, out, _) = f.add(&job, &gh);
        result.unwrap_or_else(|e| panic!("{e:#}: {out}"));
    }
    assert_eq!(f.names(&f.queue()).len(), 5);
}

#[test]
fn a_second_rm_merged_is_refused() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let (first, _, _) = f.add(&Job::RmMerged {}, &gh);
    let first = first.unwrap();

    let e = refused_besides(
        &f,
        &Job::RmMerged {},
        &gh,
        &[&format!("{STAMP}-rm-merged.json")],
    );

    assert!(e.contains(&first.display().to_string()), "{e}");
}

#[test]
fn a_second_template_of_the_same_profile_and_harness_is_refused_another_harness_written() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    f.put(
        &f.queue(),
        "a.json",
        r#"{"kind": "template", "profile": "sbxm-dev", "warm": true}"#,
    );
    refused_besides(
        &f,
        &template("sbxm-dev", JobHarness::Claude, false),
        &gh,
        &["a.json"],
    );
    let (result, _, _) = f.add(&template("sbxm-dev", JobHarness::Codex, false), &gh);
    result.unwrap();
}

// --- the file's name ---

#[test]
fn a_name_taken_in_the_same_second_waits_for_the_next() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    // Another job under the name this one would take.
    f.put(
        &f.queue(),
        &format!("{STAMP}-rm-merged.json"),
        r#"{"kind": "run", "issue": 9}"#,
    );

    let (result, _, sleeps) = f.add(&Job::RmMerged {}, &gh);

    assert_eq!(sleeps, 1);
    assert_eq!(
        result.unwrap(),
        f.queue().join("20261102T100001Z-rm-merged.json")
    );
}

#[test]
fn a_name_taken_five_seconds_in_a_row_is_refused_and_leaves_nothing() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let mut names = Vec::new();
    for s in 0..5 {
        let name = format!("20261102T10000{s}Z-rm-merged.json");
        f.put(&f.queue(), &name, r#"{"kind": "run", "issue": 9}"#);
        names.push(name);
    }

    let (result, out, _) = f.add(&Job::RmMerged {}, &gh);

    let e = format!("{:#}", result.expect_err(&out));
    assert!(e.contains("taken"), "{e}");
    assert_eq!(f.names(&f.queue()), names);
}

#[test]
fn a_name_taken_in_running_counts_as_taken() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    f.put(
        &jobs::running_dir(&f.base()),
        &format!("{STAMP}-rm-merged.json"),
        r#"{"kind": "run", "issue": 9}"#,
    );

    let (result, _, sleeps) = f.add(&Job::RmMerged {}, &gh);

    assert_eq!(sleeps, 1);
    assert_eq!(
        result.unwrap(),
        f.queue().join("20261102T100001Z-rm-merged.json")
    );
}

#[test]
fn a_write_that_fails_leaves_no_tmp_file() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    // A folder where the temporary file goes: its write fails.
    let name = format!("{STAMP}-rm-merged.json");
    fs::create_dir_all(f.queue()).unwrap();
    let tmp = f.queue().join(format!("{name}.tmp"));
    fs::create_dir_all(&tmp).unwrap();

    let (result, out, _) = f.add(&Job::RmMerged {}, &gh);

    let e = format!("{:#}", result.expect_err(&out));
    assert!(e.contains("cannot write"), "{e}");
    // The folder in the way is not sbxm's to remove; no file was left.
    assert_eq!(f.names(&f.queue()), [format!("{name}.tmp")]);
    assert!(tmp.is_dir());
}

#[test]
fn a_temporary_file_someone_else_is_writing_is_never_overwritten() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let name = format!("{STAMP}-rm-merged.json");
    let tmp = f.put(&f.queue(), &format!("{name}.tmp"), "half a job");

    let (result, out, _) = f.add(&Job::RmMerged {}, &gh);

    let e = format!("{:#}", result.expect_err(&out));
    assert!(e.contains("cannot write"), "{e}");
    assert_eq!(fs::read_to_string(&tmp).unwrap(), "half a job");
    assert_eq!(f.names(&f.queue()), [format!("{name}.tmp")]);
}

#[cfg(unix)]
#[test]
fn a_linked_sbxm_folder_is_refused_before_anything_is_written() {
    let f = fixture();
    let gh = github(Vec::new(), &[]);
    let elsewhere = f.env.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.base().join(".sbxm")).unwrap();

    let (result, out, _) = f.add(&Job::RmMerged {}, &gh);

    let e = format!("{:#}", result.expect_err(&out));
    assert!(e.contains("symlink"), "{e}");
    assert!(f.names(&elsewhere).is_empty());
}

// --- the loop takes it ---

/// The loop's phases: `task run` fails before any record, so the loop moves on.
#[derive(Default)]
struct Script {
    calls: RefCell<Vec<String>>,
}

impl Steps for Script {
    fn run(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.calls.borrow_mut().push(format!("run {issue}"));
        Err(anyhow!("scripted refusal"))
    }
    fn resume(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of #{issue}"))
    }
    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of #{issue}"))
    }
    fn file_findings(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected file-findings of #{issue}"))
    }
    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }
    fn fix_check(
        &self,
        pr: u32,
        _: u64,
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

#[test]
fn a_queued_run_is_taken_by_the_loop() {
    let f = fixture();
    // Not labeled any more by the time the loop runs: only the job brings #5 in.
    let gh = github(vec![ready(5)], &[]);
    let (result, _, _) = f.add(&Job::Run { issue: 5 }, &gh);
    let path = result.unwrap();
    let name = path.file_name().unwrap().to_owned();

    let script = Script::default();
    let env = Env {
        now: &|| START + 60,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let _ = task_loop::run(
        &f.env.config_dir(),
        &task_loop::Options {
            repo_root: f.repo_root.clone(),
            issues: vec![999],
            cap: None,
            repo: Some(REPO.into()),
        },
        &script,
        &gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );

    let out = String::from_utf8(out).unwrap();
    assert_eq!(script.calls.borrow()[0], "run 5", "{out}");
    assert!(out.contains("job run #5: started"), "{out}");
    assert!(!path.exists());
    assert!(jobs::done_dir(&f.base()).join(name).is_file(), "{out}");
}

// --- the command line ---

#[test]
fn the_command_writes_the_job_and_prints_its_path() {
    let f = fixture();

    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", f.env.config_dir())
        .args(["task", "queue", "rm-merged"])
        .assert()
        .success()
        .get_output()
        .clone();

    let names = f.names(&f.queue());
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with("Z-rm-merged.json"), "{names:?}");
    let path = f.queue().join(&names[0]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&path.display().to_string()), "{stdout}");
    assert_eq!(
        jobs::parse(&fs::read_to_string(&path).unwrap()),
        Ok(Job::RmMerged {})
    );
}

#[test]
fn the_command_refuses_a_harness_template_build_does_not_take() {
    let f = fixture();
    assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", f.env.config_dir())
        .args([
            "task",
            "queue",
            "template",
            "sbxm-dev",
            "--harness",
            "antigravity",
        ])
        .assert()
        .failure();
    assert!(f.names(&f.queue()).is_empty());
}
