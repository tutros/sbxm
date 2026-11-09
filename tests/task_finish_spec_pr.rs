//! Issue 438 (decision 301): `[finish] sink = "pr"` for a spec task. `task finish --spec` pushes
//! the branch, opens a PR with the risk and one `Fixes #n` line per number of the spec's
//! `Fixes:` line, and posts the in-task review as the PR's review with the `sbxm/review` status;
//! a draft when something is unresolved. `task start --spec` and `task run --spec` refuse a bad
//! `Fixes:` line, a number that isn't an open issue, and a GitHub repo `gh` can't reach, before
//! any sandbox.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, issue_text, ok, play, ready_spec_task,
    source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_finish::{self, Target};
use sbxm::commands::task_start::{self, SpecOptions};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{IssueText, ListedPr, PrInfo, PrRequest, PrState};
use sbxm::task::fixes;
use sbxm::task::record::{self, Record, Stage, Status, Stopped};
use sbxm::task::repo::Identity;

// --- The `Fixes:` line -------------------------------------------------------------------------

fn parsed(text: &str) -> Vec<u32> {
    fixes::parse(text).unwrap()
}

fn refused(text: &str) -> String {
    format!("{:#}", fixes::parse(text).unwrap_err())
}

#[test]
fn a_spec_without_a_fixes_line_fixes_nothing() {
    assert_eq!(
        parsed("# Batch\n\nSee #426 and #341 for context.\n"),
        Vec::<u32>::new()
    );
}

#[test]
fn one_number_and_three_numbers_are_read_in_their_order() {
    assert_eq!(parsed("# Batch\nFixes: #435\n"), vec![435]);
    assert_eq!(parsed("Fixes: #436, #435, #12\n"), vec![436, 435, 12]);
}

#[test]
fn the_same_number_twice_is_one_number() {
    assert_eq!(parsed("Fixes: #435, #436, #435\n"), vec![435, 436]);
}

#[test]
fn the_line_counts_wherever_it_is_and_only_it_counts() {
    let text = "# Batch\n\nWork on the docs (#426 did it before).\n\nFixes: #7\n\nMore on #8.\n";
    assert_eq!(parsed(text), vec![7]);
}

#[test]
fn a_crlf_line_and_trailing_spaces_are_read() {
    assert_eq!(parsed("# Batch\r\nFixes: #435, #436  \r\n"), vec![435, 436]);
}

#[test]
fn a_malformed_line_is_refused_naming_it_and_the_expected_form() {
    for line in [
        "Fixes: 435",
        "Fixes #435",
        "Fixes: #435 and the docs",
        "Fixes:",
        "Fixes: #435,",
        "Fixes: #0",
        "fixes: #435",
    ] {
        let message = refused(&format!("# Batch\n\n{line}\n"));
        assert!(message.contains("line 3"), "{line}: {message}");
        assert!(message.contains(line.trim()), "{line}: {message}");
        assert!(message.contains("Fixes: #435, #436"), "{line}: {message}");
    }
}

#[test]
fn a_prose_line_starting_with_the_word_is_not_a_fixes_line() {
    assert_eq!(
        parsed("Fixes are needed in two places.\n"),
        Vec::<u32>::new()
    );
}

#[test]
fn two_fixes_lines_are_refused_as_ambiguous() {
    let message = refused("Fixes: #1\n\ntext\nFixes: #2\n");
    assert!(message.contains("ambiguous"), "{message}");
    assert!(message.contains("lines 1 and 4"), "{message}");
}

// --- The start's checks ------------------------------------------------------------------------

fn pr_fixture() -> Fixture {
    common::task_fixture::fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\", \"cargo fmt \
         --check\"]\n\n[finish]\nsink = \"pr\"\n",
    )
}

fn spec_file(f: &Fixture, text: &str) -> PathBuf {
    let path = f.env.tmp.path().join("idea.md");
    fs::write(&path, text).unwrap();
    path
}

fn start_options(f: &Fixture, spec: PathBuf) -> SpecOptions {
    SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    }
}

struct Started {
    result: anyhow::Result<()>,
    warn: String,
}

