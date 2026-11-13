//! Issue 623: a task started by hand (`task start --issue N`, `task run --issue N`) takes the
//! loop's `ready-for-worker` label off its issue once the task's record exists, as the loop does,
//! so the issue leaves the loop's queue. A failed removal is a warning, never the task's failure.

mod common;

use std::sync::{Arc, Mutex};

use common::cloud_task;
use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, backend, fixture, fixture_with, issue_text, ok,
    open_issue, play, play_reviews, play_tasks, source,
};
use sbxm::backend::{ExecOutput, FakeBackend, Location};
use sbxm::commands::{task_run, task_start};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{GitHubBackend, Issue};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record;
use sbxm::task::repo::Identity;

const READY: &str = "ready-for-worker";
const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";

fn identity() -> Identity {
    Identity {
        name: "Dev".into(),
        email: "dev@example.com".into(),
    }
}

fn start_options(f: &Fixture) -> task_start::Options {
    task_start::Options {
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
        identity: Some(identity()),
        disk: Default::default(),
        location: None,
    }
}

fn github(labels: &[&str]) -> FakeGitHub {
    FakeGitHub::default()
        .with_default_branch("main")
        .with_issue_text(issue_text(41))
        .with_open_issues(vec![open_issue(41, labels, "")])
}

fn removal() -> GhCall {
    GhCall::IssueLabels("o/r".into(), 41, Vec::new(), vec![READY.to_owned()])
}

fn removals(github: &FakeGitHub) -> usize {
    github
        .calls()
        .iter()
        .filter(|c| matches!(c, GhCall::IssueLabels(..)))
        .count()
}

fn labels_now(github: &FakeGitHub) -> Vec<String> {
    let open: Vec<Issue> = github.issues_open("o/r").unwrap();
    open.into_iter()
        .find(|i| i.number == 41)
        .map(|i| i.labels)
        .unwrap_or_default()
}

/// The worker's sandbox plays its commits; each `claude` call first notes the GitHub calls made
/// so far, so a test sees what had happened by the time the worker ran.
fn playing(f: &Fixture, github: &Arc<FakeGitHub>, seen: &Arc<Mutex<Vec<GhCall>>>) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let (github, seen) = (Arc::clone(github), Arc::clone(seen));
    backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(move |sandbox, spec| {
            if spec.argv.iter().any(|a| a.contains("claude")) && seen.lock().unwrap().is_empty() {
                *seen.lock().unwrap() = github.calls();
            }
            worker(sandbox, spec);
        })
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn start(f: &Fixture, backend: &FakeBackend, github: &FakeGitHub) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_start::run(
        &f.env.config_dir(),
        &start_options(f),
        backend,
        github,
        &Probe,
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn task_json(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")
}

#[test]
fn a_start_by_hand_takes_the_label_off_once_the_record_exists() {
    let f = fixture();
    let gh = Arc::new(github(&[READY, "size:S"]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let b = playing(&f, &gh, &seen);

    let out = start(&f, &b, &gh);

    out.result.unwrap();
    assert!(task_json(&f).is_file());
    // Taken off before the worker ran: the record was written then, the label gone with it.
    assert!(seen.lock().unwrap().contains(&removal()), "{:?}", seen);
    assert_eq!(removals(&gh), 1, "{:?}", gh.calls());
    assert_eq!(labels_now(&gh), vec!["size:S".to_owned()]);
    assert!(!out.warn.contains("label"), "{}", out.warn);
}

#[test]
fn a_run_by_hand_takes_the_label_off_too() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    );
    let gh = Arc::new(github(&[READY]));
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), "issue-41", vec![CLEAN.to_owned()]);
    let b = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    let opts = task_run::Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issue: 41,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: None,
        repo: Some("o/r".into()),
        clone_source: Some(source(&f)),
        identity: Some(identity()),
        disk: Default::default(),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = task_run::run(
        &f.env.config_dir(),
        &opts,
        None,
        &b,
        gh.as_ref(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );

    result.unwrap();
    assert!(gh.calls().contains(&removal()), "{:?}", gh.calls());
    assert_eq!(removals(&gh), 1, "{:?}", gh.calls());
    assert!(labels_now(&gh).is_empty());
}

