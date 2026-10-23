//! M2b slice 8, step 5: `sbxm task review --issue N` as a command (spec §4, §5.2).

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, ok, play_reviews, play_tasks,
    worked_task,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_review::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::harness::Harness;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record;

fn config(reviewer: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"{reviewer}\"\n"
    ))
}

/// No fix rounds to spend: a gate failure stops the task at once instead of feeding one.
fn config_no_fix_rounds(reviewer: &str) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 0\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"{reviewer}\"\n"
    ))
}

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Issue(41),
        repo: Some("o/r".into()),
        base: None,
        clone_source: None,
        reviewer_harness: None,
        reviewer_model: None,
        reviewer_time_limit: None,
        time_limit: None,
        profile: None,
        resolved_spec_id: None,
        force: false,
        disk: Default::default(),
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn go_with_github(f: &Fixture, opts: &Options, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn go(f: &Fixture, opts: &Options, backend: &FakeBackend) -> Out {
    go_with_github(f, opts, backend, &FakeGitHub::default())
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

#[test]
fn a_clean_review_prints_the_round_and_where_the_review_is() {
    let f = config("codex");
    let backend = backend(&f, &[CLEAN]);
    worked_task(&f, &backend);

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    assert!(
        out.out
            .contains("issue-41: review round 1: 0 must-fix finding(s)"),
        "{}",
        out.out
    );
    assert!(out.out.contains("issue-41: ready"), "{}", out.out);
    assert!(
        out.out
            .contains(&meta(&f).join("review.md").display().to_string()),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("sbxm task finish --issue 41"),
        "{}",
        out.out
    );
}

/// Issue 123, decision 176(e): a review with no readable `Risk:` line warns, but never fails the
/// task, and the warning stays separate from a `Repeat of:` warning's count.
#[test]
fn a_review_with_no_risk_line_warns_but_still_succeeds() {
    let f = config("codex");
    let backend = backend(&f, &[CLEAN]);
    worked_task(&f, &backend);

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    assert!(out.warn.contains("risk is \"unknown\""), "{}", out.warn);
}

#[test]
fn a_review_with_a_readable_risk_line_does_not_warn() {
    let f = config("codex");
    let review_text = "Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n";
    let backend = backend(&f, &[review_text]);
    worked_task(&f, &backend);

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    assert!(!out.warn.contains("risk is"), "{}", out.warn);
}

#[test]
fn must_fix_findings_show_the_fix_round_and_the_second_review() {
    let f = config("codex");
    let backend = backend(&f, &[ONE, CLEAN]);
    worked_task(&f, &backend);

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    let text = &out.out;
    assert!(text.contains("review round 1: 1 must-fix"), "{text}");
    assert!(text.contains("fix round ran"), "{text}");
    assert!(text.contains("review round 2: 0 must-fix"), "{text}");
    assert!(text.contains("issue-41: ready"), "{text}");
}

#[test]
fn findings_left_after_the_fix_round_are_reported_and_the_command_still_succeeds() {
    let f = config("codex");
    let backend = backend(&f, &[ONE, ONE]);
    worked_task(&f, &backend);

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    assert!(
        out.out.contains("1 must-fix finding(s) left"),
        "{}",
        out.out
    );
    assert!(
        out.out.contains("sbxm task file-findings --issue 41"),
        "{}",
        out.out
    );
    // The budget ran out: another round is one command away (issue 118).
    assert!(
        out.out
            .contains("or give it more fix rounds: sbxm task resume --issue 41 --rounds 1"),
        "{}",
        out.out
    );
}

#[test]
fn failing_gates_fail_the_command_and_no_reviewer_runs() {
    let f = config_no_fix_rounds("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let red = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(101),
        },
    );

    let out = go(&f, &options(&f), &red);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("gates failed") && message.contains("cargo test"),
        "{message}"
    );
    assert!(
        out.out.contains("gates failed") && out.out.contains("gates.log"),
        "{}",
        out.out
    );
    assert!(red.creates().is_empty());
}

