//! M2b slice 10, step 7: `sbxm task start --restart` discards the existing task (same
//! confirmation and removal as `task rm`) and starts it again (spec §4, decision 153).

mod common;

use std::cell::RefCell;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, issue_text, ok, open_issue, play_tasks,
    source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, Options, Restart};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{IssueText, PrInfo, PrState};
use sbxm::task::record::{self, Status};
use sbxm::task::repo::Identity;

struct FakeConfirm {
    interactive: bool,
    answer: bool,
    prompts: RefCell<Vec<String>>,
}

impl FakeConfirm {
    fn new(interactive: bool, answer: bool) -> Self {
        Self {
            interactive,
            answer,
            prompts: RefCell::new(Vec::new()),
        }
    }
}

impl Confirm for FakeConfirm {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.prompts.borrow_mut().push(prompt.to_owned());
        Ok(self.answer)
    }
}

fn options(f: &Fixture) -> Options {
    Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
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

fn github() -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
}

fn go(
    f: &Fixture,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_start::run_with(
        &f.env.config_dir(),
        opts,
        restart,
        backend,
        github,
        &Probe,
        &mut out,
        &mut Vec::new(),
    );
    (result, String::from_utf8(out).unwrap())
}

/// A task for issue 41 that has already worked once.
fn started(f: &Fixture) {
    let (result, _) = go(f, &options(f), None, &playing(f), &github());
    result.unwrap();
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

#[test]
fn restart_asks_discards_the_old_task_and_starts_a_new_one() {
    let f = fixture();
    started(&f);
    // Records can't tell the runs apart (same second, same fake process), so mark the old folder.
    std::fs::write(meta(&f).join("old-run.marker"), "old").unwrap();
    let confirm = FakeConfirm::new(true, true);
    let b = playing(&f);

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    result.unwrap();
    let prompts = confirm.prompts.borrow();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&meta(&f).display().to_string()),
        "{}",
        prompts[0]
    );
    assert_eq!(b.removes(), vec!["sbxm-task-issue-41-claude"]);
    assert!(
        out.contains("removed sandbox sbxm-task-issue-41-claude"),
        "{out}"
    );
    assert!(
        out.contains("issue-41: worker claude (default model) completed"),
        "{out}"
    );
    assert_eq!(b.creates().len(), 1);
    assert!(
        !meta(&f).join("old-run.marker").exists(),
        "the old folder was reused"
    );
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "a fresh record"
    );
}

#[test]
fn declining_keeps_the_old_task_and_starts_nothing() {
    let f = fixture();
    started(&f);
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, false),
            yes: false,
        }),
        &b,
        &github(),
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("cancelled"));
    assert!(meta(&f).join("task.json").exists());
    assert!(b.creates().is_empty() && b.removes().is_empty());
}

#[test]
fn without_a_terminal_restart_needs_yes_and_yes_is_enough() {
    let f = fixture();
    started(&f);
    let confirm = FakeConfirm::new(false, false);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &playing(&f),
        &github(),
    );
    assert!(format!("{:#}", result.unwrap_err()).contains("pass --yes"));

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: true,
        }),
        &playing(&f),
        &github(),
    );
    result.unwrap();
    assert!(confirm.prompts.borrow().is_empty());
}

/// A `Confirm` that removes the worker's secret from the backend right when the user says yes,
/// modeling it vanishing while the confirmation prompt is pending (decision 185, issue #207).
struct SecretVanishesThenYes<'a> {
    backend: &'a FakeBackend,
}

impl Confirm for SecretVanishesThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        self.backend.set_secrets(&[]);
        Ok(true)
    }
}

/// Decision 185 (issue #207): a start that fails after the confirmed discard and before the new
/// task's preparation is complete leaves the old task exactly as it was, instead of losing it.
#[test]
fn restart_puts_the_old_task_back_if_the_worker_secret_vanishes_during_confirmation() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let confirm = SecretVanishesThenYes { backend: &b };

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    assert!(result.is_err(), "{out}");
    assert!(out.contains("anthropic"), "{out}");
    assert!(out.contains("put back"), "{out}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "the old task is back"
    );
    assert!(meta(&f).join("repo.git").is_dir(), "the old task is back");
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "nothing left set aside"
    );
}

