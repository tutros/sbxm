//! Issue 485: `task rm --pr N` removes the PR's review task and every fix task of it, whatever
//! head each was made for, keeping (and naming) one whose commits were never pushed unless
//! `--force`. Each boundary case of the issue is a test here.

use crate::common;

use common::fix_task::*;
use sbxm::backend::FakeBackend;
use sbxm::commands::task_rm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::finish::finish_fix;

#[test]
fn rm_pr_removes_an_interrupted_fix_task_with_no_commits_and_its_sandbox() {
    let f = config();
    let head = new_head(&f);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let id = interrupted_fix(&f, &head, &backend);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    assert!(!meta(&f, &id).exists());
    assert!(!workspace(&f, &id).exists());
    let prompts = confirm.prompts.borrow();
    let [prompt] = prompts.as_slice() else {
        panic!("{prompts:?}");
    };
    assert!(prompt.contains(&format!("task {id}")), "{prompt}");
    assert!(
        prompt.contains(&format!("  sandbox sbxm-task-{id}-")),
        "{prompt}"
    );
    assert!(
        prompt.contains(&meta(&f, &id).display().to_string()),
        "{prompt}"
    );
    assert!(
        backend
            .log()
            .iter()
            .any(|call| call.starts_with("rm ") && call.contains(&id)),
        "{:?}",
        backend.log()
    );
}

#[test]
fn rm_pr_lists_and_removes_the_review_task_and_fix_tasks_of_two_heads_with_one_question() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let old = interrupted_fix(&f, &new_head(&f), &backend);
    let current = interrupted_fix(&f, &new_head(&f), &backend);
    let review = review_task(&f);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    let prompts = confirm.prompts.borrow();
    let [prompt] = prompts.as_slice() else {
        panic!("{prompts:?}");
    };
    for id in ["pr-7", &old, &current] {
        assert!(prompt.contains(id), "{id}: {prompt}");
    }
    assert!(prompt.contains("3 tasks"), "{prompt}");
    assert!(!review.exists());
    for id in [&old, &current] {
        assert!(!meta(&f, id).exists(), "{id}");
        assert!(!workspace(&f, id).exists(), "{id}");
    }
}

#[test]
fn rm_pr_keeps_and_names_a_fix_task_whose_commits_were_never_pushed_and_removes_the_others() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let old = interrupted_fix(&f, &new_head(&f), &backend);
    let unpushed = ready_fix(&f, &new_head(&f));
    let review = review_task(&f);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    let message = format!("{:#}", out.result.unwrap_err());
    let repo_git = meta(&f, &unpushed).join("repo.git");
    assert!(
        message.contains(&format!("task {unpushed}")) && message.contains("--force"),
        "{message}"
    );
    assert!(
        message.contains(&repo_git.display().to_string()),
        "{message}"
    );
    // Named before the question too, and left whole.
    let prompts = confirm.prompts.borrow();
    let [prompt] = prompts.as_slice() else {
        panic!("{prompts:?}");
    };
    assert!(
        prompt.contains(&format!("kept: task {unpushed}")),
        "{prompt}"
    );
    assert!(repo_git.is_dir());
    assert!(meta(&f, &unpushed).join("task.json").is_file());
    assert!(workspace(&f, &unpushed).exists());
    // The others went.
    assert!(!review.exists());
    assert!(!meta(&f, &old).exists());
    assert!(
        out.out
            .contains(&format!("removed {}", meta(&f, &old).display()))
    );
}

#[test]
fn rm_pr_force_removes_a_fix_task_whose_commits_were_never_pushed() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let unpushed = ready_fix(&f, &new_head(&f));
    let confirm = yes_on_a_terminal();

    let out = rm(&f, true, &backend, &confirm);

    out.result.unwrap();
    assert!(!meta(&f, &unpushed).exists());
    assert!(!workspace(&f, &unpushed).exists());
}

#[test]
fn rm_pr_with_only_unpushed_fix_tasks_removes_nothing_and_asks_nothing() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let unpushed = ready_fix(&f, &new_head(&f));
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains(&format!("task {unpushed}")), "{message}");
    assert!(confirm.prompts.borrow().is_empty());
    assert!(meta(&f, &unpushed).join("task.json").is_file());
}

#[test]
fn rm_pr_removes_a_fix_task_whose_push_is_done() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let pushed = ready_fix(&f, &new_head(&f));
    finish_fix(&f.env.base_dir(), &pushed, &FakeGitHub::default(), &Gone).unwrap();
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    assert!(!meta(&f, &pushed).exists());
    assert!(!workspace(&f, &pushed).exists());
}

#[test]
fn rm_pr_removes_the_folders_of_a_fix_task_whose_sandbox_is_already_gone() {
    let f = config();
    let made_in = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let id = interrupted_fix(&f, &new_head(&f), &made_in);
    // Another backend: it has no sandbox of the task.
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    assert!(!meta(&f, &id).exists());
    assert!(!workspace(&f, &id).exists());
}

#[test]
fn rm_pr_with_only_a_review_task_removes_it_as_before() {
    let f = config();
    let review = review_task(&f);
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    assert!(!review.exists());
    let prompts = confirm.prompts.borrow();
    let [prompt] = prompts.as_slice() else {
        panic!("{prompts:?}");
    };
    assert!(prompt.contains("Permanently delete task pr-7:"), "{prompt}");
}

#[test]
fn rm_pr_with_no_task_of_the_pr_names_both_kinds_it_looked_for() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic"]);
    let confirm = yes_on_a_terminal();

    let out = rm(&f, false, &backend, &confirm);

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(
        message.contains("no task pr-7 or fix-7-<head>"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn rm_pr_refuses_everything_while_a_fix_task_is_still_running() {
    let f = config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let id = interrupted_fix(&f, &new_head(&f), &backend);
    let review = review_task(&f);
    let confirm = yes_on_a_terminal();
    let mut out = Vec::new();

    let result = task_rm::run(
        &f.env.config_dir(),
        &task_rm::Options {
            target: task_rm::Target::Pr(7),
            yes: true,
            force: true,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
            forget_ambiguous: false,
        },
        &backend,
        &FakeGitHub::default(),
        &Alive,
        &confirm,
        &mut out,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(&format!("task {id} is running")),
        "{message}"
    );
    assert!(review.exists());
    assert!(meta(&f, &id).join("task.json").is_file());
}
