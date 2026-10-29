//! Decision 185 (issue #207): `--restart`'s discard is undoable. These tests exercise the
//! `finish::SetAside` mechanics directly (set aside, restore, settle) rather than through a whole
//! restart command, since the command-level behavior is already covered by
//! `tests/task_start_restart.rs`, `tests/task_run.rs` and `tests/task_run_spec.rs`.

mod common;

use std::fs;

use common::task_fixture::{Fixture, Probe, ctx, fixture, issue_text, source};
use sbxm::backend::FakeBackend;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, Prepared, TaskEnv};
use sbxm::task::record::{self, ProcessProbe, Stage, Status};
use sbxm::task::{finish, resume};

/// The process a record names is gone (the `sbxm` that wrote it was killed): needed to resume a
/// task left `Working`/`Failed`, since [`Probe`] (used for the set-aside mechanics themselves)
/// always reports the recorded process as still alive.
struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// A `Confirm` that must never be asked: `set_aside`/`set_aside_many` with `yes: true` never
/// prompts.
struct Never;

impl Confirm for Never {
    fn is_interactive(&self) -> bool {
        panic!("not asked")
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        panic!("not asked")
    }
}

fn meta(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn saved(f: &Fixture) -> record::Record {
    record::read(&meta(f).join("task.json")).unwrap()
}

/// `number`, prepared and worked once, left `Working`/`Failed` as a crash would leave it: a
/// resumable state ([`resume::resume`] reruns the worker for it).
fn resumable_task_for(f: &Fixture, backend: &FakeBackend, number: u32) -> Prepared {
    let github = FakeGitHub::default();
    let source = source(f);
    let ctx = ctx(f, &source, backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &issue_text(number)).unwrap();
    pipeline::run_worker(&ctx, &mut prepared).unwrap();
    prepared.record.stage = Stage::Working;
    prepared.record.status = Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared
}

/// Issue 41, prepared and worked once, left `Working`/`Failed` as a crash would leave it.
fn resumable_task(f: &Fixture, backend: &FakeBackend) -> Prepared {
    resumable_task_for(f, backend, 41)
}

fn host() -> &'static sbxm::task::gates::FakeHostRunner {
    Box::leak(Box::default())
}

fn env<'a>(f: &'a Fixture, backend: &'a FakeBackend) -> TaskEnv<'a> {
    TaskEnv {
        config_dir: Box::leak(Box::new(f.env.config_dir())),
        config: &f.config,
        backend,
        probe: &Gone,
        host: host(),
        sleep: &std::thread::sleep,
        scoped_secrets: &[],
        identity: None,
    }
}

/// Decision 185 point 2: the old task's sandbox is removed at the discard (not undone by
/// `restore`), so a restored task has none; its `task.json` still names it, and `task resume`
/// accepts the task and makes the sandbox again rather than refusing it.
#[test]
fn a_restored_task_has_no_sandbox_but_resume_makes_it_again() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let prepared = resumable_task(&f, &backend);
    let sandbox = prepared.record.worker.clone().unwrap().sandbox;
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(backend.removes(), vec![sandbox.clone()]);
    aside.restore().unwrap();

    // The restored record still names the (now gone) sandbox: `restore` only moves folders.
    let restored = saved(&f);
    assert_eq!(restored.worker.unwrap().sandbox, sandbox);
    assert!(meta(&f).join("repo.git").is_dir());

    let mut resuming = Prepared::open(&base_dir, "issue-41").unwrap();
    let fresh = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let resumed = resume::resume(&env(&f, &fresh), &mut resuming, None).unwrap();

    assert!(resumed.sandbox_remade, "resume must make the sandbox again");
    assert!(
        fresh.creates().iter().any(|c| c.name == sandbox),
        "{:?}",
        fresh.creates()
    );
}

