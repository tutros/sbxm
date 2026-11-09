//! Issue 511 (decision 307): `sbxm task loop --check` says for each issue whether the loop would
//! take it and prints the loop command, running nothing; and the loop itself passes over an
//! issue whose body still has a `## Open points` heading.

mod common;

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text, open_issue};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, Issue, IssueText, PrInfo, PrState};
use sbxm::task::queue::{self, LockFile, Progress, has_open_points};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const OPEN_POINTS: &str = "Why\n\n## Open points\n\n1. A or B?\n\n**Depends on:** none.\n";
const ANSWERED: &str = "Why\n\n## Answered (the user)\n\n1. A.\n\n**Depends on:** none.\n";

/// Records the phases the loop asks for; `task run` fails before any record, so the loop moves on.
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

fn opts(f: &Fixture, issues: &[u32]) -> Options {
    Options {
        repo_root: f.repo_root.clone(),
        issues: issues.to_vec(),
        cap: None,
        repo: Some(REPO.into()),
    }
}

fn github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for issue in &issues {
        gh = gh.with_issue_text(issue_text(issue.number));
    }
    gh.with_open_issues(issues)
}

/// Runs the loop to its stop; its output.
fn run_loop(f: &Fixture, opts: &Options, script: &Script, gh: &FakeGitHub) -> (Result<()>, String) {
    let env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let result = task_loop::run(
        &f.env.config_dir(),
        opts,
        script,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

// --- the `## Open points` heading ---

#[test]
fn an_open_points_heading_is_found() {
    assert!(has_open_points("Why\n\n## Open points\n\n1. A or B?\n"));
    assert!(has_open_points("## Open points\n"));
    assert!(has_open_points("## Open points  \r\n- a?\n"));
}

#[test]
fn an_answered_heading_and_no_open_points_is_not_open() {
    assert!(!has_open_points("Why\n\n## Answered (the user)\n\n1. A.\n"));
    assert!(!has_open_points(""));
}

#[test]
fn open_points_in_a_code_block_or_not_at_a_line_start_do_not_count() {
    assert!(!has_open_points(
        "Example:\n\n```md\n## Open points\n```\n\n## Answered\n"
    ));
    assert!(!has_open_points("~~~\n## Open points\n~~~\n"));
    assert!(!has_open_points(
        "readied issues rename `## Open points` to `## Answered`\n"
    ));
    assert!(!has_open_points("  see ## Open points\n"));
    assert!(!has_open_points("### Open points\n"));
    // After a closed fence, a heading counts again.
    assert!(has_open_points("```\nx\n```\n## Open points\n"));
}

/// Review M-1: a real heading after a four-backtick example that holds a shorter fence.
const AFTER_NESTED_FENCE: &str =
    "````md\n```rust\nlet x = 1;\n````\n\n## Open points\n1. Which option?\n";
/// Review M-1: a quoted heading inside a four-backtick example that holds a shorter fence.
const INSIDE_NESTED_FENCE: &str = "````md\n```md\n## Open points\n```\n````\n\n## Answered\n";

#[test]
fn a_shorter_fence_or_the_other_character_does_not_close_an_open_block() {
    assert!(has_open_points(AFTER_NESTED_FENCE));
    assert!(!has_open_points(INSIDE_NESTED_FENCE));
    // Tildes inside a backtick block, and backticks inside a tilde block, are content.
    assert!(!has_open_points("```\n~~~\n## Open points\n~~~\n```\n"));
    assert!(!has_open_points("~~~~\n```\n## Open points\n```\n~~~~\n"));
    assert!(has_open_points("~~~\n```\n~~~\n## Open points\n"));
    // A closing fence takes nothing after it: `````rust` inside a block opens nothing.
    assert!(!has_open_points("```\n```rust\n## Open points\n```\n"));
    // A longer closing fence closes it.
    assert!(has_open_points("```\nx\n`````\n## Open points\n"));
}

// --- the loop passes over an issue with open points ---

#[test]
fn the_loop_passes_over_a_labeled_issue_with_open_points_and_reports_it() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &[READY, "size:S"], OPEN_POINTS),
        open_issue(7, &[READY, "size:S"], ANSWERED),
    ]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[]), &script, &gh);

    result.unwrap();
    assert_eq!(*script.calls.borrow(), ["run 7"]);
    assert!(
        out.contains("skipped #3 Fix 3: open points not answered"),
        "{out}"
    );
    // The empty queue's report names it with the other passed-over issues.
    let report = out.split("queue is empty").nth(1).unwrap_or_default();
    assert!(
        report.contains("passed over:\n  #3 Fix 3: open points not answered"),
        "{out}"
    );
}

