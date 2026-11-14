//! Issue 485: `task resume --pr N` resumes the fix task made for the PR's current head from its
//! recorded stage, as `--issue` does, and pushes its result as `task fix` would; it refuses,
//! naming the way on, when only fix tasks of older heads exist.

use crate::common;

use common::fix_task::*;
use common::git;
use common::task_fixture::{Fixture, ctx, source};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_resume;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline;
use sbxm::task::pr_fix::review_to_fix;
use sbxm::task::record::ProcessProbe;
use sbxm::task::record::{self, Stage, Status};

fn resume(
    f: &Fixture,
    backend: &FakeBackend,
    github: &FakeGitHub,
    probe: &dyn ProcessProbe,
) -> Out {
    resume_rounds(f, None, backend, github, probe)
}

fn resume_rounds(
    f: &Fixture,
    rounds: Option<u32>,
    backend: &FakeBackend,
    github: &FakeGitHub,
    probe: &dyn ProcessProbe,
) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_resume::run(
        &f.env.config_dir(),
        &task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: task_resume::Target::Pr(7),
            rounds,
            should_fix: false,
            disk: Default::default(),
            identity: None,
            forget_ambiguous: false,
        },
        backend,
        github,
        probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

#[test]
fn resume_pr_resumes_the_interrupted_fix_task_of_the_current_head_and_pushes_its_result() {
    let f = config();
    let head = new_head(&f);
    let id = record::fix_id(7, &head).unwrap();
    let backend = worker_backend(&f, &id);
    interrupted_fix(&f, &head, &backend);
    let github = github_at(&head);

    let out = resume(&f, &backend, &github, &Gone);

    out.result.unwrap();
    assert!(
        out.out
            .contains(&format!("{id}: resuming from working (running)")),
        "{}",
        out.out
    );
    assert!(
        out.out.contains(&format!("{id}: worker claude")),
        "{}",
        out.out
    );
    let tip = origin_head(&f);
    assert_ne!(tip, head);
    git(&f.origin, &["merge-base", "--is-ancestor", &head, &tip]);
    assert!(
        out.out.contains(&format!(
            "{id}: pushed 1 commit(s) onto PR #7's branch feature-x, now at {tip}"
        )),
        "{}",
        out.out
    );
    let task = record::read(&meta(&f, &id).join("task.json")).unwrap();
    assert_eq!(task.stage, Stage::Finished);
}

#[test]
fn resume_pr_rebuilds_a_gone_sandbox_as_resume_issue_does() {
    let f = config();
    let head = new_head(&f);
    let id = record::fix_id(7, &head).unwrap();
    interrupted_fix(
        &f,
        &head,
        &FakeBackend::with_secrets(&["anthropic", "openai"]),
    );
    // A backend without the task's sandbox.
    let backend = worker_backend(&f, &id);
    let github = github_at(&head);

    let out = resume(&f, &backend, &github, &Gone);

    out.result.unwrap();
    assert!(
        out.out.contains(&format!(
            "{id}: the worker's sandbox was gone; made it again"
        )),
        "{}",
        out.out
    );
    assert_ne!(origin_head(&f), head);
}

#[test]
fn resume_pr_picks_the_current_heads_fix_task_of_two() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let old = interrupted_fix(&f, &new_head(&f), &backend);
    let head = new_head(&f);
    let id = record::fix_id(7, &head).unwrap();
    let backend = worker_backend(&f, &id);
    interrupted_fix(&f, &head, &backend);
    let github = github_at(&head);

    let out = resume(&f, &backend, &github, &Gone);

    out.result.unwrap();
    assert!(
        out.out.contains(&format!("{id}: resuming from")),
        "{}",
        out.out
    );
    assert!(!out.out.contains(&old), "{}", out.out);
    let left = record::read(&meta(&f, &old).join("task.json")).unwrap();
    assert_eq!((left.stage, left.status), (Stage::Working, Status::Running));
}

