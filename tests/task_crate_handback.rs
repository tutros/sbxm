//! Issue 403, decision 291: a worker never adds a crate under a strict profile. The worker
//! prompts say to hand it back, and a branch that changes `Cargo.lock` stops before its gates
//! with the same stop as issue 402's. One test per boundary case of the issue: each changes the
//! lock file, so each must hit the stop.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Env, git, task_fixture};
use sbxm::backend::FakeBackend;
use sbxm::commands::template_build;
use sbxm::github::fake::FakeGitHub;
use sbxm::harness::Harness;
use sbxm::task::config::Prompts;
use sbxm::task::pipeline;
use sbxm::task::prompts::{self, Role};
use sbxm::task::record;
use sbxm::task::stale_lock::BRANCH_CHANGED;

const PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     strict = true\n\
     allow = [\"crates.io\"]\n";

/// `main`'s lock file: `x` depends on `nix`, which brings `libc` in.
const BASE_LOCK: &str = "version = 4\n\n\
     [[package]]\nname = \"libc\"\nversion = \"0.2.150\"\n\n\
     [[package]]\nname = \"nix\"\nversion = \"0.27.0\"\ndependencies = [\"libc\"]\n\n\
     [[package]]\nname = \"x\"\nversion = \"0.1.0\"\ndependencies = [\"nix\"]\n";

#[test]
fn every_worker_prompt_says_to_hand_a_missing_crate_back_instead_of_adding_it() {
    for role in [Role::Worker, Role::WorkerSpec, Role::WorkerPrFix] {
        let text = prompts::template(role, &Prompts::default()).unwrap().text;
        for part in [
            "don't edit Cargo.toml or Cargo.lock",
            "the user approves\nevery new crate",
            "hand it back under \"Needs a decision\", naming the crate, its version and\nwhy it's needed",
            "A branch whose commits change Cargo.lock is stopped before its gates",
        ] {
            assert!(text.contains(part), "{role:?}: {part}");
        }
    }
}

/// Commits `Cargo.toml` and [`BASE_LOCK`] onto `main` of the stand-in origin; returns a checkout.
fn main_with_lock(f: &task_fixture::Fixture) -> PathBuf {
    let scratch = f.env.tmp.path().join("main-lock");
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
    fs::write(scratch.join("Cargo.lock"), BASE_LOCK).unwrap();
    git(&scratch, &["add", "-A"]);
    git(&scratch, &["commit", "-q", "-m", "lock"]);
    git(
        &scratch,
        &["push", "-q", f.origin.to_str().unwrap(), "HEAD:main"],
    );
    scratch
}

fn build_warm(env: &Env, backend: &FakeBackend, repo: &std::path::Path, harness: Harness) {
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness,
            warm: Some(repo.to_path_buf()),
            ..Default::default()
        },
        backend,
        &record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
}

/// Issue 41's worker commits `lock` as `Cargo.lock`, with both templates warmed from `main`; the
/// error its review stops with, and the backend.
fn worker_commits_lock(lock: &'static str) -> (String, FakeBackend) {
    let f = task_fixture::fixture();
    f.env.write_profile("default", PROFILE);
    let main = main_with_lock(&f);
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("claude", task_fixture::ok(task_fixture::CLAUDE_DONE))
        .with_exec_output_matching(
            "cargo metadata",
            task_fixture::ok(r#"{"packages":[{"name":"x"}],"version":1}"#),
        )
        .with_exec_hook(move |sandbox, spec| {
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
        });
    build_warm(&f.env, &backend, &main, Harness::Claude);
    build_warm(&f.env, &backend, &main, Harness::Codex);
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
    let ctx = task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &task_fixture::issue_text(41)).unwrap();
    pipeline::run_worker(&ctx, &mut prepared).unwrap();
    let err = format!(
        "{:#}",
        pipeline::review_issue(&ctx.env(), &mut prepared).unwrap_err()
    );
    (err, backend)
}

/// The stop of issue 402's part, before any gate or reviewer.
fn assert_handed_back(err: &str, backend: &FakeBackend) {
    for part in [
        BRANCH_CHANGED,
        "Needs a decision",
        "the user approves each one",
        "naming the crate, its version and why it's needed",
    ] {
        assert!(err.contains(part), "{part}\n{err}");
    }
    assert!(!ran_gates(backend));
    assert!(
        !backend
            .creates()
            .iter()
            .any(|c| c.name.contains("-review-"))
    );
}

#[test]
fn a_transitive_crate_made_a_direct_dependency_is_handed_back() {
    let (err, backend) = worker_commits_lock(
        "version = 4\n\n\
         [[package]]\nname = \"libc\"\nversion = \"0.2.150\"\n\n\
         [[package]]\nname = \"nix\"\nversion = \"0.27.0\"\ndependencies = [\"libc\"]\n\n\
         [[package]]\nname = \"x\"\nversion = \"0.1.0\"\ndependencies = [\"libc\", \"nix\"]\n",
    );
    assert_handed_back(&err, &backend);
}

#[test]
fn a_dev_dependency_only_is_handed_back() {
    let (err, backend) = worker_commits_lock(
        "version = 4\n\n\
         [[package]]\nname = \"libc\"\nversion = \"0.2.150\"\n\n\
         [[package]]\nname = \"nix\"\nversion = \"0.27.0\"\ndependencies = [\"libc\"]\n\n\
         [[package]]\nname = \"pretty_assertions\"\nversion = \"1.4.0\"\n\n\
         [[package]]\nname = \"x\"\nversion = \"0.1.0\"\n\
         dependencies = [\"nix\", \"pretty_assertions\"]\n",
    );
    assert_handed_back(&err, &backend);
}

#[test]
fn a_version_bump_of_an_existing_crate_is_handed_back() {
    let (err, backend) = worker_commits_lock(
        "version = 4\n\n\
         [[package]]\nname = \"libc\"\nversion = \"0.2.150\"\n\n\
         [[package]]\nname = \"nix\"\nversion = \"0.28.0\"\ndependencies = [\"libc\"]\n\n\
         [[package]]\nname = \"x\"\nversion = \"0.1.0\"\ndependencies = [\"nix\"]\n",
    );
    assert_handed_back(&err, &backend);
}

#[test]
fn a_crate_with_a_build_script_is_handed_back() {
    let (err, backend) = worker_commits_lock(
        "version = 4\n\n\
         [[package]]\nname = \"cc\"\nversion = \"1.0.83\"\n\n\
         [[package]]\nname = \"libc\"\nversion = \"0.2.150\"\n\n\
         [[package]]\nname = \"nix\"\nversion = \"0.27.0\"\ndependencies = [\"libc\"]\n\n\
         [[package]]\nname = \"openssl-sys\"\nversion = \"0.9.96\"\n\
         dependencies = [\"cc\", \"libc\"]\n\n\
         [[package]]\nname = \"x\"\nversion = \"0.1.0\"\ndependencies = [\"nix\", \"openssl-sys\"]\n",
    );
    assert_handed_back(&err, &backend);
}

/// Whether a gate ran in one of the task's sandboxes (a template build's warm-up runs cargo too).
fn ran_gates(backend: &FakeBackend) -> bool {
    backend.execs().iter().any(|(sandbox, spec)| {
        sandbox.starts_with("sbxm-task-") && spec.argv.iter().any(|a| a.contains("cargo test"))
    })
}