#[test]
fn an_issue_without_the_label_has_nothing_taken_off_and_its_task_runs() {
    let f = fixture();
    let gh = Arc::new(github(&["should-fix"]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let b = playing(&f, &gh, &seen);

    let out = start(&f, &b, &gh);

    out.result.unwrap();
    assert!(out.out.contains("completed"), "{}", out.out);
    assert_eq!(removals(&gh), 0, "{:?}", gh.calls());
    assert!(!out.warn.contains("label"), "{}", out.warn);
}

#[test]
fn a_failed_removal_is_a_warning_and_the_task_still_starts() {
    let f = fixture();
    let gh = Arc::new(github(&[READY]).failing_issue_labels("HTTP 502"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let b = playing(&f, &gh, &seen);

    let out = start(&f, &b, &gh);

    out.result.unwrap();
    assert!(out.out.contains("completed"), "{}", out.out);
    assert_eq!(removals(&gh), 1, "{:?}", gh.calls());
    assert!(
        out.warn.contains(&format!(
            "warning: issue #41's {READY} label could not be removed: HTTP 502"
        )),
        "{}",
        out.warn
    );
}

#[test]
fn a_spec_task_makes_no_label_call() {
    let f = fixture();
    let spec = f.env.tmp.path().join("idea.md");
    std::fs::write(&spec, "Build a thing.\n").unwrap();
    let (id, _) = record::spec_id(&spec).unwrap();
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join(&id),
            "main",
            &id,
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let gh = FakeGitHub::default();
    let opts = task_start::SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(&f)),
        identity: Some(identity()),
        disk: Default::default(),
        location: None,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    task_start::run_spec(
        &f.env.config_dir(),
        &opts,
        &b,
        &gh,
        &Probe,
        &mut out,
        &mut warn,
    )
    .unwrap();

    assert_eq!(removals(&gh), 0, "{:?}", gh.calls());
}

/// A local start whose sandbox can't be made keeps no record of the task: the issue is still
/// free, so it keeps its label.
#[test]
fn a_local_start_whose_sandbox_fails_keeps_the_label() {
    let f = fixture();
    let gh = Arc::new(github(&[READY]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let b = playing(&f, &gh, &seen).with_failing_create_for("sbxm-task-issue-41-claude");

    let out = start(&f, &b, &gh);

    assert!(out.result.is_err());
    assert!(!task_json(&f).exists());
    assert_eq!(removals(&gh), 0, "{:?}", gh.calls());
}

/// A cloud start whose warm-up fails keeps its record, `prepared/failed`, for `task resume`
/// (issue 560): the issue is taken, so its label goes even though the preparation failed.
#[test]
fn a_cloud_start_whose_warm_up_fails_still_takes_the_label_off() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n",
    );
    f.env.write_profile(
        "default",
        "[template]\nstrict = true\nallow = [\"crates.io\"]\n",
    );
    cargo_base(&f);
    let gh = github(&[READY]);
    let b = cloud_task::cloud_backend(&f.env.tmp.path().join("cloud-fs"), "issue-41", &[CLEAN])
        .with_cloud_allow(&[
            "api.anthropic.com",
            "claude.ai",
            "console.anthropic.com",
            "crates.io",
            "index.crates.io",
        ])
        .with_exec_output_matching(
            "cargo fetch --locked",
            ExecOutput {
                stdout: String::new(),
                stderr: "no network\n".into(),
                exit_code: Some(101),
            },
        );
    let mut opts = start_options(&f);
    opts.location = Some(Location::Cloud);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = task_start::run(
        &f.env.config_dir(),
        &opts,
        &b,
        &gh,
        &Probe,
        &mut out,
        &mut warn,
    );

    assert!(result.is_err());
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("warm-up"), "{out}");
    assert!(task_json(&f).is_file(), "{out}");
    assert_eq!(removals(&gh), 1, "{:?}", gh.calls());
    assert!(labels_now(&gh).is_empty());
}

/// Adds a `Cargo.toml` and its lock to `main` in the fixture's origin, so a strict sandbox has
/// something to warm up.
fn cargo_base(f: &Fixture) {
    let seed = f.env.tmp.path().join("origin-cargo");
    common::git(
        f.env.tmp.path(),
        &["clone", "-q", &source(f), seed.to_str().unwrap()],
    );
    std::fs::write(seed.join("Cargo.toml"), "[package]\nname = \"t\"\n").unwrap();
    std::fs::write(seed.join("Cargo.lock"), "version = 4\n").unwrap();
    common::git(&seed, &["add", "-A"]);
    common::git(&seed, &["commit", "-q", "-m", "cargo"]);
    common::git(&seed, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_start_refused_before_its_record_keeps_the_label() {
    let f = fixture();
    let gh = Arc::new(github(&[READY]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let b = playing(&f, &gh, &seen);
    let mut opts = start_options(&f);
    opts.disk = common::slot::full(&f.env, true);
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = task_start::run(
        &f.env.config_dir(),
        &opts,
        &b,
        gh.as_ref(),
        &Probe,
        &mut out,
        &mut warn,
    );

    assert!(result.is_err());
    assert!(!task_json(&f).exists());
    assert_eq!(removals(&gh), 0, "{:?}", gh.calls());
}
