//! Issue 435: before every fix round the worker's `.sbxm-task/` source file (`issue.md`, or a
//! spec task's `source.md`) is written again from the task folder's copy, so a source updated
//! after the start reaches the worker as it reaches the reviewer. A local clone gets it written
//! in place, a cloud clone by `copy_in`.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::cloud_task::{
    CLEAN, HOME, cloud_backend_reviewing, identity, spec, task_record, task_run,
};
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks, worked_task,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::task_resume::{Options, Target, run};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, ProcessProbe, Stage};

const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";
const UPDATED: &str = "The source, with the open point answered after the start.\n";

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn resume(f: &Fixture, target: Target, backend: &FakeBackend) -> String {
    let opts = Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target,
        rounds: Some(1),
        should_fix: false,
        disk: Default::default(),
        identity: Some(identity()),
        forget_ambiguous: false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run(
        &f.env.config_dir(),
        &opts,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let out = format!(
        "{}{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap()
    );
    result.expect(&out);
    out
}

fn issue_backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
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

fn worker_copy(clone: &Path, name: &str) -> String {
    fs::read_to_string(clone.join(".sbxm-task").join(name)).unwrap()
}

#[test]
fn a_local_issue_task_s_fix_round_gets_the_issue_md_updated_since_the_start() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    );
    let first = issue_backend(&f, &[ONE, ONE]);
    let mut prepared = worked_task(&f, &first);
    let env = sbxm::task::pipeline::TaskEnv {
        progress: &sbxm::task::pipeline::quiet,
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &first,
        probe: &Gone,
        host: &FakeHostRunner::default(),
        sleep: &std::thread::sleep,
        scoped_secrets: &[],
        disk: Box::leak(Box::default()),
        identity: None,
        clone_source: None,
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
    let clone = prepared.workspace.clone();
    assert_ne!(worker_copy(&clone, "issue.md"), UPDATED);
    fs::write(prepared.meta.join("issue.md"), UPDATED).unwrap();

    resume(&f, Target::Issue(41), &issue_backend(&f, &[ONE]));

    assert_eq!(worker_copy(&clone, "issue.md"), UPDATED);
}

#[test]
fn a_cloud_spec_task_s_fix_round_gets_the_source_md_updated_since_the_start_by_copy() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n",
    );
    let (spec, id) = spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let clone = root.join(format!("{}/work/{id}", HOME.trim_start_matches('/')));
    let meta = record::task_dir(&f.env.base_dir(), &id);
    // The first review is when the source is updated (an open point answered); the fix round
    // after it, in the same worker sandbox and clone, must see the new text.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (worker_clone, seen_by_review) = (clone.clone(), seen.clone());
    let backend = cloud_backend_reviewing(
        &root,
        &[&id],
        &[ONE, CLEAN],
        true,
        ok(CLAUDE_DONE),
        move |_id| {
            seen_by_review
                .lock()
                .unwrap()
                .push(worker_copy(&worker_clone, "source.md"));
            fs::write(meta.join("source.md"), UPDATED).unwrap();
        },
    );

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.expect(&ran.out);
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.round), (Stage::Ready, 1));
    let seen = seen.lock().unwrap();
    assert_ne!(seen[0], UPDATED, "the worker started with the old text");
    assert!(
        seen[1..].iter().all(|text| text == UPDATED),
        "the fix round's worker had {seen:?}"
    );
    let control = format!("{HOME}/work/{id}/.sbxm-task");
    let copies = backend
        .log()
        .iter()
        .filter(|l| l.starts_with("cp-in") && l.ends_with(&format!("{control}/source.md")))
        .count();
    assert_eq!(
        copies,
        2,
        "at the start and before the fix round: {:?}",
        backend.log()
    );
}
