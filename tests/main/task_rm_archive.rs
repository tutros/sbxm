//! Issue 595: every `task rm` archives the task's text records first, as `--merged` does (the
//! same `archive_task`, the same file list), whatever its stage or status, and names the archive
//! folder in its listing. An archive folder that already exists for the id (and isn't the one an
//! earlier, part-way removal made) is never overwritten: the records go under the next free
//! numbered name, `<id>-2`, `<id>-3`, ... Each boundary case of the issue is a test here.

use crate::common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, ctx, fixture, ok, play, ready_spec_task, source,
    spec_file, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::{task_finish, task_rm};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::task::finish::{finish, pr_number};
use sbxm::task::merged::{self, archive_dir, archive_root, archive_task};
use sbxm::task::pipeline;
use sbxm::task::record::{self, Process, Stage, Status};

const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Answers yes on a terminal and records the prompts.
struct Yes(RefCell<Vec<String>>);

impl Yes {
    fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }

    fn prompts(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.0.borrow_mut().push(prompt.to_owned());
        Ok(true)
    }
}

fn meta(f: &Fixture) -> PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn numbered(f: &Fixture, id: &str, n: u32) -> PathBuf {
    archive_root(&f.env.base_dir()).join(format!("{id}-{n}"))
}

fn worked_backend(f: &Fixture) -> FakeBackend {
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
}

/// Issue 41 worked, its gates and review done by hand, then finished (pushed, PR opened).
fn finished_task(f: &Fixture) -> u32 {
    let b = worked_backend(f);
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    let url = finish(
        &f.env.base_dir(),
        "issue-41",
        &FakeGitHub::default(),
        &Probe,
    )
    .unwrap()
    .url;
    pr_number(&url).unwrap()
}

fn rm_target(
    f: &Fixture,
    target: task_rm::Target,
    yes: bool,
    force: bool,
    repo_root: &Path,
    confirm: &dyn Confirm,
) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = task_rm::run(
        &f.env.config_dir(),
        &task_rm::Options {
            target,
            yes,
            force,
            repo_root: repo_root.to_path_buf(),
            dry_run: false,
            forget_ambiguous: false,
        },
        &backend(),
        &FakeGitHub::default(),
        &Probe,
        confirm,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

fn rm_issue(f: &Fixture, confirm: &dyn Confirm) -> (anyhow::Result<()>, String) {
    let repo_root = f.env.tmp.path().join("target-repo");
    rm_target(
        f,
        task_rm::Target::Issue(41),
        false,
        false,
        &repo_root,
        confirm,
    )
}

// ---- `--issue N` ----

#[test]
fn rm_issue_of_a_finished_task_archives_its_records_then_removes_it() {
    let f = fixture();
    finished_task(&f);
    let task_json = fs::read(meta(&f).join("task.json")).unwrap();
    let confirm = Yes::new();

    let (result, out) = rm_issue(&f, &confirm);

    result.unwrap();
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert_eq!(fs::read(archive.join("task.json")).unwrap(), task_json);
    assert_eq!(
        fs::read_to_string(archive.join("review.md")).unwrap(),
        REVIEW
    );
    assert!(archive.join("result.md").is_file());
    assert!(!archive.join("repo.git").exists());
    assert!(!meta(&f).exists());
    assert!(
        out.contains(&format!("archived task issue-41 to {}", archive.display())),
        "{out}"
    );
    let prompts = confirm.prompts();
    let [prompt] = prompts.as_slice() else {
        panic!("{prompts:?}");
    };
    assert!(
        prompt.contains(&format!("archive to {}", archive.display())),
        "{prompt}"
    );
}

#[test]
fn rm_issue_of_an_interrupted_task_archives_the_records_it_has_and_skips_the_rest() {
    let f = fixture();
    let mut prepared = worked_task(&f, &worked_backend(&f));
    prepared.record.status = Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    for name in ["review.md", "result.md"] {
        let _ = fs::remove_file(meta(&f).join(name));
    }
    fs::write(meta(&f).join("run.log"), "# header\n").unwrap();
    drop(prepared);

    let (result, _) = rm_issue(&f, &Yes::new());

    result.unwrap();
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert!(archive.join("task.json").is_file());
    assert_eq!(
        fs::read_to_string(archive.join("run.log")).unwrap(),
        "# header\n"
    );
    assert!(!archive.join("review.md").exists());
    assert!(!archive.join("result.md").exists());
    assert!(!meta(&f).exists());
}

/// The listing, without a terminal and without `--yes`, names the archive folder too.
#[test]
fn the_refusal_without_a_terminal_names_the_archive_folder_and_writes_nothing() {
    struct NoTerminal;
    impl Confirm for NoTerminal {
        fn is_interactive(&self) -> bool {
            false
        }
        fn confirm(&self, _: &str) -> anyhow::Result<bool> {
            unreachable!()
        }
    }
    let f = fixture();
    finished_task(&f);

    let (result, _) = rm_issue(&f, &NoTerminal);

    let message = format!("{:#}", result.unwrap_err());
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert!(
        message.contains(&format!("archive to {}", archive.display())),
        "{message}"
    );
    assert!(!archive.exists(), "nothing is archived before the answer");
    assert!(meta(&f).join("task.json").is_file());
}

// ---- Numbering: an archive folder for the id already exists ----

#[test]
fn an_existing_archive_of_the_id_gives_the_next_numbered_name_and_stays_untouched() {
    let f = fixture();
    finished_task(&f);
    let old = archive_dir(&f.env.base_dir(), "issue-41");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("task.json"), "an older task 41\n").unwrap();
    let confirm = Yes::new();

    let (result, out) = rm_issue(&f, &confirm);

    result.unwrap();
    let second = numbered(&f, "issue-41", 2);
    assert!(second.join("task.json").is_file());
    assert_eq!(
        fs::read_to_string(old.join("task.json")).unwrap(),
        "an older task 41\n"
    );
    assert!(!meta(&f).exists());
    assert!(confirm.prompts()[0].contains(&format!("archive to {}", second.display())));
    assert!(
        out.contains(&format!("archived task issue-41 to {}", second.display())),
        "{out}"
    );
}