/// Decision 185 point 3: once the new task's preparation is complete, the set-aside copy is gone
/// for good — here simulated directly by calling `settle`, since the command-level tests already
/// cover the full restart flow.
#[test]
fn settling_deletes_the_set_aside_copy_for_good() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(finish::has_leftover(&base_dir, "issue-41"));

    aside.settle().unwrap();

    assert!(!finish::has_leftover(&base_dir, "issue-41"));
    assert!(
        !base_dir.join(".sbxm").join("tasks-replaced").exists()
            || fs::read_dir(base_dir.join(".sbxm").join("tasks-replaced"))
                .unwrap()
                .next()
                .is_none(),
        "nothing left under tasks-replaced/"
    );
}

/// Decision 185 point 4: when putting the old task back fails, the error names both failures and
/// the set-aside folder, and the folder still holds the old task (nothing more is deleted).
#[test]
fn a_failed_restore_names_both_failures_and_keeps_the_old_task_set_aside() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();

    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(finish::has_leftover(&base_dir, "issue-41"));

    // Block the restore: put a plain file where the meta folder needs to go back.
    let meta_dir = meta(&f);
    fs::create_dir_all(meta_dir.parent().unwrap()).unwrap();
    fs::write(&meta_dir, b"in the way").unwrap();

    let start_failed = anyhow::anyhow!("the worker's secret is missing");
    let combined = finish::restore_after_failed_start(&aside, start_failed);
    let message = format!("{combined:#}");

    assert!(
        message.contains("the worker's secret is missing"),
        "{message}"
    );
    assert!(message.contains("putting the old task"), "{message}");
    assert!(
        message.contains(&aside.folder().display().to_string()),
        "{message}"
    );
    assert!(
        finish::has_leftover(&base_dir, "issue-41"),
        "the old task is still set aside"
    );
    // Nothing more was deleted: the blocking file is still exactly what it was.
    assert_eq!(fs::read(&meta_dir).unwrap(), b"in the way");
}

/// Decision 185 point 4: a `--restart` of a task with a leftover copy is refused before anything
/// is checked, naming the folder and the two ways out.
#[test]
fn check_no_leftover_refuses_and_names_the_folder_and_the_two_ways_out() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();
    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();

    let message = format!(
        "{:#}",
        finish::check_no_leftover(&base_dir, "issue-41").unwrap_err()
    );

    assert!(
        message.contains(&aside.folder().display().to_string()),
        "{message}"
    );
    assert!(message.contains("move it back"), "{message}");
    assert!(message.contains("delete it"), "{message}");
}

/// A `Write` that accepts everything up to (and including) the first `\n` it sees, then fails:
/// models a sandbox-status writer (stdout, a pipe) that works for one line and then can't be
/// written to anymore (a closed pipe, a full disk) — exactly the review's "accepts the first
/// sandbox-status line and fails on the second" reproduction.
#[derive(Default)]
struct FailsAfterFirstLine(String);

impl std::io::Write for FailsAfterFirstLine {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.contains('\n') {
            return Err(std::io::Error::other("sandbox status output failed"));
        }
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Issue #207 review round 4, M-1: `set_aside_many`'s sandbox-status `writeln!` used `?` instead
/// of going through the same rollback `set_aside_one`'s failure does, so a write failure between
/// the first and second task left the first task's `SetAside` dropped without ever being restored
/// — stranded under `tasks-replaced/` with no way for the caller to know or recover it. Two tasks,
/// a writer that accepts the first task's sandbox-status line and fails on the second: the first
/// task must come back, and the error must say the write is why.
#[test]
fn multi_set_aside_restores_the_first_task_if_output_fails_on_the_second() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task_for(&f, &backend, 41);
    resumable_task_for(&f, &backend, 42);
    let base_dir = f.env.base_dir();
    let ids = vec!["issue-41".to_owned(), "issue-42".to_owned()];
    let mut out = FailsAfterFirstLine::default();