/// A `Confirm` that removes `config.toml` right when the user says yes, modeling it vanishing (or
/// changing) while the confirmation prompt is pending (decision 185, issue #207 review M-1).
struct ConfigVanishesThenYes {
    config_toml: std::path::PathBuf,
}

impl Confirm for ConfigVanishesThenYes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        std::fs::remove_file(&self.config_toml).unwrap();
        Ok(true)
    }
}

/// Issue #207 review M-1: the same guarantee as the worker-secret case above, but for the global
/// config read again after the confirmed discard instead of before it. Before the fix, `run_with`
/// reloaded `GlobalConfig` for its own use right after `discard_existing` had already set the old
/// task aside; a failure there returned straight out of `run_with`, bypassing the restore path
/// that every other post-discard failure goes through, and left the old task stranded under
/// `tasks-replaced/`.
#[test]
fn restart_puts_the_old_task_back_if_global_config_vanishes_during_confirmation() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let confirm = ConfigVanishesThenYes {
        config_toml: f.env.config_dir().join("config.toml"),
    };

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    assert!(result.is_err(), "{out}");
    assert!(out.contains("put back"), "{out}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "the old task is back"
    );
    assert!(meta(&f).join("repo.git").is_dir(), "the old task is back");
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "nothing left set aside"
    );
}

/// Decision 185 point 3: the old task's set-aside copy is gone for good once the new task's
/// preparation completes, even when the worker that runs next fails.
#[test]
fn restart_settles_the_old_task_even_when_the_new_worker_then_fails() {
    let f = fixture();
    started(&f);
    let confirm = FakeConfirm::new(true, true);
    let failing_worker = backend().with_exec_output_matching(
        "claude",
        sbxm::backend::ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &failing_worker,
        &github(),
    );

    assert!(
        result.is_err(),
        "the new worker's failure still fails the command: {out}"
    );
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "the new task exists: its preparation completed"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "settled, not left set aside, because preparation completed"
    );
}

/// Issue #207 review M-1: when the worker's `exec` call itself fails (not the agent reporting
/// failure, but `run_worker` returning `Err`), the task's preparation still completed, so the old
/// copy must already be settled, the same as a worker that runs and then fails. Before the fix,
/// this `Err` was read as "preparation never completed" and the already-populated new task was
/// mistaken for the old one to restore, which then failed because the new task's folders were
/// already there.
#[test]
fn restart_settles_the_old_task_and_does_not_try_to_restore_it_when_the_worker_cannot_run_at_all() {
    let f = fixture();
    started(&f);
    let confirm = FakeConfirm::new(true, true);
    let cannot_exec = backend().with_failing_exec_matching("claude");

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &cannot_exec,
        &github(),
    );

    assert!(result.is_err(), "{out}");
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "the new task exists: its preparation completed"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "settled, not left set aside, because preparation completed"
    );
    assert!(
        out.contains("issue-41: failed:"),
        "the worker error is reported plainly: {out}"
    );
    assert!(
        !out.contains("put back"),
        "preparation completed, so nothing was restored: {out}"
    );
}

/// Decision 185(b), issue #207 review M-1: the old task's set-aside copy is gone for good the
/// moment the new task's preparation completes (its folder, kits and sandbox exist and
/// `task.json` is written) — before its worker runs at all, not only once the worker (and
/// possibly its gates) have also finished. A gate holds the worker's one `exec` call, so the
/// assertions run while the command is still blocked there.
#[test]
fn restart_settles_the_old_task_before_its_worker_runs() {
    let f = fixture();
    started(&f);
    let (gated, gate) = playing(&f).with_exec_gate();

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let confirm = FakeConfirm::new(true, true);
            go(
                &f,
                &options(&f),
                Some(&Restart {
                    confirm: &confirm,
                    yes: false,
                }),
                &gated,
                &github(),
            )
        });

        gate.wait_for_blocked(1);
        assert!(
            record::read(&meta(&f).join("task.json")).is_ok(),
            "the new task's preparation has completed"
        );
        assert!(
            !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
            "settled before the worker ran, not left set aside"
        );
        gate.open();

        let (result, out) = handle.join().unwrap();
        result.unwrap_or_else(|e| panic!("{e:#}: {out}"));
    });
}