#[test]
fn existing_id_and_id_2_archives_give_id_3() {
    let f = fixture();
    finished_task(&f);
    let base = f.env.base_dir();
    for dir in [archive_dir(&base, "issue-41"), numbered(&f, "issue-41", 2)] {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("task.json"), "older\n").unwrap();
    }

    let archive = archive_task(&base, "issue-41").unwrap();

    assert_eq!(archive, numbered(&f, "issue-41", 3));
    assert!(archive.join("task.json").is_file());
    for dir in [archive_dir(&base, "issue-41"), numbered(&f, "issue-41", 2)] {
        assert_eq!(
            fs::read_to_string(dir.join("task.json")).unwrap(),
            "older\n"
        );
    }
}

#[test]
fn a_plain_file_in_place_of_the_archive_folder_is_skipped_too() {
    let f = fixture();
    finished_task(&f);
    let base = f.env.base_dir();
    let old = archive_dir(&base, "issue-41");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::write(&old, "not a folder\n").unwrap();

    let archive = archive_task(&base, "issue-41").unwrap();

    assert_eq!(archive, numbered(&f, "issue-41", 2));
    assert_eq!(fs::read_to_string(&old).unwrap(), "not a folder\n");
}

/// Issue 503's reuse still comes first: a byte-identical archive is the one an earlier,
/// part-way removal made, reused as it is rather than numbered.
#[test]
fn a_byte_identical_earlier_archive_is_reused_not_numbered() {
    let f = fixture();
    finished_task(&f);
    let first = archive_task(&f.env.base_dir(), "issue-41").unwrap();
    let confirm = Yes::new();

    let (result, out) = rm_issue(&f, &confirm);

    result.unwrap();
    assert!(!numbered(&f, "issue-41", 2).exists());
    assert!(confirm.prompts()[0].contains(&format!("archive to {}", first.display())));
    assert!(
        out.contains(&format!("archived task issue-41 to {}", first.display())),
        "{out}"
    );
}