    let err = finish::set_aside_many(&base_dir, &ids, true, &backend, &Probe, &Never, &mut out)
        .unwrap_err();

    assert!(
        format!("{err:#}").contains("sandbox status output failed"),
        "{err:#}"
    );
    assert!(
        !finish::has_leftover(&base_dir, "issue-41"),
        "the first old task must be put back"
    );
    assert!(
        record::task_dir(&base_dir, "issue-41")
            .join("task.json")
            .exists()
    );
    assert!(
        record::task_dir(&base_dir, "issue-41")
            .join("repo.git")
            .is_dir()
    );
    assert!(
        !finish::has_leftover(&base_dir, "issue-42"),
        "the second task's folders were never moved, so nothing is set aside for it either"
    );
    assert!(
        record::task_dir(&base_dir, "issue-42")
            .join("task.json")
            .exists()
    );
}

/// A `Write` for a two-task `set_aside_many`: when the second line starts (the first task is set
/// aside by then), it puts a plain file where the first task's folder has to go back, so that
/// task's restore fails. The second line itself fails too when `fail` is set.
struct BlocksTheFirstRestore {
    seen: String,
    in_the_way: std::path::PathBuf,
    fail: bool,
}

impl std::io::Write for BlocksTheFirstRestore {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.seen.contains('\n') {
            fs::write(&self.in_the_way, b"in the way")?;
            if self.fail {
                return Err(std::io::Error::other("sandbox status output failed"));
            }
        }
        self.seen.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn assert_names_the_stranded_first_task(message: &str, base_dir: &std::path::Path) {
    assert!(
        message.contains("putting the old task issue-41 back failed too"),
        "{message}"
    );
    let folder = base_dir
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");
    assert!(message.contains(&folder.display().to_string()), "{message}");
    assert!(
        finish::has_leftover(base_dir, "issue-41"),
        "the old task is still set aside"
    );
    assert!(
        folder
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .join("task.json")
            .exists()
    );
}

/// Issue #207 review round 7, M-1: the batch rollback dropped a failed restore, so the caller saw
/// only the output failure and was never told the first task was still set aside, or where
/// (decision 185 point 4 wants both failures and the folder named).
#[test]
fn output_failure_reports_when_rolling_back_the_first_task_also_fails() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task_for(&f, &backend, 41);
    resumable_task_for(&f, &backend, 42);
    let base_dir = f.env.base_dir();
    let ids = vec!["issue-41".to_owned(), "issue-42".to_owned()];
    let mut out = BlocksTheFirstRestore {
        seen: String::new(),
        in_the_way: record::task_dir(&base_dir, "issue-41"),
        fail: true,
    };

    let err = finish::set_aside_many(&base_dir, &ids, true, &backend, &Probe, &Never, &mut out)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("sandbox status output failed"),
        "{message}"
    );
    assert_names_the_stranded_first_task(&message, &base_dir);
}

/// The same when what fails is the second task's own set-aside (here its `tasks-replaced/` folder
/// can't be made: a plain file sits there).
#[test]
fn a_failed_set_aside_reports_when_rolling_back_the_first_task_also_fails() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task_for(&f, &backend, 41);
    resumable_task_for(&f, &backend, 42);
    let base_dir = f.env.base_dir();
    let replaced = base_dir.join(".sbxm").join("tasks-replaced");
    fs::create_dir_all(&replaced).unwrap();
    fs::write(replaced.join("issue-42"), b"not a folder").unwrap();
    let ids = vec!["issue-41".to_owned(), "issue-42".to_owned()];
    let mut out = BlocksTheFirstRestore {
        seen: String::new(),
        in_the_way: record::task_dir(&base_dir, "issue-41"),
        fail: false,
    };

    let err = finish::set_aside_many(&base_dir, &ids, true, &backend, &Probe, &Never, &mut out)
        .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("issue-42"), "{message}");
    assert_names_the_stranded_first_task(&message, &base_dir);
    assert!(
        record::task_dir(&base_dir, "issue-42")
            .join("task.json")
            .exists()
    );
}