/// Issue 186: a gate that could not run at all never gets a fix round, even with the budget
/// (here, the default 3) still open — unlike a gate that ran and failed (the test above).
#[test]
fn a_gate_that_cannot_run_ends_the_review_without_a_fix_round() {
    let f = config("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let red = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_failing_exec_matching("cargo test");

    let out = go(&f, &options(&f), &red);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("could not run"), "{message}");
    assert!(message.contains("gates.log"), "{message}");
    assert!(
        out.out.contains("gates could not run") && out.out.contains("gates.log"),
        "{}",
        out.out
    );
    assert!(red.creates().is_empty(), "no reviewer sandbox");
    let record = record::read(&meta(&f).join("task.json")).unwrap();
    assert_eq!(record.round, 0, "no fix round ran");
}

#[test]
fn a_reviewer_that_fails_fails_the_command() {
    let f = config("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let broken =
        FakeBackend::with_secrets(&["anthropic", "openai"]).with_failing_exec_matching("codex");

    let out = go(&f, &options(&f), &broken);

    assert!(out.result.is_err());
}

#[test]
fn the_reviewer_flags_override_the_file_and_are_checked_before_anything_is_created() {
    let f = config("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let mut opts = options(&f);
    opts.reviewer_harness = Some(Harness::Antigravity);
    let before = good.creates().len();

    let out = go(&f, &opts, &good);

    // Antigravity needs the `google` secret, which is not stored.
    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("google"), "{message}");
    assert_eq!(good.creates().len(), before, "nothing was created");
}

#[test]
fn a_harness_that_cannot_run_headless_is_refused_before_anything_happens() {
    let f = config("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let mut opts = options(&f);
    opts.reviewer_harness = Some(Harness::Pi);
    let before = good.execs().len();

    let message = format!("{:#}", go(&f, &opts, &good).result.unwrap_err());

    assert!(
        message.contains("pi") && message.contains("claude, codex or antigravity"),
        "{message}"
    );
    assert_eq!(good.execs().len(), before);
}

#[test]
fn an_empty_or_blank_reviewer_model_flag_is_refused_before_anything_happens() {
    for bad in ["", "  "] {
        let f = config("codex");
        let good = backend(&f, &[CLEAN]);
        worked_task(&f, &good);
        let mut opts = options(&f);
        opts.reviewer_model = Some(bad.into());
        let before_execs = good.execs().len();
        let before_creates = good.creates().len();
        let before_secrets = good.secret_service_calls();
        let github = FakeGitHub::default();

        let message = format!(
            "{:#}",
            go_with_github(&f, &opts, &good, &github)
                .result
                .unwrap_err()
        );

        assert!(message.contains("--reviewer-model"), "{bad:?}: {message}");
        assert_eq!(good.execs().len(), before_execs, "{bad:?}");
        assert_eq!(good.creates().len(), before_creates, "{bad:?}");
        assert_eq!(good.secret_service_calls(), before_secrets, "{bad:?}");
        assert!(github.calls().is_empty(), "{bad:?}");
    }
}

#[test]
fn a_bad_time_limit_is_refused_before_anything_happens() {
    let f = config("codex");
    let good = backend(&f, &[CLEAN]);
    worked_task(&f, &good);
    let mut opts = options(&f);
    opts.reviewer_time_limit = Some("soon".into());
    let before = good.execs().len();

    let message = format!("{:#}", go(&f, &opts, &good).result.unwrap_err());

    assert!(
        message.contains("soon") && message.contains("duration"),
        "{message}"
    );
    assert_eq!(good.execs().len(), before);
}

#[test]
fn a_reviewer_like_the_worker_warns_on_the_warning_writer() {
    let f = config("claude");
    let good = FakeBackend::with_secrets(&["anthropic"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook({
            let worker = play_tasks(
                &f.env.base_dir(),
                "main",
                Play {
                    commits: vec!["a.txt".into()],
                    result_md: Some(b"done\n".to_vec()),
                    bundle_bytes: None,
                },
            );
            move |sandbox, spec| worker(sandbox, spec)
        });
    worked_task(&f, &good);

    // The claude reviewer writes no review.md in this fake, so the review itself fails; the
    // warning must already have been printed.
    let out = go(&f, &options(&f), &good);

    assert!(out.warn.contains("same harness"), "{}", out.warn);
}

#[test]
fn a_task_that_does_not_exist_is_refused_with_how_to_list_them() {
    let f = config("codex");
    let message = format!(
        "{:#}",
        go(&f, &options(&f), &backend(&f, &[CLEAN]))
            .result
            .unwrap_err()
    );
    assert!(
        message.contains("no task issue-41") && message.contains("sbxm task status"),
        "{message}"
    );
}

#[test]
fn a_missing_config_file_says_how_to_make_one() {
    let f = config("codex");
    let mut opts = options(&f);
    opts.repo_root = f.env.tmp.path().join("nowhere");
    let message = format!(
        "{:#}",
        go(&f, &opts, &backend(&f, &[CLEAN])).result.unwrap_err()
    );
    assert!(
        message.contains("sbxm-task.toml") && message.contains("sbxm task init"),
        "{message}"
    );
}

// ---- `task review --pr N` ----

use common::task_fixture::{add_pr_head, issue_text, source};
use sbxm::github::fake::GhCall;
use sbxm::github::{PrInfo, PrState};

fn pr_info() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "Adds x.\n\nFixes #4".into(),
        closing_issues: vec![4],
    }
}

fn pr_github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(pr_info())
        .with_issue_text(issue_text(4))
}

fn pr_options(f: &Fixture) -> Options {
    let mut opts = options(f);
    opts.target = Target::Pr(7);
    opts.clone_source = Some(source(f));
    opts
}

fn pr_backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(play_reviews(
            &f.env.base_dir(),
            "pr-7",
            reviews.iter().map(|s| (*s).to_owned()).collect(),
        ))
}