#[test]
fn resume_pr_with_only_an_older_heads_fix_task_refuses_and_names_rm_then_fix() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let old = interrupted_fix(&f, &new_head(&f), &backend);
    let head = new_head(&f);
    let github = github_at(&head);
    let before = backend.log().len();

    let out = resume(&f, &backend, &github, &Gone);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains(&old), "{message}");
    assert!(message.contains(&head), "{message}");
    let rm_at = message.find("`sbxm task rm --pr 7`").expect(&message);
    let fix_at = message.find("`sbxm task fix --pr 7`").expect(&message);
    assert!(rm_at < fix_at, "{message}");
    assert_eq!(backend.log().len(), before, "{:?}", backend.log());
    let left = record::read(&meta(&f, &old).join("task.json")).unwrap();
    assert_eq!((left.stage, left.status), (Stage::Working, Status::Running));
}

#[test]
fn resume_pr_is_refused_while_the_fix_tasks_sbxm_process_still_runs() {
    let f = config();
    let head = new_head(&f);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let id = interrupted_fix(&f, &head, &backend);
    let github = github_at(&head);
    let before = backend.log().len();

    let out = resume(&f, &backend, &github, &Alive);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains(&format!("task {id} is still running in sbxm process")),
        "{message}"
    );
    assert_eq!(backend.log().len(), before, "{:?}", backend.log());
}

#[test]
fn resume_pr_with_no_fix_task_says_so_and_names_task_fix() {
    let f = config();
    let head = new_head(&f);
    let backend = FakeBackend::with_secrets(&["anthropic"]);

    let out = resume(&f, &backend, &github_at(&head), &Gone);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("no fix task of PR #7"), "{message}");
    assert!(message.contains("`sbxm task fix --pr 7`"), "{message}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

/// A fix task that reached `ready` with nothing unresolved, but whose push never happened (its
/// process was gone before it): only the push is left, and `--pr` makes it.
#[test]
fn resume_pr_pushes_a_ready_fix_task_whose_push_never_happened() {
    let f = config();
    let head = new_head(&f);
    let id = ready_fix(&f, &head);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let out = resume(&f, &backend, &github_at(&head), &Gone);

    out.result.unwrap();
    assert!(
        out.out.contains(&format!(
            "{id}: resuming from ready: its push onto PR #7's branch"
        )),
        "{}",
        out.out
    );
    let tip = origin_head(&f);
    assert_ne!(tip, head);
    assert!(
        out.out.contains(&format!(
            "{id}: pushed 1 commit(s) onto PR #7's branch feature-x, now at {tip}"
        )),
        "{}",
        out.out
    );
    // Nothing ran again: no sandbox was made or used.
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn resume_pr_rounds_is_refused_for_a_ready_fix_task_with_nothing_unresolved() {
    let f = config();
    let head = new_head(&f);
    let id = ready_fix(&f, &head);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let out = resume_rounds(&f, Some(1), &backend, &github_at(&head), &Gone);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains(&format!("task {id} is ready")),
        "{message}"
    );
    assert!(message.contains("`sbxm task resume --pr 7`"), "{message}");
    assert_eq!(origin_head(&f), head);
}

// --- task fix --pr N's refusal ---

#[test]
fn task_fix_on_a_head_with_a_fix_task_names_resume_and_rm_instead_of_hand_deletion() {
    let f = config();
    let head = new_head(&f);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let id = interrupted_fix(&f, &head, &backend);
    let pr = pr7(&head);
    let review = review_to_fix(&pr, "o/r", None, "tutros", &[posted_review(&head)]).unwrap();
    let github = FakeGitHub::default();
    let source = source(&f);
    let context = ctx(&f, &source, &backend, &github);

    let message = format!(
        "{:#}",
        pipeline::prepare_fix(&context, &pr, &review, Location::Local).unwrap_err()
    );

    assert!(message.contains(&format!("task {id} exists")), "{message}");
    assert!(message.contains("`sbxm task resume --pr 7`"), "{message}");
    assert!(message.contains("`sbxm task rm --pr 7`"), "{message}");
    assert!(!message.contains("by hand"), "{message}");
}
