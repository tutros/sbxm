//! Issue 402, decision 290: a strict profile's warm template whose lock files differ from
//! `main`'s is rebuilt from a clean checkout of `main`'s tip before the task's sandbox is created;
//! a task whose own branch changes them stops with a hand-back. One test per boundary case.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{Env, git, task_fixture};
use sbxm::backend::FakeBackend;
use sbxm::commands::template_build;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::harness::Harness;
use sbxm::task::pipeline;
use sbxm::task::record;
use sbxm::task::stale_lock::BRANCH_CHANGED;
use sbxm::template;

const PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     strict = true\n\
     allow = [\"crates.io\"]\n";

const LOCK: &str = "version = 4\n";
/// The first template build's clock, well before any rebuild's (each build gets a fresh tag,
/// decision 211).
const BUILT_AT: u64 = 1_700_000_000;

/// Commits `Cargo.toml` and a `Cargo.lock` holding `lock` onto the stand-in origin at `refname`,
/// on top of `main`, and returns a checkout of that commit.
fn push_lock(f: &task_fixture::Fixture, name: &str, lock: &str, refname: &str) -> PathBuf {
    let scratch = f.env.tmp.path().join(name);
    git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            scratch.to_str().unwrap(),
        ],
    );
    fs::write(scratch.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    fs::write(scratch.join("Cargo.lock"), lock).unwrap();
    fs::write(scratch.join("pushed.txt"), format!("{name}\n")).unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", name]);
    git(
        &scratch,
        &[
            "push",
            "-q",
            "-f",
            f.origin.to_str().unwrap(),
            &format!("HEAD:{refname}"),
        ],
    );
    scratch
}

fn backend() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("claude", task_fixture::ok(task_fixture::CLAUDE_DONE))
        .with_exec_output_matching("codex", task_fixture::ok(task_fixture::CODEX_DONE))
        .with_exec_output_matching(
            "cargo metadata",
            task_fixture::ok(r#"{"packages":[{"name":"x"}],"version":1}"#),
        )
}

/// Builds the warm template of `default` for `harness` from `repo` at `now`, returning its tag.
fn build_warm(env: &Env, backend: &FakeBackend, repo: &Path, harness: Harness, now: u64) -> String {
    let mut out = Vec::new();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness,
            warm: Some(repo.to_path_buf()),
            now: Some(now),
            ..Default::default()
        },
        backend,
        &record::SystemProbe,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

/// How many template build sandboxes were created.
fn builds(backend: &FakeBackend) -> usize {
    backend
        .creates()
        .iter()
        .filter(|c| c.name.starts_with("sbxm-template-"))
        .count()
}

fn worker_template(backend: &FakeBackend) -> Option<String> {
    backend
        .creates()
        .into_iter()
        .rfind(|c| c.name == "sbxm-task-issue-41-claude")
        .and_then(|c| c.template)
}

fn record_of(env: &Env, harness: Harness) -> template::Record {
    template::Record::load(&env.base_dir(), &template::key("default", harness))
        .unwrap()
        .unwrap()
}

/// A strict profile whose claude template was warmed from `main` holding [`LOCK`].
fn warmed() -> (task_fixture::Fixture, FakeBackend, String) {
    let f = task_fixture::fixture();
    f.env.write_profile("default", PROFILE);
    let main = push_lock(&f, "main-lock", LOCK, "refs/heads/main");
    let backend = backend();
    let tag = build_warm(&f.env, &backend, &main, Harness::Claude, BUILT_AT);
    (f, backend, tag)
}

fn prepare_41(
    f: &task_fixture::Fixture,
    backend: &FakeBackend,
) -> anyhow::Result<pipeline::Prepared> {
    let (github, source) = (FakeGitHub::default(), task_fixture::source(f));
    pipeline::prepare(
        &task_fixture::ctx(f, &source, backend, &github),
        &task_fixture::issue_text(41),
    )
}

#[test]
fn an_unchanged_lock_file_rebuilds_nothing() {
    let (f, backend, tag) = warmed();

    let prepared = prepare_41(&f, &backend).unwrap();

    assert_eq!(builds(&backend), 1);
    assert_eq!(worker_template(&backend).as_deref(), Some(tag.as_str()));
    assert!(
        !prepared.events.iter().any(|e| e.contains("rebuilt")),
        "{:?}",
        prepared.events
    );
}

