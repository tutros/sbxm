//! Issue 536: `sbxm status`'s queue (and a `--issue` loop's `pending:` list) says why the loop
//! would pass an issue over, with the text `task loop --check` gives for it (decision 307), read
//! by the same function inside status's GitHub read.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};

use common::Env as TestEnv;
use common::task_fixture::issue_text;
use sbxm::backend::FakeBackend;
use sbxm::commands::status::{self, Env, LoopSource};
use sbxm::commands::task_loop::{self, Options};
use sbxm::github::Issue;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::queue::{LockFile, lock_path};
use sbxm::task::record::{Process, ProcessProbe};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
const T0: u64 = 1_790_000_000;
const OPEN_POINTS: &str = "Why\n\n## Open points\n\n1. A or B?\n";

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        Some(T0 - 1_000)
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

fn issue(number: u32, title: &str, labels: &[&str], body: &str) -> Issue {
    Issue {
        number,
        open: true,
        title: title.into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        body: body.into(),
    }
}

/// GitHub with these open issues, each one's text scripted (what `task run`'s checks read).
fn github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main");
    for i in &issues {
        gh = gh.with_issue_text(issue_text(i.number));
    }
    gh.with_open_issues(issues)
}

fn env(terminal: bool, width: usize) -> Env {
    Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width,
        iterations: None,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    }
}

/// The Loop section of one `status` frame, up to the disk footer.
fn section(base: &Path, gh: &FakeGitHub, terminal: bool, width: usize) -> String {
    let source = LoopSource {
        github: gh,
        repo: Ok(REPO.into()),
    };
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        &Probe,
        &env(terminal, width),
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    let loop_part = text.split("\nLoop:").nth(1).expect("a Loop section");
    format!(
        "Loop:{}",
        loop_part.split("\nFree space:").next().unwrap_or_default()
    )
}