/// Decision 185 point 4: a `--restart` of a task with a leftover copy under `tasks-replaced/` is
/// refused before the prompt, naming the folder and the two ways out; nothing is deleted or
/// created.
#[test]
fn a_restart_refuses_before_the_prompt_while_a_leftover_copy_exists() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    // An earlier `--restart` that was interrupted after discarding but before settling.
    sbxm::task::finish::set_aside(
        &f.env.base_dir(),
        "issue-41",
        true,
        &b,
        &Probe,
        &FakeConfirm::new(true, true),
        &mut Vec::new(),
    )
    .unwrap();
    let (removes_before, creates_before) = (b.removes().len(), b.creates().len());
    let confirm = FakeConfirm::new(true, true);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("tasks-replaced"), "{message}");
    assert!(message.contains("move it back"), "{message}");
    assert!(message.contains("delete it"), "{message}");
    assert!(
        confirm.prompts.borrow().is_empty(),
        "nothing is asked while a leftover exists"
    );
    assert_eq!(b.removes().len(), removes_before, "nothing more removed");
    assert_eq!(b.creates().len(), creates_before, "nothing created");
}

#[test]
fn a_running_task_is_not_restarted() {
    let f = fixture();
    started(&f);
    let mut task = record::read(&meta(&f).join("task.json")).unwrap();
    task.status = Status::Running;
    record::write(&meta(&f), &task).unwrap();
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &github(),
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("is running"));
    assert!(b.creates().is_empty() && b.removes().is_empty());
    assert!(meta(&f).join("task.json").exists());
}

#[test]
fn a_closed_issue_is_not_restarted_because_that_would_only_delete_the_work() {
    let f = fixture();
    started(&f);
    let closed = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(IssueText {
            state: "CLOSED".into(),
            ..issue_text(41)
        })
        .with_open_issues(vec![]);
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &closed,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("issue #41 is closed") && message.contains("task rm"),
        "{message}"
    );
    assert!(meta(&f).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn a_bad_flag_is_found_before_anything_is_deleted() {
    let f = fixture();
    started(&f);
    let mut opts = options(&f);
    opts.time_limit = Some("soon".into());
    let b = playing(&f);

    let (result, _) = go(
        &f,
        &opts,
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &github(),
    );

    assert!(result.is_err());
    assert!(meta(&f).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn restart_without_issues_is_refused_and_a_task_that_does_not_exist_just_starts() {
    let f = fixture();
    let confirm = FakeConfirm::new(true, true);
    let restart = Restart {
        confirm: &confirm,
        yes: false,
    };
    let mut none = options(&f);
    none.issues.clear();
    none.workers = Some(1);
    let (result, _) = go(&f, &none, Some(&restart), &playing(&f), &github());
    assert!(format!("{:#}", result.unwrap_err()).contains("--restart needs"));

    let (result, out) = go(&f, &options(&f), Some(&restart), &playing(&f), &github());

    result.unwrap();
    assert!(
        confirm.prompts.borrow().is_empty(),
        "nothing to discard, nothing to ask"
    );
    assert!(
        out.contains("issue-41: worker claude (default model) completed"),
        "{out}"
    );
}

#[test]
fn without_restart_an_existing_task_still_refuses() {
    let f = fixture();
    started(&f);

    let (result, out) = go(&f, &options(&f), None, &playing(&f), &github());

    let text = format!("{:#}{out}", result.unwrap_err());
    assert!(
        text.contains("already has a task") || text.contains("--restart"),
        "{text}"
    );
    assert!(meta(&f).join("task.json").exists());
}

// ---- Slice 10 review, must-fix 1: a restart must not delete what it cannot start again ----

fn github_of(issues: &[(u32, &[&str], &str)]) -> FakeGitHub {
    let mut gh = FakeGitHub::default().with_default_branch("main");
    let mut open = Vec::new();
    for (number, labels, body) in issues {
        gh = gh.with_issue_text(issue_text(*number));
        open.push(open_issue(*number, labels, body));
    }
    gh.with_open_issues(open)
}

fn options_for(f: &Fixture, issues: &[u32]) -> Options {
    let mut opts = options(f);
    opts.issues = issues.to_vec();
    opts
}

fn meta_of(f: &Fixture, number: u32) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), &format!("issue-{number}"))
}

