//! M2b slice 6, step 5: `sbxm task start` as a command (spec §4, §5.1): the repo and base
//! come from flags or the checkout, flags override `sbxm-task.toml`, and the output says what
//! happened to each task.

mod common;

use std::path::PathBuf;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, issue_text, ok, open_issue, play_tasks,
    source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, Options, github_repo};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, PrInfo, PrState};
use sbxm::harness::Harness;
use sbxm::task::record;
use sbxm::task::repo::Identity;

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
    }
}

fn playing(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play_tasks(
            &f.env.base_dir(),
            "main",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn run(f: &Fixture, opts: &Options, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &Probe,
        &mut out,
        &mut warn,
    );
    let (out, warn) = (
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap(),
    );
    if let Err(e) = &result {
        // The error alone ("1 of 1 task(s) failed; see above") hides why; show what was above.
        eprintln!("task start failed: {e:#}\n--- out ---\n{out}\n--- warn ---\n{warn}");
    }
    Out { result, out, warn }
}

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(issue_text(1))
        .with_issue_text(issue_text(2))
        .with_open_issues(vec![
            open_issue(41, &["should-fix"], ""),
            open_issue(1, &["should-fix"], ""),
            open_issue(2, &["must-fix"], ""),
            open_issue(3, &["question"], ""),
        ])
}

#[test]
fn github_urls_become_owner_slash_name() {
    for (url, want) in [
        ("https://github.com/tutros/sbxm", "tutros/sbxm"),
        ("https://github.com/tutros/sbxm.git", "tutros/sbxm"),
        ("https://github.com/tutros/sbxm/", "tutros/sbxm"),
        ("git@github.com:tutros/sbxm.git", "tutros/sbxm"),
        ("ssh://git@github.com/tutros/sbxm.git", "tutros/sbxm"),
        ("https://user@github.com/tutros/sbxm.git", "tutros/sbxm"),
    ] {
        assert_eq!(github_repo(url).unwrap(), want, "{url}");
    }
}

#[test]
fn anything_else_is_refused_with_the_flag_to_use() {
    for url in [
        "https://gitlab.com/o/r.git",
        "https://github.com/onlyowner",
        "https://github.com/o/r/extra",
        "/some/local/path",
        "https://github.com/o/r name",
        "",
    ] {
        let message = format!("{:#}", github_repo(url).unwrap_err());
        assert!(message.contains("--repo owner/name"), "{url}: {message}");
    }
}

#[test]
fn a_started_task_prints_what_happened_and_the_next_step() {
    let f = fixture();
    let (backend, github) = (playing(&f), github());

    let out = run(&f, &options(&f), &backend, &github);

    out.result.unwrap();
    assert!(out.out.contains("issue-41"), "{}", out.out);
    assert!(
        out.out.contains("completed") && out.out.contains("1 commit"),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("sbxm task review --issue 41"),
        "{}",
        out.out
    );
}

#[test]
fn a_failed_task_is_reported_and_the_command_fails() {
    let f = fixture();
    let backend = playing(&f).with_failing_create_for("sbxm-task-issue-41-claude");

    let out = run(&f, &options(&f), &backend, &github());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("1 of 1"), "{message}");
    assert!(
        out.out.contains("issue-41") && out.out.contains("sbxm-task-issue-41-claude"),
        "{}",
        out.out
    );
}

#[test]
fn passing_gates_are_reported_after_the_worker_line() {
    let f = fixture();
    let (backend, github) = (playing(&f), github());

    let out = run(&f, &options(&f), &backend, &github);

    out.result.unwrap();
    assert!(
        out.out.contains("gates: passed (2 sandbox, 0 host)"),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("sbxm task review --issue 41"),
        "{}",
        out.out
    );
}