/// What `task loop --check --issue <number>` says after `skip: `, or `None` for a take.
fn check_reason(f: &Fixture, gh: &FakeGitHub, number: u32) -> Option<String> {
    let opts = Options {
        repo_root: f.repo_root.clone(),
        issues: vec![number],
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let mut out = Vec::new();
    task_loop::check(&f.env.config_dir(), &opts, gh, &Probe, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    let line = out
        .lines()
        .find(|l| l.starts_with(&format!("#{number}")))
        .unwrap_or_else(|| panic!("no line for #{number}: {out}"));
    line.split_once(": skip: ").map(|(_, why)| why.to_owned())
}

#[test]
fn no_queued_issue_shows_the_queue_as_before() {
    let f = fixture();
    let gh = github(Vec::new());

    let text = section(&f.env.base_dir(), &gh, false, 80);

    assert!(text.contains("  queue: empty\n"), "{text}");
    assert!(!text.contains("blocked"), "{text}");
}

#[test]
fn a_takeable_issue_shows_no_reason_and_no_blocked_count() {
    let f = fixture();
    let gh = github(vec![issue(7, "Seven", &[READY, "size:S"], "")]);

    let text = section(&f.env.base_dir(), &gh, false, 80);

    assert!(
        text.contains("  queue: 1 issue\n    #7: Seven (size S)\n"),
        "{text}"
    );
    assert!(!text.contains("blocked"), "{text}");
    assert_eq!(check_reason(&f, &gh, 7), None);
}

#[test]
fn a_blocked_issue_shows_the_reason_task_loop_check_gives() {
    let f = fixture();
    // #530's situation on 2026-11-09: it depends on #527, an open issue.
    let gh = github(vec![
        issue(527, "Stop", &[], ""),
        issue(
            530,
            "Toast",
            &[READY, "size:M"],
            "Why\n\n**Depends on:** #527\n",
        ),
        issue(536, "Status", &[READY, "size:S"], ""),
    ]);

    let text = section(&f.env.base_dir(), &gh, false, 80);

    let why = check_reason(&f, &gh, 530).expect("530 is passed over");
    assert_eq!(why, "depends on #527 (an open issue), not merged");
    assert!(
        text.contains(&format!(
            "  queue: 2 issues (1 blocked)\n    #536: Status (size S)\n    #530: Toast (size \
             M) — blocked: {why}\n"
        )),
        "{text}"
    );
}

#[test]
fn open_points_and_two_reasons_each_show_the_check_text() {
    let f = fixture();
    let gh = github(vec![
        issue(3, "Points", &[READY, "size:S"], OPEN_POINTS),
        issue(4, "Two", &[READY], OPEN_POINTS),
    ]);

    let text = section(&f.env.base_dir(), &gh, false, 80);

    let three = check_reason(&f, &gh, 3).expect("3 is passed over");
    let four = check_reason(&f, &gh, 4).expect("4 is passed over");
    assert!(four.contains("; also: "), "{four}");
    assert!(
        text.contains(&format!(
            "  queue: 2 issues (2 blocked)\n    #3: Points (size S) — blocked: {three}\n    #4: \
             Two — blocked: {four}\n"
        )),
        "{text}"
    );
}

#[test]
fn on_a_terminal_ten_show_and_the_count_still_counts_the_eleventh() {
    let f = fixture();
    let mut issues: Vec<Issue> = (1..=10)
        .map(|n| issue(n, &format!("T{n}"), &[READY, "size:S"], ""))
        .collect();
    issues.push(issue(11, "Last", &[READY, "size:L"], OPEN_POINTS));
    let gh = github(issues);

    let text = section(&f.env.base_dir(), &gh, true, 200);

    assert!(text.contains("  queue: 11 issues (1 blocked)\n"), "{text}");
    assert!(
        text.contains("    #10: T10 (size S)\n    … 1 more\n"),
        "{text}"
    );
    assert!(!text.contains("#11"), "{text}");
}

#[test]
fn a_long_reason_is_cut_on_a_terminal_and_whole_on_a_pipe() {
    let f = fixture();
    let gh = github(vec![issue(3, "Points", &[READY, "size:S"], OPEN_POINTS)]);
    let why = check_reason(&f, &gh, 3).unwrap();
    let whole = format!("    #3: Points (size S) — blocked: {why}");

    let piped = section(&f.env.base_dir(), &gh, false, 30);
    let cut = section(&f.env.base_dir(), &gh, true, 30);

    assert!(piped.contains(&format!("{whole}\n")), "{piped}");
    let line = cut
        .lines()
        .find(|l| l.starts_with("    #3:"))
        .expect("the issue's line");
    assert!(line.ends_with('…'), "{line}");
    assert_eq!(line.chars().count(), 30, "{line}");
}

#[test]
fn a_failed_read_is_unreadable_and_one_failed_check_is_blocked_unknown() {
    let f = fixture();
    let failing = FakeGitHub::default().failing("boom");

    let text = section(&f.env.base_dir(), &failing, false, 80);

    assert!(text.contains("  queue: unreadable (boom)\n"), "{text}");

    // #8's text isn't scripted, so `task run`'s check of it fails.
    let gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_open_issues(vec![issue(8, "Eight", &[READY, "size:S"], "")]);

    let text = section(&f.env.base_dir(), &gh, false, 80);

    assert!(
        text.contains(
            "  queue: 1 issue (1 blocked)\n    #8: Eight (size S) — blocked: unknown \
             (FakeGitHub: issue 8 is not scripted)\n"
        ),
        "{text}"
    );
}

#[test]
fn an_issue_loops_pending_list_shows_the_reason() {
    let f = fixture();
    let base = f.env.base_dir();
    let lock = LockFile {
        process: Process::new(1, T0 - 1_000),
        issues: vec![5, 6],
        cap: None,
        progress: None,
    };
    let path = lock_path(&base);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string(&lock).unwrap()).unwrap();
    let gh = github(vec![
        issue(5, "Five", &["size:S"], ""),
        issue(6, "Six", &[READY, "size:S"], OPEN_POINTS),
    ]);

    let text = section(&base, &gh, false, 80);

    let five = check_reason(&f, &gh, 5).expect("5 isn't labeled");
    let six = check_reason(&f, &gh, 6).expect("6 has open points");
    assert!(
        text.contains(&format!(
            "  pending: 2 issues (2 blocked)\n    #5: Five — blocked: {five}\n    #6: Six — \
             blocked: {six}\n"
        )),
        "{text}"
    );
}

/// Every path under `dir`, sorted.
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            all.extend(tree(&path));
        }
        all.push(path);
    }
    all.sort();
    all
}

#[test]
fn checking_the_queue_writes_nothing_reads_the_issues_once_and_changes_no_sandbox() {
    let f = fixture();
    let base = f.env.base_dir();
    let gh = github(vec![
        issue(527, "Stop", &[], ""),
        issue(530, "Toast", &[READY, "size:M"], "**Depends on:** #527\n"),
        issue(3, "Points", &[READY, "size:S"], OPEN_POINTS),
        issue(7, "Seven", &[READY, "size:S"], ""),
    ]);
    let backend = FakeBackend::default();
    let before = tree(&base);
    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };

    status::run_with(
        &base,
        &backend,
        &Probe,
        &env(false, 80),
        None,
        Some(&source),
        &mut Vec::new(),
    )
    .unwrap();

    assert_eq!(tree(&base), before);
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(backend.execs().is_empty());
    let calls = gh.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, GhCall::IssuesOpen(_)))
            .count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.iter().all(|c| matches!(
            c,
            GhCall::IssuesOpen(_) | GhCall::Issue(..) | GhCall::Pr(..) | GhCall::BranchExists(..)
        )),
        "{calls:?}"
    );
}