/// Tasks for issues 41 and 42 that have both worked once.
fn started_both(f: &Fixture) {
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);
    let (result, _) = go(f, &options_for(f, &[41, 42]), None, &playing(f), &gh);
    result.unwrap();
}

#[test]
fn restarting_an_issue_that_is_now_a_question_deletes_nothing() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let confirm = FakeConfirm::new(true, true);
    let gh = github_of(&[(41, &["question"], "")]);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: true,
        }),
        &b,
        &gh,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("#41") && message.contains("question"),
        "{message}"
    );
    assert!(
        message.contains("--restart") || message.contains("restart"),
        "{message}"
    );
    assert!(
        meta(&f).join("task.json").exists(),
        "the task must still be there"
    );
    assert!(b.removes().is_empty() && b.creates().is_empty());
    assert!(
        confirm.prompts.borrow().is_empty(),
        "nothing is asked for what cannot happen"
    );
}

#[test]
fn restarting_an_issue_that_is_now_blocked_deletes_nothing() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let gh = github_of(&[
        (41, &["should-fix"], "**Depends on:** #5"),
        (5, &["should-fix"], ""),
    ]);

    let (result, _) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &gh,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("#41") && message.contains("blocked by open #5"),
        "{message}"
    );
    assert!(meta(&f).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn if_one_of_several_cannot_restart_none_of_them_is_deleted() {
    let f = fixture();
    started_both(&f);
    let b = playing(&f);
    // #41 is fine; #42 has become a question.
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["question"], "")]);

    let (result, _) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &gh,
    );

    assert!(result.is_err());
    assert!(meta_of(&f, 41).join("task.json").exists());
    assert!(meta_of(&f, 42).join("task.json").exists());
    assert!(b.removes().is_empty() && b.creates().is_empty());
}

#[test]
fn several_restarts_are_confirmed_once_for_all_and_declining_deletes_none() {
    let f = fixture();
    started_both(&f);
    let b = playing(&f);
    let confirm = FakeConfirm::new(true, false);
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);

    let (result, _) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &gh,
    );

    assert!(format!("{:#}", result.unwrap_err()).contains("cancelled"));
    let prompts = confirm.prompts.borrow();
    assert_eq!(
        prompts.len(),
        1,
        "one question for all of them: {prompts:?}"
    );
    assert!(
        prompts[0].contains("issue-41") && prompts[0].contains("issue-42"),
        "{}",
        prompts[0]
    );
    assert!(meta_of(&f, 41).join("task.json").exists());
    assert!(meta_of(&f, 42).join("task.json").exists());
    assert!(b.removes().is_empty());
}

#[test]
fn several_restarts_accepted_once_delete_and_start_them_all() {
    let f = fixture();
    started_both(&f);
    std::fs::write(meta_of(&f, 41).join("old-run.marker"), "old").unwrap();
    let b = playing(&f);
    let confirm = FakeConfirm::new(true, true);
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);

    let (result, out) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &gh,
    );

    result.unwrap();
    assert_eq!(confirm.prompts.borrow().len(), 1);
    assert_eq!(b.creates().len(), 2);
    assert!(
        out.contains("issue-41: worker claude (default model) completed")
            && out.contains("issue-42: worker claude (default model) completed"),
        "{out}"
    );
    assert!(
        !meta_of(&f, 41).join("old-run.marker").exists(),
        "the old folder was reused"
    );
    assert!(record::read(&meta_of(&f, 41).join("task.json")).is_ok());
}

// ---- Decision 185 (issue #207): several restarts, each settled on its own ----