#[test]
fn the_loop_passes_over_a_named_issue_with_open_points() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &["size:S"], OPEN_POINTS)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[3]), &script, &gh);

    result.unwrap();
    assert!(script.calls.borrow().is_empty());
    assert!(
        out.contains("skipped #3 Fix 3: open points not answered"),
        "{out}"
    );
}

// --- a branch on origin, through `gh` ---

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    outputs: Mutex<Vec<String>>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        self.outputs
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow!("no scripted output left"))
    }
}

fn gh_answering(outputs: &[&str]) -> (GhBackend, &'static Scripted) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
    }));
    (GhBackend::with_runner(Box::new(script)), script)
}

#[test]
fn a_branch_on_origin_is_found_by_its_exact_ref() {
    // The shape of `gh api repos/<repo>/git/matching-refs/heads/<branch>` (written by hand, no
    // GitHub access where issue 511 was built): every ref the name starts, so only an exact
    // match counts.
    let (gh, script) = gh_answering(&[
        r#"[{"ref":"refs/heads/issue-468","object":{"sha":"abc","type":"commit"}}]"#,
        r#"[{"ref":"refs/heads/issue-4680","object":{"sha":"abc","type":"commit"}}]"#,
        "[]",
    ]);

    assert!(gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-468").unwrap());
    assert_eq!(
        script.calls.lock().unwrap()[0],
        ["api", "repos/o/r/git/matching-refs/heads/issue-468"]
    );
}

#[test]
fn a_bad_branch_name_is_refused_before_gh_runs() {
    let (gh, script) = gh_answering(&[]);
    assert!(gh.branch_exists(REPO, "../x").is_err());
    assert!(script.calls.lock().unwrap().is_empty());
}

#[test]
fn the_fake_has_the_branches_a_test_scripted() {
    let gh = FakeGitHub::default().with_branch_head("issue-468", "abc");
    assert!(gh.branch_exists(REPO, "issue-468").unwrap());
    assert!(!gh.branch_exists(REPO, "issue-469").unwrap());
    assert_eq!(
        gh.calls()[0],
        GhCall::BranchExists(REPO.into(), "issue-468".into())
    );
}

// --- `task loop --check` ---

/// Runs `task loop --check`; whether every issue would be taken, and its output.
fn check(f: &Fixture, issues: &[u32], gh: &FakeGitHub) -> (Result<bool>, String) {
    let mut out = Vec::new();
    let result = task_loop::check(&f.env.config_dir(), &opts(f, issues), gh, &Probe, &mut out);
    (result, String::from_utf8(out).unwrap())
}

/// Every file and folder under `dir`, for "nothing was written".
fn tree(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    let mut todo = vec![dir.to_owned()];
    while let Some(d) = todo.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path.clone());
            }
            all.push(path);
        }
    }
    all.sort();
    all
}

/// The calls that only read: anything else would be a GitHub write.
fn only_reads(gh: &FakeGitHub) {
    for call in gh.calls() {
        assert!(
            matches!(
                call,
                GhCall::Whoami
                    | GhCall::DefaultBranch(_)
                    | GhCall::IssuesOpen(_)
                    | GhCall::IssuesOpenLabeled(..)
                    | GhCall::Issue(..)
                    | GhCall::Pr(..)
                    | GhCall::PrMergeState(..)
                    | GhCall::BranchHead(..)
                    | GhCall::BranchExists(..)
            ),
            "a GitHub write: {call:?}"
        );
    }
}