#[test]
fn a_lock_file_changed_by_one_line_on_main_rebuilds_the_template_from_mains_tip() {
    let (f, backend, old) = warmed();
    let main = push_lock(
        &f,
        "main-moved",
        "version = 4\n# one more line\n",
        "refs/heads/main",
    );
    let tip = git(&main, &["rev-parse", "HEAD"]);

    let prepared = prepare_41(&f, &backend).unwrap();

    assert_eq!(builds(&backend), 2);
    let rebuilt = record_of(&f.env, Harness::Claude);
    assert_ne!(rebuilt.tag, old);
    assert_eq!(
        rebuilt.lock_hash.unwrap(),
        template::lock_hash(&main, &["Cargo.lock".to_owned()]).unwrap()
    );
    assert_eq!(worker_template(&backend), Some(rebuilt.tag.clone()));
    let line = prepared
        .events
        .iter()
        .find(|e| e.contains("rebuilt it from main's tip"))
        .unwrap_or_else(|| panic!("{:?}", prepared.events));
    assert!(line.contains(&tip) && line.contains(&rebuilt.tag), "{line}");
    // The checkout of main it was warmed from is gone.
    assert!(!prepared.meta.join("template-base-claude").exists());
}

#[test]
fn a_lock_file_changed_only_in_line_endings_rebuilds_nothing() {
    let (f, backend, tag) = warmed();
    push_lock(&f, "main-crlf", "version = 4\r\n", "refs/heads/main");

    prepare_41(&f, &backend).unwrap();

    assert_eq!(builds(&backend), 1);
    assert_eq!(worker_template(&backend).as_deref(), Some(tag.as_str()));
}

#[test]
fn two_tasks_needing_the_same_rebuild_make_one_build_and_the_second_waits_on_the_lock() {
    let (f, backend, _) = warmed();
    let main = push_lock(
        &f,
        "main-moved",
        "version = 4\n# one more line\n",
        "refs/heads/main",
    );
    // Another task's rebuild holds the build lock, and its process is alive.
    let lock = template::dir(&f.env.base_dir()).join("default-claude.build.json");
    fs::write(
        &lock,
        serde_json::to_string(&record::Process::current(&task_fixture::Probe)).unwrap(),
    )
    .unwrap();
    let waits = AtomicUsize::new(0);
    let other_build = |_: Duration| {
        // The other task's build finishes while this one waits.
        if waits.fetch_add(1, Ordering::SeqCst) == 0 {
            fs::remove_file(&lock).unwrap();
            build_warm(&f.env, &backend, &main, Harness::Claude, BUILT_AT + 3600);
        }
    };
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
    let mut ctx = task_fixture::ctx(&f, &source, &backend, &github);
    ctx.sleep = &other_build;

    let prepared = pipeline::prepare(&ctx, &task_fixture::issue_text(41)).unwrap();

    assert_eq!(waits.load(Ordering::SeqCst), 1);
    // The first build and the other task's: this task built nothing itself.
    assert_eq!(builds(&backend), 2);
    let rebuilt = record_of(&f.env, Harness::Claude);
    assert_eq!(worker_template(&backend), Some(rebuilt.tag.clone()));
    assert!(
        prepared
            .events
            .iter()
            .any(|e| e.contains("another task rebuilt it from main meanwhile")),
        "{:?}",
        prepared.events
    );
}

/// Plays issue 41's worker: on its headless run it commits `Cargo.lock` holding `lock`.
fn changes_lock(f: &task_fixture::Fixture, lock: &'static str) -> FakeBackend {
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    backend().with_exec_hook(move |sandbox, spec| {
        if sandbox.contains("-review-") {
            return;
        }
        if task_fixture::is_headless(spec) {
            fs::write(workspace.join("Cargo.lock"), lock).unwrap();
            git(&workspace, &["commit", "-q", "-am", "add a crate"]);
            let dir = workspace.join(".sbxm-task");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("result.md"), "done\n").unwrap();
        } else if task_fixture::is_bundle(spec) {
            git(
                &workspace,
                &[
                    "bundle",
                    "create",
                    ".sbxm-task/branch.bundle",
                    "issue-41",
                    "^origin/main",
                ],
            );
        }
    })
}