/// Decision 185 point 5: `task start --restart --issue 41 --issue 42` where #42's preparation
/// fails settles #41 (its old copy is gone) and restores #42 (its old copy is back), the command
/// exits with an error, and the output says per task which of the two happened.
#[test]
fn restart_batch_settles_the_one_that_starts_and_restores_the_one_whose_preparation_fails() {
    let f = fixture();
    started_both(&f);
    let b = playing(&f).with_failing_create_for("sbxm-task-issue-42-claude");
    let confirm = FakeConfirm::new(true, true);
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);

    let (result, out) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &gh,
    );

    assert!(result.is_err(), "{out}");
    assert!(
        record::read(&meta_of(&f, 41).join("task.json")).is_ok(),
        "issue 41's new task is in place"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "nothing left set aside for issue 41"
    );
    assert!(
        meta_of(&f, 42).join("repo.git").is_dir(),
        "issue 42's old task is back"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-42"),
        "nothing left set aside for issue 42"
    );
    assert!(
        out.contains("issue-42") && out.contains("put back"),
        "{out}"
    );
}

/// A `Confirm` that drops an issue from the open list right when the user says yes, so
/// `pipeline::choose_issues`' later call (after `check_restartable` already approved it) never
/// picks it again: its `--restart` never even reaches `pipeline::start`.
struct DropIssueThenYes<'a> {
    github: &'a FakeGitHub,
    keep: Vec<u32>,
}

impl Confirm for DropIssueThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        let kept: Vec<_> = self
            .keep
            .iter()
            .map(|&n| open_issue(n, &["should-fix"], ""))
            .collect();
        self.github.set_open_issues(kept);
        Ok(true)
    }
}

/// Decision 185 point 5: a restarted issue that the selection doesn't pick again this time
/// around never reaches `pipeline::start`, so it never gets a report; it is restored exactly as
/// one whose own preparation failed is.
#[test]
fn restart_batch_restores_a_task_the_command_never_got_to_starting_again() {
    let f = fixture();
    started_both(&f);
    let b = playing(&f);
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);
    let confirm = DropIssueThenYes {
        github: &gh,
        keep: vec![41],
    };

    let (result, out) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &gh,
    );

    assert!(result.is_err(), "{out}");
    assert!(
        record::read(&meta_of(&f, 41).join("task.json")).is_ok(),
        "issue 41's new task is in place"
    );
    assert!(!sbxm::task::finish::has_leftover(
        &f.env.base_dir(),
        "issue-41"
    ));
    assert!(
        meta_of(&f, 42).join("repo.git").is_dir(),
        "issue 42's old task is back"
    );
    assert!(!sbxm::task::finish::has_leftover(
        &f.env.base_dir(),
        "issue-42"
    ));
}

// ---- Issue #83 review M-2: a PR that can't be continued is refused before anything is deleted ----

/// Issue `number`, now a finding of PR 7 (`PR: #7` on its body's first line).
fn finding_of_pr_7(number: u32) -> IssueText {
    IssueText {
        text: format!(
            "title:\tFix {number}\nstate:\tOPEN\n--\nPR: #7\n\n**Acceptance criteria:** do it\n"
        ),
        ..issue_text(number)
    }
}

fn pr_7(state: PrState, fork: bool) -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: fork,
        state,
        title: "Add x".into(),
        body: String::new(),
        closing_issues: vec![],
    }
}

fn untouched(f: &Fixture, number: u32) {
    let meta = meta_of(f, number);
    assert!(meta.join("task.json").exists(), "#{number}: record deleted");
    assert!(
        meta.join("repo.git").is_dir(),
        "#{number}: repo.git deleted"
    );
    assert!(
        f.env
            .base_dir()
            .join("tasks")
            .join(format!("issue-{number}"))
            .is_dir(),
        "#{number}: workspace deleted"
    );
}

#[test]
fn restarting_an_issue_of_a_closed_merged_or_fork_pr_deletes_nothing() {
    for (state, fork, word) in [
        (PrState::Closed, false, "closed"),
        (PrState::Merged, false, "merged"),
        (PrState::Open, true, "fork"),
    ] {
        let f = fixture();
        started(&f);
        let b = playing(&f);
        let gh = FakeGitHub::default()
            .with_default_branch("main")
            .with_issue_text(finding_of_pr_7(41))
            .with_open_issues(vec![open_issue(41, &["should-fix"], "")])
            .with_pr(pr_7(state, fork));

        let (result, _) = go(
            &f,
            &options(&f),
            Some(&Restart {
                confirm: &FakeConfirm::new(true, true),
                yes: true,
            }),
            &b,
            &gh,
        );

        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains("PR #7") && message.contains(word),
            "{word}: {message}"
        );
        untouched(&f, 41);
        assert!(
            b.removes().is_empty() && b.creates().is_empty(),
            "{word}: the sandbox was touched"
        );
    }
}