fn go_pr(f: &Fixture, opts: &Options, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        opts,
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn pr_meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "pr-7")
}

#[test]
fn a_pr_review_prints_the_result_and_that_it_was_posted() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github();

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[ONE]), &github);

    out.result.unwrap();
    assert!(
        out.out
            .contains("pr-7: review round 1: 1 must-fix finding(s)"),
        "{}",
        out.out
    );
    assert!(out.out.contains("posted on PR #7"), "{}", out.out);
    assert!(
        !out.out.contains("finish"),
        "a PR has nothing to finish: {}",
        out.out
    );
    assert!(
        github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrComment(r, 7, _) if r == "o/r"))
    );
}

#[test]
fn a_pr_from_a_fork_is_refused_before_anything_is_created() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let mut fork = pr_info();
    fork.is_cross_repository = true;
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(fork);
    let backend = pr_backend(&f, &[CLEAN]);

    let out = go_pr(&f, &pr_options(&f), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("fork"), "{message}");
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
    assert!(!pr_meta(&f).exists());
}

#[test]
fn a_closed_pr_is_refused() {
    let f = config("codex");
    let mut closed = pr_info();
    closed.state = PrState::Closed;
    let github = FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(closed);

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("closed") && message.contains("only open"),
        "{message}"
    );
}

#[test]
fn a_pr_github_cannot_show_is_an_error() {
    let f = config("codex");
    let github = FakeGitHub::default().with_default_branch("main");

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("pr 7"), "{message}");
}

#[test]
fn failing_gates_fail_the_pr_review_and_post_nothing() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github();
    let red = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "cargo test",
        ExecOutput {
            stdout: String::new(),
            stderr: "red\n".into(),
            exit_code: Some(101),
        },
    );

    let out = go_pr(&f, &pr_options(&f), &red, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("gates failed") && message.contains("cargo test"),
        "{message}"
    );
    assert!(
        out.out.contains("gates failed") && out.out.contains("gates.log"),
        "{}",
        out.out
    );
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrComment(..)))
    );
}

#[test]
fn a_review_that_failed_can_be_run_again_without_starting_over() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github();
    // First attempt: the reviewer writes no usable review.
    let bad = pr_backend(&f, &["Looks fine.\n"]);
    assert!(go_pr(&f, &pr_options(&f), &bad, &github).result.is_err());

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert!(out.out.contains("posted on PR #7"), "{}", out.out);
}