fn existing_task(f: &Fixture, issue: u32) {
    let rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: issue,
            repo: REPO,
            title: &format!("Fix {issue}"),
            base: "main",
            branch: &format!("issue-{issue}"),
            config_hash: "h",
            id: None,
        },
        1_793_613_600,
        Process::new(1, 0),
    );
    record::write(
        &record::task_dir(&f.env.base_dir(), &format!("issue-{issue}")),
        &rec,
    )
    .unwrap();
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

const SIZED: [&str; 2] = [READY, "size:S"];

/// One issue for each reason the loop passes over, plus one it takes (#3) and the open #11 that
/// #10 depends on.
fn every_reason(f: &Fixture) -> FakeGitHub {
    existing_task(f, 12);
    let merged_pr = IssueText {
        number: 13,
        title: "Fix 13".into(),
        state: "OPEN".into(),
        text: "title:\tFix 13\nstate:\tOPEN\n--\nPR: #50\n\nWhy\n".into(),
    };
    let issues = vec![
        open_issue(3, &SIZED, ANSWERED),
        open_issue(10, &SIZED, "**Depends on:** #11\n"),
        open_issue(11, &["size:M"], ""),
        open_issue(12, &SIZED, ""),
        open_issue(13, &SIZED, "PR: #50\n\nWhy\n"),
        open_issue(14, &SIZED, ""),
        open_issue(15, &["size:S"], ""),
        open_issue(16, &[READY], ""),
        open_issue(17, &SIZED, OPEN_POINTS),
    ];
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_pr(pr(50, PrState::Merged))
        .with_branch_head("issue-14", "abc")
        .with_issue_text(merged_pr);
    for issue in issues.iter().filter(|i| i.number != 13) {
        gh = gh.with_issue_text(issue_text(issue.number));
    }
    gh.with_open_issues(issues)
}

#[test]
fn check_says_take_for_a_ready_issue_and_prints_its_command() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &SIZED, ANSWERED)]);

    let (result, out) = check(&f, &[3], &gh);

    assert!(result.unwrap(), "{out}");
    assert_eq!(out, "#3 Fix 3: take\ncommand: sbxm task loop --issue 3\n");
}