#[test]
fn one_restart_with_an_uncontinuable_pr_keeps_every_task() {
    let f = fixture();
    started_both(&f);
    let b = playing(&f);
    // #41 is fine; #42 is now a finding of a merged PR.
    let gh = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(finding_of_pr_7(42))
        .with_open_issues(vec![
            open_issue(41, &["should-fix"], ""),
            open_issue(42, &["should-fix"], ""),
        ])
        .with_pr(pr_7(PrState::Merged, false));

    let (result, _) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &gh,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("#42") && message.contains("merged"),
        "{message}"
    );
    untouched(&f, 41);
    untouched(&f, 42);
    assert!(b.removes().is_empty() && b.creates().is_empty());
}

#[test]
fn a_new_issue_of_an_uncontinuable_pr_keeps_the_task_that_would_be_restarted() {
    let f = fixture();
    started(&f); // #41 has a task; #42 has none and is a finding of a merged PR
    let b = playing(&f);
    let gh = FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_issue_text(finding_of_pr_7(42))
        .with_open_issues(vec![
            open_issue(41, &["should-fix"], ""),
            open_issue(42, &["should-fix"], ""),
        ])
        .with_pr(pr_7(PrState::Merged, false));

    let (result, _) = go(
        &f,
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &FakeConfirm::new(true, true),
            yes: true,
        }),
        &b,
        &gh,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("#42") && message.contains("merged"),
        "{message}"
    );
    untouched(&f, 41);
    assert!(!meta_of(&f, 42).exists(), "a task for #42 was created");
    assert!(b.removes().is_empty() && b.creates().is_empty());
}

/// A `Write` that takes `self.1` lines and fails from then on.
struct FailsAfterLines(String, usize);

impl std::io::Write for FailsAfterLines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.matches('\n').count() >= self.1 {
            return Err(std::io::Error::other("screen output failed"));
        }
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// PR 219 review, M-2: screen output cannot interrupt a restart. Both old tasks are set aside,
/// then the screen stops taking output: the restart still runs to its end, nothing stays set
/// aside, and the command reports the output failure once it is done.
#[test]
fn a_batch_restart_runs_to_its_end_when_the_screen_stops_taking_output() {
    let f = fixture();
    started_both(&f);
    let confirm = FakeConfirm::new(true, true);
    let gh = github_of(&[(41, &["should-fix"], ""), (42, &["should-fix"], "")]);
    let mut out = FailsAfterLines(String::new(), 2);

    let result = task_start::run_with(
        &f.env.config_dir(),
        &options_for(&f, &[41, 42]),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &playing(&f),
        &gh,
        &Probe,
        &mut out,
        &mut Vec::new(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("screen output failed"), "{message}");
    for number in [41, 42] {
        assert!(
            record::read(&meta_of(&f, number).join("task.json")).is_ok(),
            "issue {number}'s task is in place: {message}"
        );
        assert!(
            !sbxm::task::finish::has_leftover(&f.env.base_dir(), &format!("issue-{number}")),
            "nothing left set aside for issue {number}: {message}"
        );
    }
}

/// A `Confirm` that, right when the user says yes, removes the worker's secret (so the old task
/// is put back) and makes the set-aside tree's `.sbxm` folder undeletable (`common::Undeletable`),
/// so deleting the tree after the restore fails (issue #224).
struct SecretVanishesAndCleanupFailsThenYes<'a> {
    backend: &'a FakeBackend,
    remnant: std::path::PathBuf,
    lock: RefCell<Option<common::Undeletable>>,
}

impl Confirm for SecretVanishesAndCleanupFailsThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        self.backend.set_secrets(&[]);
        let inner = self.remnant.join(".sbxm");
        std::fs::create_dir_all(inner.join("tasks")).unwrap();
        *self.lock.borrow_mut() = common::Undeletable::new(&inner);
        Ok(true)
    }
}