/// Issue 195, gap G34: an unchanged head is refused with the commit and the `--force` command,
/// and forcing it reviews the PR again (same head) instead of needing `task rm --pr` first.
#[test]
fn a_pr_with_an_unchanged_head_is_refused_unless_forced() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github();
    let backend = pr_backend(&f, &[CLEAN, ONE]);
    go_pr(&f, &pr_options(&f), &backend, &github)
        .result
        .unwrap();

    let out = go_pr(&f, &pr_options(&f), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("already reviewed") && message.contains("--force"),
        "{message}"
    );

    let mut forced = pr_options(&f);
    forced.force = true;
    let out = go_pr(&f, &forced, &backend, &github);

    out.result.unwrap();
    assert!(
        out.out
            .contains("pr-7: review round 2: 1 must-fix finding(s)"),
        "{}",
        out.out
    );
}

#[test]
fn the_pr_review_uses_the_default_branch_as_the_base() {
    let f = config("codex");
    add_pr_head(&f, 7);
    // The base must exist: the risk floor diffs the PR against it (issue 123).
    common::git(&f.origin, &["branch", "trunk", "main"]);
    let github = pr_github().with_default_branch("trunk");

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert_eq!(
        record::read(&pr_meta(&f).join("task.json")).unwrap().base,
        "trunk"
    );
}

#[test]
fn a_pr_review_whose_base_is_missing_is_refused_before_any_sandbox_or_gate() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github().with_default_branch("trunk");
    let backend = pr_backend(&f, &[CLEAN]);

    let out = go_pr(&f, &pr_options(&f), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("changed paths"), "{message}");
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
}

#[test]
fn the_recorded_worker_decides_the_fix_adapter_and_the_default_reviewer() {
    // The task was started with a Codex worker (a flag; or the file said so then) ...
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [worker]\nharness = \"codex\"\n",
    );
    let backend = backend(&f, &[ONE, CLEAN]);
    worked_task(&f, &backend);
    assert_eq!(
        record::read(&meta(&f).join("task.json"))
            .unwrap()
            .worker
            .unwrap()
            .harness,
        "codex"
    );
    // ... and the file read at review time says nothing about it (so claude is its worker).
    std::fs::write(
        f.env.tmp.path().join("target-repo").join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    )
    .unwrap();

    let out = go(&f, &options(&f), &backend);

    out.result.unwrap();
    let reviewers: Vec<_> = backend
        .creates()
        .into_iter()
        .filter(|c| c.name.contains("-review-"))
        .collect();
    assert!(!reviewers.is_empty());
    assert!(
        reviewers.iter().all(|c| c.agent != "codex"),
        "the reviewer must differ from the Codex worker: {reviewers:?}"
    );
    let fix = backend
        .execs()
        .into_iter()
        .find(|(_, spec)| spec.argv.iter().any(|a| a.contains("fix-prompt.md")))
        .expect("a fix run");
    assert_eq!(
        fix.0, "sbxm-task-issue-41-codex",
        "the recorded worker's sandbox"
    );
    assert!(
        fix.1.argv.iter().any(|a| a == "codex"),
        "the fix run must use the Codex adapter: {:?}",
        fix.1.argv
    );
}

// ---- Issue 198: a PR head the in-task review covered isn't reviewed again ----

const REVIEWED: &str = "1111111111111111111111111111111111111111";

/// A finished task for issue 4 that recorded `pr` as its PR and `commit` as what its review
/// covered and posted, as `task finish` leaves it; `continues` makes it a task that continued
/// that PR.
fn finished_issue_task(f: &Fixture, pr: &str, commit: &str, continues: bool) {
    use record::{NewTask, Process, Record, Stage, Status};
    let mut task = Record::new(
        &NewTask {
            kind: record::Kind::Issue,
            number: 4,
            repo: "o/r",
            title: "Fix 4",
            base: "main",
            branch: "issue-4",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 0),
    );
    (task.stage, task.status) = (Stage::Finished, Status::Ok);
    task.pr = Some(pr.to_owned());
    task.review = Some(record::ReviewResult {
        round: 1,
        commit: Some(commit.to_owned()),
        posted: Some(commit.to_owned()),
        ..Default::default()
    });
    if continues {
        task.continues = Some(record::PrBranch {
            pr: 7,
            branch: "feature-x".into(),
            base: "2222222222222222222222222222222222222222".into(),
        });
    }
    let meta = record::task_dir(&f.env.base_dir(), "issue-4");
    record::write(&meta, &task).unwrap();
    std::fs::write(meta.join("review.md"), CLEAN).unwrap();
}