/// PR 219 review, M-1: a later folder that fails the safety check (a link where a task folder
/// should be) used to stop the set-aside after an earlier folder had already moved. Every folder
/// is checked before any is moved, so the old task stays whole where it was.
#[test]
fn a_later_linked_folder_fails_the_set_aside_with_every_folder_still_in_place() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();
    let elsewhere = f.env.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    common::dir_link(&base_dir.join("tasks").join("issue-41-review"), &elsewhere);

    let result = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    );

    assert!(result.is_err(), "a linked task folder must be refused");
    assert!(
        base_dir.join("tasks").join("issue-41").is_dir(),
        "the workspace must not have moved"
    );
    assert!(meta(&f).join("task.json").exists());
    assert!(!finish::has_leftover(&base_dir, "issue-41"));
}

/// Issue #224: every task folder moves back, but the emptied set-aside tree can't be deleted (a
/// folder in it is held open). The restore still succeeds, and what the command reports names the
/// cleanup failure and its exact folder instead of an unqualified "put back".
#[test]
fn a_restore_whose_cleanup_fails_puts_the_task_back_and_names_the_folder() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();
    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    let Some(_lock) = common::Undeletable::new(&aside.folder().join(".sbxm")) else {
        return;
    };

    let combined = finish::restore_after_failed_start(
        &aside,
        anyhow::anyhow!("the worker's secret is missing"),
    );

    let message = format!("{combined:#}");
    assert!(
        message.contains("the worker's secret is missing"),
        "{message}"
    );
    assert!(message.contains("was put back"), "{message}");
    assert!(message.contains("could not be deleted"), "{message}");
    assert!(
        message.contains(&aside.folder().display().to_string()),
        "{message}"
    );
    assert!(meta(&f).join("task.json").exists(), "the old task is back");
    assert!(meta(&f).join("repo.git").is_dir(), "the old task is back");
    assert!(aside.folder().is_dir(), "the remnant is still there");
    assert!(
        !finish::has_leftover(&base_dir, "issue-41"),
        "an empty remnant is not an old copy"
    );
}

/// Issue #224: the remnant of the test above is cleaned up by the next leftover check: refused
/// with an accurate "empty folder, safe to delete" message while it still can't be deleted, which
/// may have removed part of it (a rerun sees what is left, still only empty folders, the same
/// way), and deleted once it can be.
#[test]
fn the_next_check_deletes_a_cleanup_remnant_or_says_it_is_safe_to_delete() {
    let f = fixture();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    resumable_task(&f, &backend);
    let base_dir = f.env.base_dir();
    let aside = finish::set_aside(
        &base_dir,
        "issue-41",
        true,
        &backend,
        &Probe,
        &Never,
        &mut Vec::new(),
    )
    .unwrap();
    let folder = aside.folder();
    let lock = common::Undeletable::new(&folder.join(".sbxm"));
    if lock.is_none() {
        return;
    }
    let _ = finish::restore_after_failed_start(&aside, anyhow::anyhow!("start failed"));

    for attempt in 1..=2 {
        let message = format!(
            "{:#}",
            finish::check_no_leftover(&base_dir, "issue-41").unwrap_err()
        );
        assert!(
            message.contains("empty folder") && message.contains("safe to delete"),
            "attempt {attempt}: {message}"
        );
        assert!(
            !message.contains("old copy"),
            "attempt {attempt}: {message}"
        );
        assert!(
            message.contains(&folder.display().to_string()),
            "attempt {attempt}: {message}"
        );
        assert!(folder.is_dir(), "attempt {attempt}");
        assert!(!finish::has_leftover(&base_dir, "issue-41"));
    }

    drop(lock);
    finish::check_no_leftover(&base_dir, "issue-41").unwrap();
    assert!(!folder.exists(), "the remnant is gone");
    assert!(meta(&f).join("task.json").exists(), "the task is untouched");
}