fn start(f: &Fixture, spec: &Path, backend: &FakeBackend, github: &FakeGitHub) -> Started {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run_spec(
        &f.env.config_dir(),
        &start_options(f, spec.to_path_buf()),
        backend,
        github,
        &Probe,
        &mut out,
        &mut warn,
    );
    Started {
        result,
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn playing(f: &Fixture, id: &str) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join(id),
            "main",
            id,
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

fn closed_issue(number: u32) -> IssueText {
    IssueText {
        state: "CLOSED".into(),
        ..issue_text(number)
    }
}

fn open_pr_info(number: u32) -> PrInfo {
    PrInfo {
        number,
        head_ref: "feature".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "A PR".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    }
}

/// The start refused: its message, after checking nothing reached the backend or the base dir.
fn refused_start(f: &Fixture, text: &str, github: &FakeGitHub) -> String {
    let spec = spec_file(f, text);
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(f, &id);
    let started = start(f, &spec, &backend, github);
    let message = format!("{:#}", started.result.unwrap_err());
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(
        !record::task_dir(&f.env.base_dir(), &id).exists(),
        "{message}"
    );
    message
}

#[test]
fn a_closed_issue_is_refused_at_start_naming_it() {
    let f = pr_fixture();
    let github = FakeGitHub::default()
        .with_issue_text(issue_text(435))
        .with_issue_text(closed_issue(436));
    let message = refused_start(&f, "Fixes: #435, #436\n", &github);
    assert!(
        message.contains("#436") && message.contains("closed"),
        "{message}"
    );
}

#[test]
fn a_pr_number_is_refused_at_start_naming_it() {
    let f = pr_fixture();
    let github = FakeGitHub::default()
        .with_issue_text(issue_text(12))
        .with_pr(open_pr_info(12));
    let message = refused_start(&f, "Fixes: #12\n", &github);
    assert!(
        message.contains("#12") && message.contains("pull request"),
        "{message}"
    );
}

#[test]
fn a_number_that_does_not_exist_is_refused_at_start_naming_it() {
    let f = pr_fixture();
    let github = FakeGitHub::default().with_issue_text(issue_text(435));
    let message = refused_start(&f, "Fixes: #435, #999\n", &github);
    assert!(message.contains("#999"), "{message}");
}

#[test]
fn a_malformed_line_is_refused_at_start() {
    let f = pr_fixture();
    let message = refused_start(&f, "# Batch\nFixes #435\n", &FakeGitHub::default());
    assert!(
        message.contains("line 2") && message.contains("Fixes: #435, #436"),
        "{message}"
    );
}

#[test]
fn a_github_repo_gh_cannot_reach_is_refused_at_start() {
    let f = pr_fixture();
    let github = FakeGitHub::default().failing("gh: could not resolve host");
    let message = refused_start(&f, "Build a thing.\n", &github);
    assert!(
        message.contains("sink = \"pr\"") && message.contains("o/r"),
        "{message}"
    );
}

#[test]
fn no_github_origin_is_refused_at_start() {
    let f = pr_fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(&f, &id);
    let mut opts = start_options(&f, spec);
    // No --repo, and the checkout isn't a git repo with a GitHub origin.
    opts.repo = None;
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run_spec(
        &f.env.config_dir(),
        &opts,
        &backend,
        &FakeGitHub::default(),
        &Probe,
        &mut out,
        &mut warn,
    );
    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("origin"), "{message}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn no_fixes_line_warns_and_the_start_goes_on() {
    let f = pr_fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(&f, &id);

    let started = start(&f, &spec, &backend, &FakeGitHub::default());

    started.result.unwrap();
    assert!(
        started.warn.contains("no `Fixes:` line") && started.warn.contains("closes no issue"),
        "{}",
        started.warn
    );
}

#[test]
fn open_issues_pass_the_start() {
    let f = pr_fixture();
    let spec = spec_file(&f, "# Batch\nFixes: #435, #436\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(&f, &id);
    let github = FakeGitHub::default()
        .with_issue_text(issue_text(435))
        .with_issue_text(issue_text(436));

    let started = start(&f, &spec, &backend, &github);

    started.result.unwrap();
    assert!(started.warn.is_empty(), "{}", started.warn);
}

#[test]
fn the_local_sink_makes_no_github_call_at_start_whatever_the_line() {
    let f = fixture();
    let spec = spec_file(&f, "Fixes #435\n");
    let (id, _) = record::spec_id(&spec).unwrap();
    let backend = playing(&f, &id);
    let github = FakeGitHub::default();

    let started = start(&f, &spec, &backend, &github);

    started.result.unwrap();
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}

// --- `task run --spec` --------------------------------------------------------------------------

#[test]
fn task_run_refuses_a_closed_issue_before_any_sandbox() {
    use sbxm::commands::task_run::{SpecOptions as RunOptions, run_spec};
    let f = pr_fixture();
    let spec = spec_file(&f, "Fixes: #436\n");
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = FakeGitHub::default().with_issue_text(closed_issue(436));
    let opts = RunOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(&f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_spec(
        &f.env.config_dir(),
        &opts,
        None,
        &backend,
        &github,
        &Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("#436") && message.contains("closed"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

// --- The finish ---------------------------------------------------------------------------------

fn repo_root(f: &Fixture) -> PathBuf {
    f.env.tmp.path().join("target-repo")
}

fn use_pr_sink(f: &Fixture) {
    let toml = repo_root(f).join("sbxm-task.toml");
    let text = fs::read_to_string(&toml).unwrap();
    fs::write(&toml, format!("{text}\n[finish]\nsink = \"pr\"\n")).unwrap();
}

fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

fn reread(f: &Fixture, id: &str) -> Record {
    record::read(&meta(f, id).join("task.json")).unwrap()
}

/// A ready spec task under the `pr` sink whose spec, as the start saved it, is `text`.
fn ready_with_spec(f: &Fixture, text: &str) -> (PathBuf, String) {
    let (spec, id) = ready_spec_task(f);
    use_pr_sink(f);
    fs::write(meta(f, &id).join("source.md"), text).unwrap();
    (spec, id)
}

fn finish(f: &Fixture, spec: &Path, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: Target::Spec(spec.to_path_buf()),
            repo_root: repo_root(f),
            push_unresolved: false,
        },
        &FakeBackend::default(),
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn origin_head(f: &Fixture, branch: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(&f.origin)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

fn task_tip(f: &Fixture, id: &str) -> String {
    common::git(&meta(f, id).join("repo.git"), &["rev-parse", id])
}

/// Gives review.md the `Scope:` line the reviewer writes, from the base to the task's tip, so its
/// `sbxm/review` status is posted (issue 317, review M-1).
fn scope_review(f: &Fixture, id: &str) {
    let repo_git = meta(f, id).join("repo.git");
    let base = common::git(&repo_git, &["rev-parse", "main"]);
    let tip = task_tip(f, id);
    let path = meta(f, id).join("review.md");
    let review = fs::read_to_string(&path).unwrap();
    let at = review.find("Must-fix findings:").unwrap();
    let end = at + review[at..].find('\n').unwrap() + 1;
    let scoped = format!(
        "{}\nScope: `{base}..{tip}`\n{}",
        &review[..end],
        &review[end..]
    );
    fs::write(path, scoped).unwrap();
}

fn created(github: &FakeGitHub) -> PrRequest {
    github
        .calls()
        .into_iter()
        .find_map(|c| match c {
            GhCall::PrCreate(repo, request) if repo == "o/r" => Some(request),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", github.calls()))
}

fn fixes_lines(body: &str) -> Vec<&str> {
    body.lines().filter(|l| l.starts_with("Fixes #")).collect()
}

#[test]
fn finish_pushes_opens_a_pr_fixing_every_number_and_posts_the_review() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "# Docs batch\n\nFixes: #435, #436, #12\n\nSee #426.\n");
    scope_review(&f, &id);
    let github = FakeGitHub::default();

    let (result, out) = finish(&f, &spec, &github);

    result.unwrap();
    let tip = task_tip(&f, &id);
    assert_eq!(origin_head(&f, &id), Some(tip.clone()));
    let request = created(&github);
    assert_eq!(
        (request.head.as_str(), request.base.as_str()),
        (id.as_str(), "main")
    );
    assert!(!request.draft);
    assert!(request.body.starts_with("## Risk"), "{}", request.body);
    assert_eq!(
        fixes_lines(&request.body),
        ["Fixes #435", "Fixes #436", "Fixes #12"]
    );
    assert!(!request.body.contains("Fixes #426"), "{}", request.body);
    let calls = github.calls();
    let comment = calls
        .iter()
        .find_map(|c| match c {
            GhCall::PrComment(_, _, body) => Some(body.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert!(comment.contains(&tip) && comment.contains(&id), "{comment}");
    let status = calls
        .iter()
        .find_map(|c| match c {
            GhCall::CommitStatus(_, sha, status) => Some((sha.clone(), status.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert_eq!(status.0, tip);
    assert_eq!(status.1.context, "sbxm/review");
    let record = reread(&f, &id);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    let url = record.pr.clone().unwrap();
    assert!(out.contains(&url), "{out}");
    assert_eq!(record.review.unwrap().posted, Some(tip));
    assert!(out.contains("sbxm task rm --spec"), "{out}");
    // The PR's title is the spec's first heading.
    assert_eq!(request.title, "Docs batch");
    // `task review --pr N` finds the in-task review that covers the PR's head.
    let number = sbxm::task::finish::pr_number(&url).unwrap();
    assert_eq!(
        sbxm::task::finish::reviewed_pr_task(&f.env.base_dir(), "o/r", number).map(|t| t.0),
        Some(id)
    );
}

#[test]
fn one_number_gives_one_fixes_line() {
    let f = fixture();
    let (spec, _) = ready_with_spec(&f, "Fixes: #435\n");
    let github = FakeGitHub::default();
    finish(&f, &spec, &github).0.unwrap();
    assert_eq!(fixes_lines(&created(&github).body), ["Fixes #435"]);
}

// Issue 506: `**Closes on merge:** no` is an issue body's line; a spec's PR is unchanged by it.
#[test]
fn a_spec_with_a_closes_on_merge_no_line_still_fixes_its_numbers() {
    let f = fixture();
    let (spec, _) = ready_with_spec(&f, "**Closes on merge:** no\n\nFixes: #435\n");
    let github = FakeGitHub::default();
    let (result, out) = finish(&f, &spec, &github);
    result.unwrap();
    let body = created(&github).body;
    assert_eq!(fixes_lines(&body), ["Fixes #435"]);
    assert!(!body.contains("Refs #"), "{body}");
    assert!(!out.contains("leaves #"), "{out}");
}

#[test]
fn no_fixes_line_opens_a_pr_that_says_it_closes_no_issue() {
    let f = fixture();
    let (spec, _) = ready_with_spec(&f, "Build a thing.\n");
    let github = FakeGitHub::default();

    finish(&f, &spec, &github).0.unwrap();

    let body = created(&github).body;
    assert!(fixes_lines(&body).is_empty(), "{body}");
    assert!(body.contains("closes no issue"), "{body}");
}

#[test]
fn must_fix_findings_left_open_a_draft() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "Fixes: #435\n");
    let mut record = reread(&f, &id);
    record.review.as_mut().unwrap().must_fix = 2;
    record::write(&meta(&f, &id), &record).unwrap();
    let github = FakeGitHub::default();

    let (result, out) = finish(&f, &spec, &github);

    result.unwrap();
    let request = created(&github);
    assert!(request.draft);
    assert!(request.body.contains("2 must-fix"), "{}", request.body);
    assert!(out.contains("draft PR"), "{out}");
}

#[test]
fn a_work_stop_opens_a_draft() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "Fixes: #435\n");
    let mut record = reread(&f, &id);
    record.stopped = Some(Stopped::RoundsExhausted);
    record::write(&meta(&f, &id), &record).unwrap();
    let github = FakeGitHub::default();

    finish(&f, &spec, &github).0.unwrap();

    assert!(created(&github).draft);
}

#[test]
fn an_open_pr_of_the_branch_is_recorded_as_the_tasks_pr() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "Fixes: #435\n");
    let url = "https://github.com/o/r/pull/77".to_owned();
    let github = FakeGitHub::default().with_listed_pr(
        "o/r",
        ListedPr {
            number: 77,
            url: url.clone(),
            head_ref: id.clone(),
            base_ref: "main".into(),
            is_cross_repository: false,
        },
        PrState::Open,
    );

    let (result, out) = finish(&f, &spec, &github);

    result.unwrap();
    assert!(
        !github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrCreate(..))),
        "{:?}",
        github.calls()
    );
    assert_eq!(reread(&f, &id).pr, Some(url.clone()));
    assert!(out.contains("already open"), "{out}");
}

#[test]
fn a_malformed_line_saved_at_start_is_refused_at_finish_before_the_push() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "Fixes #435\n");
    let github = FakeGitHub::default();

    let (result, _) = finish(&f, &spec, &github);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("Fixes #435"), "{message}");
    assert_eq!(origin_head(&f, &id), None);
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert_eq!(reread(&f, &id).stage, Stage::Ready);
}

#[test]
fn a_failed_pr_create_names_the_spec_retry() {
    let f = fixture();
    let (spec, id) = ready_with_spec(&f, "Fixes: #435\n");
    let github = FakeGitHub::default().failing_pr_create("gh: HTTP 502", false);

    let (result, _) = finish(&f, &spec, &github);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("sbxm task finish --spec"), "{message}");
    assert!(!message.contains("--issue"), "{message}");
    assert_eq!(reread(&f, &id).stage, Stage::Ready);
}

// --- This repo's own setting --------------------------------------------------------------------

#[test]
fn this_repos_sbxm_task_toml_sets_the_pr_sink() {
    let config =
        sbxm::task::config::TaskConfig::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert_eq!(config.sink, sbxm::task::config::Sink::Pr);
}

// --- A `task run` list (decision 193) -----------------------------------------------------------

#[test]
fn a_list_refuses_a_later_specs_closed_issue_before_the_first_target_starts() {
    use sbxm::commands::task_run::{ListOptions, Target as RunTarget, run_list};
    let f = pr_fixture();
    let first = f.env.tmp.path().join("first.md");
    fs::write(&first, "Fixes: #435\n").unwrap();
    let second = f.env.tmp.path().join("second.md");
    fs::write(&second, "# Second\nFixes: #436\n").unwrap();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let github = FakeGitHub::default()
        .with_issue_text(issue_text(435))
        .with_issue_text(closed_issue(436));
    let opts = ListOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(&f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_list(
        &f.env.config_dir(),
        &opts,
        &[RunTarget::Spec(first), RunTarget::Spec(second)],
        None,
        &backend,
        &github,
        &Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("second.md") && message.contains("#436"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}