/// [`pr_github`] with PR 7's head at `head`, and a body that looks like a posted review.
fn pr_github_at(head: &str) -> FakeGitHub {
    let mut pr = pr_info();
    pr.head_oid = head.to_owned();
    pr.body = format!("sbxm in-task review of issue #4's task, at commit {head}\n");
    FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(pr)
        .with_issue_text(issue_text(4))
}

fn posted(github: &FakeGitHub) -> bool {
    github
        .calls()
        .iter()
        .any(|c| matches!(c, GhCall::PrComment(_, 7, _)))
}

#[test]
fn a_pr_head_the_local_issue_task_reviewed_is_refused_before_anything_with_a_force_hint() {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    let github = pr_github_at(REVIEWED);
    let backend = pr_backend(&f, &[CLEAN]);

    let out = go_pr(&f, &pr_options(&f), &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains(&format!("already reviewed at `{REVIEWED}`")),
        "{message}"
    );
    assert!(
        message.contains("`sbxm task review --pr 7 --repo o/r --force`"),
        "{message}"
    );
    assert!(message.contains("issue-4"), "{message}");
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
    assert!(!pr_meta(&f).exists());
    assert!(!posted(&github));
}

/// Review M-1: the `--force` command keeps the `--base` the refused command was given.
#[test]
fn the_force_hint_of_an_already_reviewed_head_keeps_the_given_base() {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    let github = pr_github_at(REVIEWED);
    let backend = pr_backend(&f, &[CLEAN]);
    let mut opts = pr_options(&f);
    opts.base = Some("release".into());

    let out = go_pr(&f, &opts, &backend, &github);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("`sbxm task review --pr 7 --repo o/r --base release --force`"),
        "{message}"
    );
}

#[test]
fn force_reviews_a_pr_head_the_local_issue_task_reviewed() {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    let github = pr_github_at(REVIEWED);
    let mut opts = pr_options(&f);
    opts.force = true;

    let out = go_pr(&f, &opts, &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert!(out.out.contains("posted on PR #7"), "{}", out.out);
    assert!(posted(&github));
}

#[test]
fn a_new_commit_on_the_pr_is_reviewed_without_force() {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    let github = pr_github_at("3333333333333333333333333333333333333333");

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert!(posted(&github));
}

/// Issue 229: [`pr_github_at`] with PR 7 in `state` and from a fork or not.
fn pr_github_with(head: &str, state: PrState, fork: bool) -> FakeGitHub {
    let mut pr = pr_info();
    pr.head_oid = head.to_owned();
    pr.state = state;
    pr.is_cross_repository = fork;
    FakeGitHub::default()
        .with_default_branch("main")
        .with_pr(pr)
        .with_issue_text(issue_text(4))
}

/// Issue 229: a PR that isn't reviewable is refused as such, before the already-reviewed check,
/// whatever its head and `--force` say, and nothing is created.
#[test]
fn a_closed_merged_or_fork_pr_is_refused_as_such_before_the_already_reviewed_check() {
    const OTHER: &str = "3333333333333333333333333333333333333333";
    for (state, fork, head, force, expected) in [
        (
            PrState::Closed,
            false,
            REVIEWED,
            false,
            "PR #7 is closed; only open PRs are reviewed",
        ),
        (
            PrState::Closed,
            false,
            REVIEWED,
            true,
            "PR #7 is closed; only open PRs are reviewed",
        ),
        (
            PrState::Closed,
            false,
            OTHER,
            false,
            "PR #7 is closed; only open PRs are reviewed",
        ),
        (
            PrState::Merged,
            false,
            REVIEWED,
            false,
            "PR #7 is merged; only open PRs are reviewed",
        ),
        (
            PrState::Merged,
            false,
            REVIEWED,
            true,
            "PR #7 is merged; only open PRs are reviewed",
        ),
        (
            PrState::Merged,
            false,
            OTHER,
            false,
            "PR #7 is merged; only open PRs are reviewed",
        ),
        (
            PrState::Open,
            true,
            REVIEWED,
            false,
            "PR #7 comes from a fork",
        ),
    ] {
        let case = format!("{state:?}, fork {fork}, head {head}, force {force}");
        let f = config("codex");
        add_pr_head(&f, 7);
        finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
        let github = pr_github_with(head, state, fork);
        let backend = pr_backend(&f, &[CLEAN]);
        let mut opts = pr_options(&f);
        opts.force = force;

        let out = go_pr(&f, &opts, &backend, &github);

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains(expected), "{case}: {message}");
        assert!(!message.contains("already reviewed"), "{case}: {message}");
        assert!(!message.contains("--force"), "{case}: {message}");
        assert!(
            backend.creates().is_empty() && backend.execs().is_empty(),
            "{case}"
        );
        assert!(!pr_meta(&f).exists(), "{case}");
        assert!(!posted(&github), "{case}");
    }
}