/// A numbered archive an earlier part-way removal made is reused the same way.
#[test]
fn a_byte_identical_numbered_archive_is_reused() {
    let f = fixture();
    finished_task(&f);
    let base = f.env.base_dir();
    let old = archive_dir(&base, "issue-41");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("task.json"), "older\n").unwrap();
    let second = archive_task(&base, "issue-41").unwrap();
    assert_eq!(second, numbered(&f, "issue-41", 2));

    assert_eq!(archive_task(&base, "issue-41").unwrap(), second);
    assert!(!numbered(&f, "issue-41", 3).exists());
}

#[test]
fn rm_merged_numbers_its_archive_the_same_way_and_its_dry_run_names_it() {
    let f = fixture();
    let number = finished_task(&f);
    let old = archive_dir(&f.env.base_dir(), "issue-41");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("task.json"), "older\n").unwrap();
    let github = FakeGitHub::default().with_pr(PrInfo {
        number,
        head_ref: "issue-41".into(),
        head_oid: "0".repeat(40),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Merged,
        title: "Fix 41".into(),
        body: String::new(),
        closing_issues: vec![41],
        base_moved: false,
    });
    let second = numbered(&f, "issue-41", 2);
    let merged_rm = |dry_run: bool, confirm: &Yes| {
        let mut out = Vec::new();
        let result = task_rm::run(
            &f.env.config_dir(),
            &task_rm::Options {
                target: task_rm::Target::Merged,
                yes: false,
                force: false,
                repo_root: f.env.tmp.path().join("target-repo"),
                dry_run,
                forget_ambiguous: false,
            },
            &backend(),
            &github,
            &Probe,
            confirm,
            &mut out,
        );
        (result, String::from_utf8(out).unwrap())
    };

    let (result, out) = merged_rm(true, &Yes::new());
    result.unwrap();
    assert!(
        out.contains(&format!("archive to {}", second.display())),
        "{out}"
    );
    assert!(!second.exists(), "a dry run writes nothing");

    let confirm = Yes::new();
    let (result, out) = merged_rm(false, &confirm);
    result.unwrap();
    assert!(confirm.prompts()[0].contains(&format!("archive to {}", second.display())));
    assert!(second.join("task.json").is_file());
    assert_eq!(
        fs::read_to_string(old.join("task.json")).unwrap(),
        "older\n"
    );
    assert!(
        out.contains(&format!("archived task issue-41 to {}", second.display())),
        "{out}"
    );
    assert!(!meta(&f).exists());
}

// ---- Archiving fails ----

/// A plain file where `.sbxm/task-archive` belongs: refused before the question, naming the
/// archive path as not a folder, with nothing deleted. The refusal must not hang on the OS's
/// error for a path under a plain file: Linux says "not a directory", Windows "not found".
#[test]
fn an_archive_that_cannot_be_made_deletes_nothing_and_names_the_archive_path() {
    let f = fixture();
    finished_task(&f);
    let root = archive_root(&f.env.base_dir());
    fs::write(&root, "not a folder\n").unwrap();
    let confirm = Yes::new();

    let (result, _) = rm_issue(&f, &confirm);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains(&root.display().to_string()), "{message}");
    assert!(message.contains("is not a folder"), "{message}");
    assert!(meta(&f).join("task.json").is_file());
    assert!(f.env.base_dir().join("tasks").join("issue-41").is_dir());
    assert!(confirm.prompts().is_empty(), "refused before the question");
}

/// Keeps `path` unreadable on Unix until dropped; `None` when that can't stop a read here.
#[cfg(unix)]
struct Unreadable(PathBuf);

#[cfg(unix)]
impl Unreadable {
    fn new(path: &Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        let guard = Self(path.to_owned());
        fs::read(path).is_err().then_some(guard)
    }
}

#[cfg(unix)]
impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o644));
    }
}