/// `tasks-replaced/<id>` under a fresh base dir.
fn bare_remnant() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let base_dir = tmp.path().join("base");
    let root = base_dir
        .join(".sbxm")
        .join("tasks-replaced")
        .join("issue-41");
    (tmp, base_dir, root)
}

/// Issue #224: a set-aside tree holding only empty folders, at any depth, is no leftover; the
/// next check deletes it and goes on.
#[test]
fn an_empty_remnant_is_deleted_by_the_next_check() {
    let (_tmp, base_dir, root) = bare_remnant();
    fs::create_dir_all(root.join(".sbxm").join("tasks")).unwrap();
    fs::create_dir_all(root.join("tasks")).unwrap();

    assert!(!finish::has_leftover(&base_dir, "issue-41"));
    finish::check_no_leftover(&base_dir, "issue-41").unwrap();

    assert!(!root.exists());
    assert!(base_dir.join(".sbxm").join("tasks-replaced").is_dir());
}

fn assert_old_copy_refused(base_dir: &std::path::Path) {
    assert!(finish::has_leftover(base_dir, "issue-41"));
    let message = format!(
        "{:#}",
        finish::check_no_leftover(base_dir, "issue-41").unwrap_err()
    );
    assert!(message.contains("left its old copy"), "{message}");
}

/// Issue #224: a file anywhere in the tree is still an old copy, refused as before; nothing is
/// deleted.
#[test]
fn a_remnant_with_a_file_deep_inside_is_still_an_old_copy() {
    let (_tmp, base_dir, root) = bare_remnant();
    let deep = root.join("a").join("b").join("c");
    fs::create_dir_all(&deep).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(deep.join("task.json"), b"{}").unwrap();

    assert_old_copy_refused(&base_dir);

    assert_eq!(fs::read(deep.join("task.json")).unwrap(), b"{}");
    assert!(root.join("empty").is_dir(), "nothing was deleted");
}

/// Issue #224: so is a link (a junction on Windows) anywhere in the tree, even to an empty folder;
/// neither the link nor what it points at is touched.
#[test]
fn a_remnant_with_a_link_inside_is_still_an_old_copy() {
    let (tmp, base_dir, root) = bare_remnant();
    let outside = tmp.path().join("outside");
    fs::create_dir_all(outside.join("empty")).unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    common::dir_link(&root.join("a").join("link"), &outside);

    assert_old_copy_refused(&base_dir);

    assert!(root.join("a").join("link").symlink_metadata().is_ok());
    assert!(outside.join("empty").is_dir());
}

/// Issue #224: when the set-aside root itself, `.sbxm/tasks-replaced` or `.sbxm` is a link, the
/// check refuses and deletes nothing through it, even if all it reaches is empty folders.
#[test]
fn a_linked_remnant_or_parent_is_refused_and_nothing_is_deleted_through_it() {
    for linked in ["issue-41", "tasks-replaced", ".sbxm"] {
        let (tmp, base_dir, root) = bare_remnant();
        let outside = tmp.path().join("outside");
        let link = match linked {
            "issue-41" => root.clone(),
            "tasks-replaced" => root.parent().unwrap().to_path_buf(),
            _ => base_dir.join(".sbxm"),
        };
        // What the link reaches: the same empty tree it stands for.
        let reached = outside.join(root.strip_prefix(&link).unwrap());
        fs::create_dir_all(reached.join("tasks")).unwrap();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        common::dir_link(&link, &outside);

        let err = finish::check_no_leftover(&base_dir, "issue-41").unwrap_err();

        let message = format!("{err:#}");
        assert!(message.contains("issue-41"), "{linked}: {message}");
        assert!(reached.join("tasks").is_dir(), "{linked}: {message}");
        assert!(link.symlink_metadata().is_ok(), "{linked}: {message}");
    }
}
