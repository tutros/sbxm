//! Issue #444 (PR #482 review round, M-1): a review right after its collect lists the task's
//! changed paths once, keeping them for the commit the collect left the branch at; a fix round's
//! commits are never judged by the paths kept from before them.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play, play_reviews, source,
    worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;
use sbxm::task::record;
use sbxm::task::risk::Level;

const FINDING: &str = "Must-fix findings: 1\n\nRisk: low\n\n\
    ## Must fix\n\n\
    ### M-1 - a.txt does the wrong thing\n\n\
    **Where:** `a.txt:1`\n\
    **What happens:** it returns the wrong value\n\
    **Why it matters:** decision 1\n\
    **Fix:** return the right value\n";
const CLEAN: &str = "Must-fix findings: 0\n\nRisk: low\n\nNothing found.\n";

fn done(file: &str) -> Play {
    Play {
        commits: vec![file.to_owned()],
        result_md: Some(b"done\n".to_vec()),
        bundle_bytes: None,
    }
}

/// The worker commits `a.txt`, and its fix round commits `Cargo.lock` (a dependency manifest,
/// which the path rules floor to medium).
fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    let first = play(&workspace, "main", "issue-41", done("a.txt"));
    let fix = play(&workspace, "main", "issue-41", done("Cargo.lock"));
    let runs = AtomicUsize::new(0);
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            let worker_run =
                !sandbox.contains("-review-") && spec.argv.iter().any(|a| a.contains("claude"));
            if worker_run && runs.fetch_add(1, Ordering::SeqCst) > 0 {
                fix(sandbox, spec);
            } else {
                first(sandbox, spec);
            }
            reviewer(sandbox, spec);
        })
}

#[test]
fn a_fix_rounds_new_paths_reach_the_floor_of_the_review_after_it() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    );
    let backend = backend(&f, &[FINDING, CLEAN, CLEAN]);
    let mut prepared = worked_task(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);

    pipeline::review_issue(&ctx(&f, &source, &backend, &github).env(), &mut prepared).unwrap();

    let saved =
        record::read(&record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")).unwrap();
    assert_eq!(saved.round, 1, "{saved:?}");
    let review = saved.review.unwrap();
    assert_eq!(review.risk, Level::Medium, "{:?}", review.risk_reasons);
    assert!(
        review.risk_reasons.iter().any(|r| r.contains("Cargo.lock")),
        "{:?}",
        review.risk_reasons
    );
}