/// A record that can't be copied once the removal is confirmed: the task stays whole, and the
/// error names the archive path.
#[cfg(unix)]
#[test]
fn a_copy_that_fails_after_the_answer_deletes_nothing_and_names_the_archive_path() {
    let f = fixture();
    finished_task(&f);
    let Some(_lock) = Unreadable::new(&meta(&f).join("result.md")) else {
        return;
    };

    let (result, out) = rm_issue(&f, &Yes::new());

    let message = format!("{:#}", result.unwrap_err());
    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert!(
        message.contains(&archive.display().to_string()),
        "{message}"
    );
    assert!(out.contains("could not archive task issue-41"), "{out}");
    assert!(meta(&f).join("task.json").is_file());
    assert!(f.env.base_dir().join("tasks").join("issue-41").is_dir());
}

// ---- `--pr N` ----

#[test]
fn rm_pr_archives_each_fix_task_under_its_own_id() {
    use common::fix_task;
    let f = fix_task::config();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let old = fix_task::interrupted_fix(&f, &fix_task::new_head(&f), &backend);
    let current = fix_task::interrupted_fix(&f, &fix_task::new_head(&f), &backend);
    // The review task has only its clone: no records, so nothing of it is archived.
    fix_task::review_task(&f);
    let confirm = fix_task::yes_on_a_terminal();

    let out = fix_task::rm(&f, false, &backend, &confirm);

    out.result.unwrap();
    let prompt = confirm.prompts.borrow()[0].clone();
    for id in [&old, &current] {
        let archive = archive_dir(&f.env.base_dir(), id);
        let saved = record::read(&archive.join("task.json")).unwrap();
        assert_eq!(&saved.id, id);
        assert!(!fix_task::meta(&f, id).exists(), "{id}");
        assert!(
            prompt.contains(&format!("archive to {}", archive.display())),
            "{prompt}"
        );
    }
    assert!(!archive_dir(&f.env.base_dir(), "pr-7").exists());
}

// ---- `--spec FILE` ----

fn kept_spec_task(f: &Fixture) -> (PathBuf, String) {
    let (spec, id) = ready_spec_task(f);
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(spec.clone()),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeBackend::default(),
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    (spec, id)
}

#[test]
fn rm_spec_with_force_archives_the_task() {
    let f = fixture();
    let (spec, id) = kept_spec_task(&f);
    let task_json = fs::read(record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    let repo_root = f.env.tmp.path().join("target-repo");

    let (result, _) = rm_target(
        &f,
        task_rm::Target::Spec(spec),
        true,
        true,
        &repo_root,
        &Yes::new(),
    );

    result.unwrap();
    let archive = archive_dir(&f.env.base_dir(), &id);
    assert_eq!(fs::read(archive.join("task.json")).unwrap(), task_json);
    assert!(!archive.join("repo.git").exists());
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
}

#[test]
fn rm_spec_without_force_archives_the_task() {
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let b = backend();
    let github = FakeGitHub::default();
    let source = source(&f);
    let mut prepared = pipeline::prepare_spec(&ctx(&f, &source, &b, &github), &spec).unwrap();
    prepared.record.finish(Status::Failed).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    prepared.lock = None;
    let id = prepared.record.id.clone();
    let repo_root = f.env.tmp.path().join("target-repo");

    let (result, _) = rm_target(
        &f,
        task_rm::Target::Spec(spec),
        true,
        false,
        &repo_root,
        &Yes::new(),
    );

    result.unwrap();
    let saved = record::read(&archive_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!(saved.id, id);
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
}

/// `merged::archive_target` names the folder `archive_task` would use, writing nothing.
#[test]
fn the_planned_archive_folder_is_the_one_archiving_uses_and_planning_writes_nothing() {
    let f = fixture();
    finished_task(&f);
    let base = f.env.base_dir();

    let planned = merged::archive_target(&base, "issue-41").unwrap();

    assert_eq!(planned, archive_dir(&base, "issue-41"));
    assert!(!archive_root(&base).exists(), "planning wrote something");
    assert_eq!(archive_task(&base, "issue-41").unwrap(), planned);
}