#[test]
fn check_says_skip_with_the_reason_for_each_kind_of_issue_the_loop_passes_over() {
    let f = fixture();
    let gh = every_reason(&f);

    let (result, out) = check(&f, &[3, 10, 12, 13, 14, 15, 16, 17], &gh);

    assert!(!result.unwrap(), "exit 1 when one would be skipped: {out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "#3 Fix 3: take", "{out}");
    assert_eq!(
        lines[1],
        "#10 Fix 10: skip: depends on #11 (an open issue), not merged"
    );
    assert!(
        lines[2].starts_with("#12 Fix 12: skip: refused before any write: already has a task"),
        "{out}"
    );
    assert!(
        lines[3].starts_with("#13 Fix 13: skip: refused before any write: issue #13 belongs to PR #50, which is merged"),
        "{out}"
    );
    assert!(
        lines[4].starts_with("#14 Fix 14: skip: branch issue-14 is already on origin"),
        "{out}"
    );
    assert_eq!(lines[5], "#15 Fix 15: skip: not labeled ready-for-worker");
    assert_eq!(
        lines[6],
        "#16 Fix 16: skip: no size label (size:S, size:M or size:L)"
    );
    assert_eq!(lines[7], "#17 Fix 17: skip: open points not answered");
    assert_eq!(lines[8], "command: sbxm task loop --issue 3");
    assert_eq!(lines.len(), 9, "{out}");
}

#[test]
fn check_gives_every_reason_an_issue_has() {
    let f = fixture();
    let gh = github(vec![open_issue(5, &[], OPEN_POINTS)]);

    let (result, out) = check(&f, &[5], &gh);

    assert!(!result.unwrap());
    assert!(
        out.starts_with(
            "#5 Fix 5: skip: not labeled ready-for-worker; also: no size label (size:S, size:M \
             or size:L); also: open points not answered\n"
        ),
        "{out}"
    );
    assert!(
        out.ends_with("command: none, no issue would be taken\n"),
        "{out}"
    );
}

#[test]
fn check_takes_an_issue_whose_open_points_heading_is_only_quoted() {
    let f = fixture();
    let body = "Example:\n\n```md\n## Open points\n```\n\nRename `## Open points` when readied.\n";
    let gh = github(vec![open_issue(3, &SIZED, body)]);

    let (result, out) = check(&f, &[3], &gh);

    assert!(result.unwrap(), "{out}");
    assert!(out.starts_with("#3 Fix 3: take\n"), "{out}");
}

#[test]
fn check_skips_a_real_heading_after_a_nested_fence_and_takes_one_inside_it() {
    let f = fixture();
    let gh = github(vec![
        open_issue(3, &SIZED, AFTER_NESTED_FENCE),
        open_issue(4, &SIZED, INSIDE_NESTED_FENCE),
    ]);

    let (result, out) = check(&f, &[3], &gh);
    assert!(!result.unwrap(), "{out}");
    assert!(
        out.starts_with("#3 Fix 3: skip: open points not answered\n"),
        "{out}"
    );

    let (result, out) = check(&f, &[4], &gh);
    assert!(result.unwrap(), "{out}");
    assert!(out.starts_with("#4 Fix 4: take\n"), "{out}");
}

#[test]
fn the_loop_never_runs_an_issue_whose_heading_follows_a_nested_fence() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &SIZED, AFTER_NESTED_FENCE)]);
    let script = Script::default();

    let (result, out) = run_loop(&f, &opts(&f, &[3]), &script, &gh);

    result.unwrap();
    assert!(script.calls.borrow().is_empty(), "{out}");
    assert!(
        out.contains("skipped #3 Fix 3: open points not answered"),
        "{out}"
    );
}

#[test]
fn check_names_an_issue_that_is_not_open() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &SIZED, "")]);

    let (result, out) = check(&f, &[3, 9], &gh);

    assert!(!result.unwrap());
    assert!(out.contains("#9: skip: isn't an open issue\n"), "{out}");
    assert!(
        out.ends_with("command: sbxm task loop --issue 3\n"),
        "{out}"
    );
}

#[test]
fn check_gives_an_issue_named_twice_one_verdict() {
    let f = fixture();
    let gh = github(vec![open_issue(3, &SIZED, ""), open_issue(4, &SIZED, "")]);

    let (result, out) = check(&f, &[3, 4, 3], &gh);

    assert!(result.unwrap(), "{out}");
    assert_eq!(
        out,
        "#3 Fix 3: take\n#4 Fix 4: take\ncommand: sbxm task loop --issue 3 --issue 4\n"
    );
}

#[test]
fn check_refuses_issue_zero_before_reading_anything() {
    let f = fixture();
    let gh = github(Vec::new());

    let (result, _) = check(&f, &[0], &gh);

    assert!(result.is_err());
    assert!(gh.calls().is_empty());
}

#[test]
fn check_without_issues_takes_the_labeled_queue_in_the_loop_s_order() {
    let f = fixture();
    let gh = github(vec![
        open_issue(9, &[READY, "size:M"], ""),
        open_issue(7, &SIZED, ""),
        open_issue(4, &[READY], ""),
        open_issue(2, &["size:S"], ""),
        open_issue(8, &SIZED, OPEN_POINTS),
    ]);

    let (result, out) = check(&f, &[], &gh);

    assert!(!result.unwrap(), "{out}");
    assert_eq!(
        out,
        "#7 Fix 7: take\n\
         #8 Fix 8: skip: open points not answered\n\
         #9 Fix 9: take\n\
         #4 Fix 4: skip: no size label (size:S, size:M or size:L)\n\
         command: sbxm task loop --issue 7 --issue 9\n"
    );
}

