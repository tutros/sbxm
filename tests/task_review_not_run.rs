//! Issue 545 (#460 guardrail 1): a review round records the checks its review couldn't run (its
//! `## Not run` section), and a review without a readable list warns that it is unknown.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, fixture_with, ok, play_reviews, play_tasks,
    worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_review::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, ReviewResult};

const UNKNOWN_WARNING: &str = "its not-run list is unknown";

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn backend(f: &Fixture, review: &str) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![review.to_owned()]);
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
        location: None,
        identity: None,
    }
}

/// Reviews the worked task with `review` as the reviewer's text; returns the recorded review
/// and what was warned.
fn review_with(review: &str) -> (ReviewResult, String) {
    let f = config();
    let backend = backend(&f, review);
    worked_task(&f, &backend);
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    run(
        &f.env.config_dir(),
        &options(&f),
        &backend,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )
    .unwrap();
    let dir = record::task_dir(&f.env.base_dir(), "issue-41");
    let rec = record::read(&dir.join("task.json")).unwrap();
    (rec.review.unwrap(), String::from_utf8(warn).unwrap())
}

#[test]
fn a_none_list_is_recorded_empty_without_a_warning() {
    let (review, warn) = review_with("Must-fix findings: 0\n\nRisk: low\n\n## Not run\n\n- none\n");
    assert_eq!(review.not_run, Some(Vec::new()));
    assert!(!warn.contains(UNKNOWN_WARNING), "{warn}");
}

#[test]
fn a_listed_check_is_recorded() {
    let (review, warn) = review_with(
        "Must-fix findings: 0\n\nRisk: low\n\n## Not run\n\n- Pester tests: no pwsh here\n",
    );
    assert_eq!(
        review.not_run,
        Some(vec!["Pester tests: no pwsh here".to_owned()])
    );
    assert!(!warn.contains(UNKNOWN_WARNING), "{warn}");
}

#[test]
fn a_review_without_the_section_records_unknown_and_warns() {
    let (review, warn) = review_with("Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n");
    assert_eq!(review.not_run, None);
    assert!(warn.contains(UNKNOWN_WARNING), "{warn}");
    assert!(warn.contains("## Not run"), "{warn}");
}

#[test]
fn a_record_from_before_the_list_reads_as_unknown() {
    let json = r#"{"round":1,"must_fix":0,"full":true,"repeat":false,"risk":"low"}"#;
    let review: ReviewResult = serde_json::from_str(json).unwrap();
    assert_eq!(review.not_run, None);
}
