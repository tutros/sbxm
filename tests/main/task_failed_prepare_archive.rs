//! Issue 682: a new task whose preparation fails keeps its text records. Before the rollback
//! deletes the task folder, it archives them (as `task rm` does) to
//! `<base>/.sbxm/task-archive/<id>-failed-<UTC yyyyMMdd-HHmmss>`, and the error names the archive.
//! A failed `--restart` restores the old task instead and archives nothing as failed.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, ctx, fixture, issue_text, ok, open_issue,
    play_tasks, source,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_start::{self, Options, Restart};
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, FailedAfterCreate};
use sbxm::task::record;
use sbxm::task::repo::Identity;

const PULL: [&str; 2] = ["{name}: PREPARE IMAGE", "{name}: CONFIGURE AGENT"];

fn failing() -> FakeBackend {
    FakeBackend::failing_create()
        .and_secrets(&["anthropic"])
        .with_create_output(&PULL)
        .with_create_failure_text("sbx create failed: no space left")
}

/// The `-failed-` archives of `id`, sorted by name.
fn failed_archives(base: &Path, id: &str) -> Vec<PathBuf> {
    let root = base.join(".sbxm").join("task-archive");
    let prefix = format!("{id}-failed-");
    let mut found: Vec<PathBuf> = fs::read_dir(&root)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Whether `name` is `<id>-failed-yyyyMMdd-HHmmss`, perhaps with a `-<n>` after it.
fn is_failed_name(name: &str, id: &str) -> bool {
    let Some(rest) = name.strip_prefix(&format!("{id}-failed-")) else {
        return false;
    };
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = rest.split('-');
    let (date, time) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let suffix_ok = parts.next().is_none_or(|n| n.parse::<u32>().is_ok()) && parts.next().is_none();
    digits(date, 8) && digits(time, 6) && suffix_ok
}

fn prepare_failing(f: &Fixture, backend: &FakeBackend) -> anyhow::Error {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::prepare(&ctx(f, &source, backend, &github), &issue_text(41)).unwrap_err()
}

/// Boundary case 1: the folder is gone, the archive holds a `run.log` with the lines `create`
/// printed, and the error names the archive.
#[test]
fn task_failed_prepare_archive_keeps_the_run_log_and_names_it() {
    let f = fixture();
    let base = f.env.base_dir();

    let err = prepare_failing(&f, &failing());

    let message = format!("{err:#}");
    assert!(message.contains("no space left"), "{message}");
    assert!(
        !record::task_dir(&base, "issue-41").exists(),
        "the folder goes"
    );
    let archives = failed_archives(&base, "issue-41");
    assert_eq!(archives.len(), 1, "{archives:?}");
    let archive = &archives[0];
    let name = archive.file_name().unwrap().to_str().unwrap();
    assert!(is_failed_name(name, "issue-41"), "{name}");
    let log = fs::read_to_string(archive.join("run.log")).unwrap();
    assert!(
        log.contains("sbxm-task-issue-41-claude: PREPARE IMAGE")
            && log.contains("sbxm-task-issue-41-claude: CONFIGURE AGENT"),
        "{log}"
    );
    assert!(
        message.contains(&archive.display().to_string()),
        "the error names {}: {message}",
        archive.display()
    );
    let after = err
        .downcast_ref::<FailedAfterCreate>()
        .expect("the failure carries the archive");
    assert!(after.warnings.is_empty(), "{:?}", after.warnings);
}

/// Boundary case 2: two failures in a row make two archives, neither overwritten.
#[test]
fn task_failed_prepare_archive_twice_keeps_two_archives() {
    let f = fixture();
    let base = f.env.base_dir();

    prepare_failing(&f, &failing());
    let first = failed_archives(&base, "issue-41");
    assert_eq!(first.len(), 1, "{first:?}");
    let marker = first[0].join("run.log");
    fs::write(&marker, "the first failure\n").unwrap();
    prepare_failing(&f, &failing());

    let both = failed_archives(&base, "issue-41");
    assert_eq!(both.len(), 2, "{both:?}");
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "the first failure\n",
        "the first archive is never written to again"
    );
    for archive in &both {
        let name = archive.file_name().unwrap().to_str().unwrap();
        assert!(is_failed_name(name, "issue-41"), "{name}");
    }
}

/// Boundary case 4: an archive that can't be written is one warning, and the original error is
/// still returned; the folder goes as before.
#[test]
fn task_failed_prepare_archive_that_cant_be_written_is_one_warning() {
    let f = fixture();
    let base = f.env.base_dir();
    fs::create_dir_all(base.join(".sbxm")).unwrap();
    fs::write(base.join(".sbxm").join("task-archive"), "not a folder").unwrap();

    let err = prepare_failing(&f, &failing());

    let message = format!("{err:#}");
    assert!(message.contains("no space left"), "{message}");
    assert!(
        !record::task_dir(&base, "issue-41").exists(),
        "the folder goes"
    );
    let after = err
        .downcast_ref::<FailedAfterCreate>()
        .expect("the failure carries the warning");
    assert_eq!(after.warnings.len(), 1, "{:?}", after.warnings);
    assert!(
        after.warnings[0].contains("task-archive"),
        "{:?}",
        after.warnings
    );
}

/// Boundary case 5: a preparation that succeeds makes no `-failed-` archive.
#[test]
fn task_failed_prepare_archive_none_when_the_preparation_succeeds() {
    let f = fixture();
    let (backend, github) = (backend(), FakeGitHub::default());
    let source = source(&f);

    pipeline::prepare(&ctx(&f, &source, &backend, &github), &issue_text(41)).unwrap();

    assert!(failed_archives(&f.env.base_dir(), "issue-41").is_empty());
}

// ---- Boundary case 3: a failed `--restart` restores the old task, archives nothing ----

struct Yes;

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
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

fn start(f: &Fixture, restart: Option<&Restart>, backend: &FakeBackend) -> anyhow::Result<()> {
    task_start::run_with(
        &f.env.config_dir(),
        &options(f),
        restart,
        backend,
        &github(),
        &Probe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
}

#[test]
fn task_failed_prepare_archive_none_for_a_failed_restart() {
    let f = fixture();
    let base = f.env.base_dir();
    start(&f, None, &playing(&f)).unwrap();
    let meta = record::task_dir(&base, "issue-41");
    let before = fs::read(meta.join("task.json")).unwrap();

    let result = start(
        &f,
        Some(&Restart {
            confirm: &Yes,
            yes: true,
        }),
        &playing(&f).with_failing_create_for("sbxm-task-issue-41-claude"),
    );

    assert!(result.is_err());
    assert_eq!(
        fs::read(meta.join("task.json")).unwrap(),
        before,
        "the old task is back"
    );
    assert!(failed_archives(&base, "issue-41").is_empty());
}