#[test]
fn check_of_an_empty_queue_says_so_and_passes() {
    let f = fixture();
    let gh = github(vec![open_issue(2, &["size:S"], "")]);

    let (result, out) = check(&f, &[], &gh);

    assert!(result.unwrap(), "{out}");
    assert_eq!(
        out,
        "the queue is empty: no open issue is labeled ready-for-worker; nothing to run\n"
    );
}

/// A loop started with `--issue 3 --issue 7`, on its second task, as its lock says.
fn plant_loop_lock(f: &Fixture) {
    let lock = LockFile {
        process: Process::new(4242, 0),
        issues: vec![3, 7],
        cap: None,
        progress: Some(Progress {
            counted: 1,
            current: Some(7),
            outcomes: vec![queue::IssueOutcome {
                issue: 3,
                outcome: queue::Outcome::Done,
            }],
        }),
    };
    let path = queue::lock_path(&f.env.base_dir());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_string(&lock).unwrap()).unwrap();
}

#[test]
fn check_while_a_loop_runs_names_it_first_and_still_gives_every_verdict() {
    let f = fixture();
    plant_loop_lock(&f);
    let gh = github(vec![
        open_issue(5, &SIZED, ""),
        open_issue(6, &["size:S"], ""),
    ]);
    let lock = fs::read_to_string(queue::lock_path(&f.env.base_dir())).unwrap();

    let (result, out) = check(&f, &[5, 6], &gh);

    assert!(!result.unwrap(), "exit codes unchanged: {out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "a loop is running: process 4242, task 2 of 2");
    assert_eq!(lines[1], "#5 Fix 5: take");
    assert_eq!(lines[2], "#6 Fix 6: skip: not labeled ready-for-worker");
    assert_eq!(lines[3], "command: sbxm task loop --issue 5");
    assert!(
        lines[4].contains("a second loop is refused until the running one (process 4242) ends"),
        "{out}"
    );
    // The running loop's lock is left as it was.
    assert_eq!(
        fs::read_to_string(queue::lock_path(&f.env.base_dir())).unwrap(),
        lock
    );
}

#[test]
fn check_while_a_loop_runs_still_passes_when_every_issue_would_be_taken() {
    let f = fixture();
    plant_loop_lock(&f);
    let gh = github(vec![open_issue(5, &SIZED, "")]);

    let (result, out) = check(&f, &[5], &gh);

    assert!(result.unwrap(), "{out}");
    assert!(out.starts_with("a loop is running: "), "{out}");
}

#[test]
fn check_writes_nothing_and_runs_nothing() {
    let f = fixture();
    let gh = every_reason(&f);
    let before = tree(f.env.tmp.path());

    let (result, out) = check(&f, &[3, 10, 12, 13, 14, 15, 16, 17], &gh);
    let (queue_result, _) = check(&f, &[], &gh);

    assert!(!result.unwrap(), "{out}");
    assert!(!queue_result.unwrap());
    // No task folder, no lock, no slot, no log row: not a file more or less anywhere.
    assert_eq!(tree(f.env.tmp.path()), before);
    assert!(!queue::lock_path(&f.env.base_dir()).exists());
    only_reads(&gh);
}

// --- the command line ---

fn sbxm(f: &Fixture) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::cargo_bin("sbxm").unwrap();
    cmd.current_dir(&f.repo_root)
        .env("SBXM_CONFIG_DIR", f.env.config_dir());
    cmd
}

#[test]
fn check_is_a_flag_of_task_loop() {
    let f = fixture();
    let output = sbxm(&f).args(["task", "loop", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{help}");
    assert!(help.contains("--check"), "{help}");
}

#[test]
fn check_runs_from_the_command_line_and_refuses_issue_zero() {
    let f = fixture();
    let output = sbxm(&f)
        .args(["task", "loop", "--check", "--issue", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("issue numbers start at 1"), "{stderr}");
}

#[test]
fn check_takes_neither_cap_nor_clear_pause() {
    let f = fixture();
    for extra in ["--cap=2", "--clear-pause"] {
        let output = sbxm(&f)
            .args(["task", "loop", "--check", extra])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{extra}");
    }
    assert!(!queue::lock_path(&f.env.base_dir()).exists());
}