#[test]
fn failing_gates_fail_the_command_and_say_where_to_look() {
    let f = fixture();
    let backend = playing(&f).with_exec_output_matching(
        "cargo test",
        sbxm::backend::ExecOutput {
            stdout: String::new(),
            stderr: "1 test failed\n".into(),
            exit_code: Some(101),
        },
    );

    let out = run(&f, &options(&f), &backend, &github());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("1 of 1"), "{message}");
    assert!(
        out.out
            .contains("gates: failed: `cargo test` (sandbox, exit 101)"),
        "{}",
        out.out
    );
    assert!(out.out.contains("gates.log"), "{}", out.out);
    assert!(
        out.out.contains("sbxm task gates --issue 41"),
        "{}",
        out.out
    );
    assert!(
        !out.out.contains("sbxm task review --issue 41"),
        "{}",
        out.out
    );
}

#[test]
fn notes_from_the_worker_are_printed() {
    let f = fixture();
    let backend = playing(&f).with_exec_output_matching("status", ok(" M a.rs\n"));

    let out = run(&f, &options(&f), &backend, &github());

    out.result.unwrap();
    assert!(out.out.contains("1 uncommitted"), "{}", out.out);
}

#[test]
fn workers_picks_by_the_selection_rules_and_prints_the_skips() {
    let f = fixture();
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(2);
    let (backend, github) = (playing(&f), github());

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    // must-fix #2 first, then the lowest should-fix number (#1); #41 is beyond the cap.
    assert!(
        out.out.contains("issue-2") && out.out.contains("issue-1"),
        "{}",
        out.out
    );
    assert!(!out.out.contains("issue-41"), "{}", out.out);
    let mut created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    created.sort();
    assert_eq!(
        created,
        ["sbxm-task-issue-1-claude", "sbxm-task-issue-2-claude"]
    );
}

#[test]
fn skipped_issues_say_why() {
    let f = fixture();
    let mut opts = options(&f);
    opts.issues = vec![3, 1];
    let (backend, github) = (playing(&f), github());

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    assert!(
        out.out
            .contains("#3: skipped, a question, needs your answer first"),
        "{}",
        out.out
    );
}

#[test]
fn the_base_defaults_to_the_repos_default_branch() {
    let f = fixture();
    let github = github().with_default_branch("trunk");

    let out = run(&f, &options(&f), &playing(&f), &github);

    // The stand-in origin has no `trunk`: the failure proves which branch was asked for.
    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        out.out.contains("trunk") || message.contains("trunk"),
        "{message} / {}",
        out.out
    );
}

#[test]
fn flags_override_the_config_file() {
    let f = fixture();
    let mut opts = options(&f);
    opts.worker_harness = Some(Harness::Codex);

    // The config says claude (anthropic is stored); codex needs `openai`, which is not.
    let out = run(&f, &opts, &playing(&f), &github());

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("openai") || out.out.contains("openai"),
        "{message} / {}",
        out.out
    );
}

#[test]
fn a_harness_that_cannot_run_headless_is_refused_before_anything_happens() {
    let f = fixture();
    let mut opts = options(&f);
    opts.worker_harness = Some(Harness::Pi);
    let backend = playing(&f);

    let message = format!(
        "{:#}",
        run(&f, &opts, &backend, &github()).result.unwrap_err()
    );

    assert!(
        message.contains("pi") && message.contains("claude, codex or antigravity"),
        "{message}"
    );
    assert!(backend.creates().is_empty());
}

#[test]
fn a_bad_time_limit_flag_is_refused_before_anything_happens() {
    let f = fixture();
    let mut opts = options(&f);
    opts.time_limit = Some("soon".into());
    let backend = playing(&f);

    let message = format!(
        "{:#}",
        run(&f, &opts, &backend, &github()).result.unwrap_err()
    );

    assert!(
        message.contains("soon") && message.contains("duration"),
        "{message}"
    );
    assert!(backend.creates().is_empty());
}