#[test]
fn a_task_whose_own_branch_changes_the_lock_file_stops_with_the_hand_back_and_rebuilds_nothing() {
    let f = task_fixture::fixture();
    f.env.write_profile("default", PROFILE);
    let main = push_lock(&f, "main-lock", LOCK, "refs/heads/main");
    let backend = changes_lock(&f, "version = 4\n# ctrlc 3.4.5\n");
    build_warm(&f.env, &backend, &main, Harness::Claude, BUILT_AT);
    build_warm(&f.env, &backend, &main, Harness::Codex, BUILT_AT);
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
    let ctx = task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &task_fixture::issue_text(41)).unwrap();
    pipeline::run_worker(&ctx, &mut prepared).unwrap();

    let err = format!(
        "{:#}",
        pipeline::review_issue(&ctx.env(), &mut prepared).unwrap_err()
    );

    for part in [
        BRANCH_CHANGED,
        "Needs a decision",
        "can't fetch crates (decision 291)",
        "naming the crate, its version and why it's needed",
        "then run `sbxm task resume --issue 41`",
    ] {
        assert!(err.contains(part), "{part}\n{err}");
    }
    assert_eq!(builds(&backend), 2);
    // Stopped before the gates and the reviewer.
    assert!(!ran_gates(&backend));
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name.contains("-review-"))
    );
}

fn pr_7() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "Adds x.".into(),
        closing_issues: vec![],
    }
}

#[test]
fn main_changing_the_lock_and_the_branch_changing_it_again_rebuilds_from_main_then_stops() {
    let f = task_fixture::fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    );
    f.env.write_profile("default", PROFILE);
    let main = push_lock(&f, "main-lock", LOCK, "refs/heads/main");
    let backend = backend();
    build_warm(&f.env, &backend, &main, Harness::Codex, BUILT_AT);
    let moved = push_lock(
        &f,
        "main-moved",
        "version = 4\n# on main\n",
        "refs/heads/main",
    );
    push_lock(&f, "pr-head", "version = 4\n# ctrlc\n", "refs/pull/7/head");
    let github = FakeGitHub::default().with_pr(pr_7());
    let source = task_fixture::source(&f);
    let ctx = task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = pipeline::prepare_pr(&ctx, &pr_7()).unwrap();

    let err = format!(
        "{:#}",
        pipeline::review_pr(&ctx.env(), &github, &mut prepared, false).unwrap_err()
    );

    assert_eq!(builds(&backend), 2);
    assert_eq!(
        record_of(&f.env, Harness::Codex).lock_hash.unwrap(),
        template::lock_hash(&moved, &["Cargo.lock".to_owned()]).unwrap()
    );
    assert!(err.contains(BRANCH_CHANGED), "{err}");
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name.contains("-review-"))
    );
}

#[test]
fn a_rebuild_that_fails_stops_the_task_naming_the_template_build_command() {
    let (f, backend, _) = warmed();
    // `main` now has a changed lock file and no Cargo.toml, so no warm build can run from it.
    let scratch = push_lock(
        &f,
        "main-broken",
        "version = 4\n# changed\n",
        "refs/heads/main",
    );
    git(&scratch, &["rm", "-q", "Cargo.toml"]);
    git(&scratch, &["commit", "-q", "-m", "no manifest"]);
    git(
        &scratch,
        &[
            "push",
            "-q",
            "-f",
            f.origin.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ],
    );

    let err = format!("{:#}", prepare_41(&f, &backend).unwrap_err());

    for part in [
        "rebuilding it from main's tip",
        "failed",
        "sbxm template build default --warm <a clean checkout of main>",
    ] {
        assert!(err.contains(part), "{part}\n{err}");
    }
    assert_eq!(worker_template(&backend), None);
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name.starts_with("sbxm-task-"))
    );
    assert!(!record::task_dir(&f.env.base_dir(), "issue-41").exists());
}

/// Whether a gate ran in one of the task's sandboxes (a template build's warm-up runs cargo too).
fn ran_gates(backend: &FakeBackend) -> bool {
    backend.execs().iter().any(|(sandbox, spec)| {
        sandbox.starts_with("sbxm-task-") && spec.argv.iter().any(|a| a.contains("cargo test"))
    })
}