/// Issue #224: the old task is put back but its set-aside tree can't be deleted, and the screen
/// takes no output at all, so the warning can't be shown: the restore itself is unchanged (the
/// task is back, nothing counts as a leftover) and the command reports the output failure.
#[test]
fn a_restore_whose_cleanup_fails_is_unchanged_when_the_screen_fails() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let remnant = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");
    let confirm = SecretVanishesAndCleanupFailsThenYes {
        backend: &b,
        remnant: remnant.clone(),
        lock: RefCell::new(None),
    };
    let mut out = FailsAfterLines(String::new(), 0);

    let result = task_start::run_with(
        &f.env.config_dir(),
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
        &Probe,
        &mut out,
        &mut Vec::new(),
    );

    if confirm.lock.borrow().is_none() {
        return;
    }
    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("screen output failed"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::read(&meta(&f).join("task.json")).is_ok(),
        "the old task is back"
    );
    assert!(meta(&f).join("repo.git").is_dir(), "the old task is back");
    assert!(remnant.is_dir(), "the cleanup failed");
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "an empty remnant is not an old copy"
    );
}

/// Issue #224: an empty remnant of an earlier restore doesn't block the next `--restart`: it is
/// deleted and the restart goes on.
#[test]
fn a_restart_deletes_an_empty_remnant_and_goes_on() {
    let f = fixture();
    started(&f);
    let remnant = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");
    std::fs::create_dir_all(remnant.join(".sbxm").join("tasks")).unwrap();
    std::fs::create_dir_all(remnant.join("tasks")).unwrap();
    let b = playing(&f);

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &FakeConfirm::new(false, false),
            yes: true,
        }),
        &b,
        &github(),
    );

    result.unwrap();
    assert!(!b.creates().is_empty(), "the new task started: {out}");
    assert!(!remnant.exists(), "{out}");
}

/// Issue #224: with a working screen, the command says the old task is back but its set-aside
/// tree could not be deleted, naming the tree.
#[test]
fn a_restore_whose_cleanup_fails_names_the_folder_on_screen() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let remnant = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");
    let confirm = SecretVanishesAndCleanupFailsThenYes {
        backend: &b,
        remnant: remnant.clone(),
        lock: RefCell::new(None),
    };

    let (result, out) = go(
        &f,
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &b,
        &github(),
    );

    if confirm.lock.borrow().is_none() {
        return;
    }
    assert!(result.is_err(), "{out}");
    assert!(out.contains("was put back"), "{out}");
    assert!(out.contains("could not be deleted"), "{out}");
    assert!(out.contains(&remnant.display().to_string()), "{out}");
    assert!(record::read(&meta(&f).join("task.json")).is_ok(), "{out}");
}

/// PR 219 review, M-1 (issue #223): with an issue task set aside, a preparation failure, a blocked
/// restore and an output writer that has already failed all at once, the returned error must still
/// name all four things — decision 185(c) requires the first three of a restore's error, and
/// 185(e) says the screen can't swallow any of it. Before the fix, the combined-error text
/// `restore_after_failed_start` built only ever reached `out` (which this test makes fail from the
/// first write); the function's own return value carried only the generic summary.
#[test]
fn a_combined_failure_names_the_preparation_restore_folder_and_output_failures() {
    let f = fixture();
    started(&f);
    let b = playing(&f);
    let blocked = common::SecretCheckBlocksRestore {
        inner: &b,
        blocker: meta(&f),
        skip: std::sync::atomic::AtomicU32::new(0),
    };
    let confirm = FakeConfirm::new(true, true);
    let mut out = FailsAfterLines(String::new(), 0);
    let remnant = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");

    let result = task_start::run_with(
        &f.env.config_dir(),
        &options(&f),
        Some(&Restart {
            confirm: &confirm,
            yes: false,
        }),
        &blocked,
        &github(),
        &Probe,
        &mut out,
        &mut Vec::new(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("is not stored in sbx"), "{message}");
    assert!(message.contains("putting the old task"), "{message}");
    assert!(message.contains("back failed too"), "{message}");
    assert!(
        message.contains(&remnant.display().to_string()),
        "{message}"
    );
    assert!(message.contains("screen output failed"), "{message}");
    assert!(
        sbxm::task::finish::has_leftover(&f.env.base_dir(), "issue-41"),
        "the old task is still set aside: {message}"
    );
}