#[test]
fn a_missing_config_file_says_how_to_make_one() {
    let f = fixture();
    let mut opts = options(&f);
    opts.repo_root = PathBuf::from(f.env.tmp.path()).join("no-such-repo");

    let message = format!(
        "{:#}",
        run(&f, &opts, &playing(&f), &github()).result.unwrap_err()
    );

    assert!(
        message.contains("sbxm-task.toml") && message.contains("sbxm task init"),
        "{message}"
    );
}

#[test]
fn a_repo_that_is_not_owner_slash_name_is_refused() {
    let f = fixture();
    let mut opts = options(&f);
    opts.repo = Some("not a repo".into());

    let message = format!(
        "{:#}",
        run(&f, &opts, &playing(&f), &github()).result.unwrap_err()
    );

    assert!(message.contains("owner/name"), "{message}");
}

#[test]
fn neither_issues_nor_workers_is_refused() {
    let f = fixture();
    let mut opts = options(&f);
    opts.issues = Vec::new();

    let message = format!(
        "{:#}",
        run(&f, &opts, &playing(&f), &github()).result.unwrap_err()
    );

    assert!(
        message.contains("--issue") && message.contains("--workers"),
        "{message}"
    );
}

#[test]
fn warnings_go_to_the_warning_writer() {
    let f = fixture();
    let out = run(&f, &options(&f), &playing(&f), &github());
    out.result.unwrap();
    // The default config has a different reviewer harness and no unsupported settings.
    assert!(out.warn.is_empty(), "{}", out.warn);
}

fn pr_in_state(number: u32, state: PrState) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("issue-{number}"),
        is_cross_repository: false,
        state,
        title: format!("PR {number}"),
        body: String::new(),
        closing_issues: Vec::new(),
    }
}

fn github_with_pr_finding(state: PrState) -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(issue_text(2))
        .with_pr(pr_in_state(40, state))
        .with_open_issues(vec![
            open_issue(41, &["must-fix"], "PR: #40\nfinding"),
            open_issue(2, &["must-fix"], ""),
        ])
}

#[test]
fn workers_picks_a_must_fix_issue_of_an_open_pr_first() {
    let f = fixture();
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(1);
    let (backend, github) = (playing(&f), github_with_pr_finding(PrState::Open));

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-41-claude"]);
}

#[test]
fn workers_skips_a_must_fix_issue_of_a_merged_pr_and_says_why() {
    let f = fixture();
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(2);
    let (backend, github) = (playing(&f), github_with_pr_finding(PrState::Merged));

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    assert!(
        out.out.contains("#41: skipped, its PR #40 is merged"),
        "{}",
        out.out
    );
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-2-claude"]);
}

#[test]
fn an_explicit_issue_starts_whatever_its_pr() {
    let f = fixture();
    let opts = options(&f);
    let (backend, github) = (playing(&f), github_with_pr_finding(PrState::Merged));

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-41-claude"]);
}

/// #41 is a must-fix issue whose `PR: #999` no longer exists; #2 is an unrelated eligible issue.
fn github_with_a_stale_pr_finding(body: &str, labels: &[&str], extra: Vec<Issue>) -> FakeGitHub {
    let mut open = vec![
        open_issue(41, labels, body),
        open_issue(2, &["must-fix"], ""),
    ];
    open.extend(extra);
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(issue_text(2))
        .with_open_issues(open)
}

fn workers_start_only_issue_2(f: &Fixture, github: &FakeGitHub) {
    let mut opts = options(f);
    opts.issues = Vec::new();
    opts.workers = Some(2);
    let backend = playing(f);

    let out = run(f, &opts, &backend, github);

    out.result.unwrap();
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-2-claude"], "{}", out.out);
}

#[test]
fn a_stale_pr_on_a_question_is_still_skipped_as_a_question() {
    let f = fixture();
    let github =
        github_with_a_stale_pr_finding("PR: #999\nfinding", &["must-fix", "question"], vec![]);
    workers_start_only_issue_2(&f, &github);
}