/// Only a local task folder decides: a PR body or comment that looks like a posted review, with
/// no task recording the PR, changes nothing (sbxm reads no comments to decide).
#[test]
fn with_no_local_task_recording_the_pr_it_is_reviewed_whatever_github_says() {
    let f = config("codex");
    add_pr_head(&f, 7);
    let github = pr_github_at(REVIEWED);

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert!(posted(&github));
}

/// PR review, rounds 1 and 2: only the record of a posted review counts. This is the task
/// `task finish` leaves when it had no `review.md` to post, when the post failed, and when it was
/// interrupted between recording the PR and posting: the PR, the reviewed commit and `review.md`
/// are all there, the proof of a post is not.
#[test]
fn a_task_with_no_record_of_a_posted_review_does_not_count() {
    let f = config("codex");
    add_pr_head(&f, 7);
    finished_issue_task(&f, "https://github.com/o/r/pull/7", REVIEWED, false);
    let meta = record::task_dir(&f.env.base_dir(), "issue-4");
    let mut task = record::read(&meta.join("task.json")).unwrap();
    task.review.as_mut().unwrap().posted = None;
    record::write(&meta, &task).unwrap();
    let github = pr_github_at(REVIEWED);

    let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

    out.result.unwrap();
    assert!(posted(&github));
}

#[test]
fn a_task_recording_another_repos_pr_or_continuing_the_pr_does_not_count() {
    for (url, continues) in [
        ("https://github.com/o/other/pull/7", false),
        ("https://github.com/o/r/pull/8", false),
        ("https://github.com/o/r/pull/7", true),
    ] {
        let f = config("codex");
        add_pr_head(&f, 7);
        finished_issue_task(&f, url, REVIEWED, continues);
        let github = pr_github_at(REVIEWED);

        let out = go_pr(&f, &pr_options(&f), &pr_backend(&f, &[CLEAN]), &github);

        out.result
            .unwrap_or_else(|e| panic!("{url} {continues}: {e:#}"));
        assert!(posted(&github), "{url} {continues}");
    }
}

/// Issue 195, review finding S-1: `--force`'s help names both refusals it overrides, the one for
/// a head an issue task's in-task review covered and the one for the PR task's own last round.
#[test]
fn the_force_help_names_both_reasons_a_head_counts_as_reviewed() {
    use assert_cmd::Command;
    let output = Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "review", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains("in-task review"), "{help}");
    assert!(help.contains("PR task's own last review"), "{help}");
}

// Issue 210: a low sandbox drive refuses before the reviewer's sandbox, leaving the task as it was.

#[test]
fn a_low_sandbox_drive_refuses_before_the_review() {
    let f = config("codex");
    worked_task(&f, &backend(&f, &[CLEAN]));
    let before = std::fs::read(meta(&f).join("task.json")).unwrap();
    let backend = backend(&f, &[CLEAN]);
    let mut opts = options(&f);
    opts.disk = common::low_disk(&f.env);

    let out = go(&f, &opts, &backend);

    let err = format!("{:#}", out.result.unwrap_err());
    assert!(err.starts_with(common::LOW_DISK), "{err}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert_eq!(std::fs::read(meta(&f).join("task.json")).unwrap(), before);
}