#[test]
fn a_stale_pr_on_a_blocked_issue_is_still_skipped_as_blocked() {
    let f = fixture();
    let github = github_with_a_stale_pr_finding(
        "PR: #999\n**Depends on:** #3",
        &["must-fix"],
        vec![open_issue(3, &["question"], "")],
    );
    workers_start_only_issue_2(&f, &github);
}

#[test]
fn a_stale_pr_on_an_issue_that_already_has_a_task_is_still_skipped_as_having_one() {
    let f = fixture();
    // #41 gets its task first, from a run that never reads its PR (an explicit issue).
    let first = run(&f, &options(&f), &playing(&f), &github());
    first.result.unwrap();
    let github = github_with_a_stale_pr_finding("PR: #999\nfinding", &["must-fix"], vec![]);
    workers_start_only_issue_2(&f, &github);
}

#[test]
fn a_stale_pr_on_an_issue_related_to_an_existing_task_is_still_skipped_as_related() {
    let f = fixture();
    let first = run(&f, &options(&f), &playing(&f), &github());
    first.result.unwrap(); // #41 has its task
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(issue_text(2))
        .with_open_issues(vec![
            open_issue(41, &["must-fix"], ""),
            open_issue(42, &["must-fix"], "PR: #999\n**Related:** #41"),
            open_issue(2, &["must-fix"], ""),
        ]);
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(3);
    let backend = playing(&f);

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    assert!(out.out.contains("#42: skipped"), "{}", out.out);
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-2-claude"]);
}

#[test]
fn an_unreadable_pr_skips_its_issue_with_a_reason_and_starts_nothing_for_it() {
    let f = fixture();
    let github = github_with_a_stale_pr_finding("PR: #999\nfinding", &["must-fix"], vec![]);
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(2);
    let backend = playing(&f);

    let out = run(&f, &opts, &backend, &github);

    out.result.unwrap();
    assert!(
        out.out
            .contains("#41: skipped, its PR #999 couldn't be read"),
        "{}",
        out.out
    );
    // The warning carries the lookup's own reason.
    assert!(
        out.warn.contains("#41") && out.warn.contains("PR #999") && out.warn.contains("skipped"),
        "{}",
        out.warn
    );
    // Only the unrelated issue starts; no task folder or sandbox exists for #41.
    let created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    assert_eq!(created, ["sbxm-task-issue-2-claude"]);
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

#[test]
fn when_every_candidate_is_skipped_an_unreadable_pr_is_still_warned_about() {
    let f = fixture();
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["must-fix"], "PR: #999\nfinding")]);
    let mut opts = options(&f);
    opts.issues = Vec::new();
    opts.workers = Some(1);
    let backend = playing(&f);

    let out = run(&f, &opts, &backend, &github);

    let err = format!("{:#}", out.result.unwrap_err());
    assert!(err.contains("nothing to start"), "{err}");
    assert!(err.contains("its PR #999 couldn't be read"), "{err}");
    // The warning carries the lookup's own reason ("pr 999 is not scripted").
    assert!(
        out.warn.contains("#41") && out.warn.contains("999") && out.warn.contains("not scripted"),
        "{}",
        out.warn
    );
    assert!(backend.creates().is_empty());
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

#[test]
fn an_issue_selection_already_skips_never_costs_a_pr_lookup_or_a_warning() {
    for (labels, body, extra) in [
        (&["must-fix", "question"][..], "PR: #999\nfinding", vec![]),
        (
            &["must-fix"][..],
            "PR: #999\n**Depends on:** #3",
            vec![open_issue(3, &["question"], "")],
        ),
    ] {
        let f = fixture();
        let github = github_with_a_stale_pr_finding(body, labels, extra);
        workers_start_only_issue_2(&f, &github);
        let looked_up: Vec<_> = github
            .calls()
            .into_iter()
            .filter(|c| matches!(c, sbxm::github::fake::GhCall::Pr(..)))
            .collect();
        assert!(looked_up.is_empty(), "{looked_up:?}");
    }
}
