//! Issue 424: a task whose branch only inherits older lock files than `main`'s, behind a strict
//! warm template that follows `main` (decision 290), is refused by `stale_lock::refresh` with the
//! way out that exists for it (`task fix --pr N`, or `task run --restart`), never a template
//! rebuild the next task would undo. When sbxm refuses doesn't change. One test per boundary case.
//!
//! The tests whose `main` moves rebuild the template, whose clone failed now and then on Linux CI
//! (issue 468): they run [`traced`], so a failure ends with the last lines of git's trace.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::git_trace::traced;
use common::{Env, git, task_fixture};
use sbxm::backend::FakeBackend;
use sbxm::commands::{new, template_build};
use sbxm::github::fake::FakeGitHub;
use sbxm::harness::Harness;
use sbxm::task::pipeline;
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

const LOCK: &str = "version = 4\n";
const MOVED: &str = "version = 4\n# on main\n";
const BUILT_AT: u64 = 1_700_000_000;

/// The played worker, made once the fixture's base dir is known.
type Hook = Box<dyn Fn(&str, &sbxm::backend::ExecSpec) + Send + Sync>;

/// Commits `Cargo.toml` and a `Cargo.lock` holding `lock` onto `main` of the stand-in origin, and
/// returns a checkout of that commit.
fn push_main_lock(f: &task_fixture::Fixture, name: &str, lock: &str) -> PathBuf {
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
            "HEAD:refs/heads/main",
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

fn build_warm(env: &Env, backend: &FakeBackend, repo: &Path, harness: Harness) {
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness,
            warm: Some(repo.to_path_buf()),
            now: Some(BUILT_AT),
            ..Default::default()
        },
        backend,
        &record::SystemProbe,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
}

/// A strict profile whose claude and codex templates were warmed from `main` holding [`LOCK`],
/// and a backend whose worker plays `play` (given the base dir).
fn warmed(play: impl FnOnce(&Path) -> Hook) -> (task_fixture::Fixture, FakeBackend) {
    let f = task_fixture::fixture();
    f.env.write_profile("default", PROFILE);
    let main = push_main_lock(&f, "main-lock", LOCK);
    let backend = backend().with_exec_hook(play(&f.env.base_dir()));
    build_warm(&f.env, &backend, &main, Harness::Claude);
    build_warm(&f.env, &backend, &main, Harness::Codex);
    (f, backend)
}

fn issue_worker(commits: &[&str]) -> impl FnOnce(&Path) -> Hook {
    let commits: Vec<String> = commits.iter().map(|c| (*c).to_owned()).collect();
    move |base_dir| {
        Box::new(task_fixture::play_tasks(
            base_dir,
            "main",
            task_fixture::Play {
                commits,
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ))
    }
}

/// Issue 41's task, its worker run; then `main` moves to `lock` when given. Returns the review's
/// result.
fn review_41(
    commits: &[&str],
    main_moves: Option<&str>,
    edit: impl FnOnce(&mut record::Record),
) -> (task_fixture::Fixture, FakeBackend, anyhow::Result<()>) {
    let (f, backend) = warmed(issue_worker(commits));
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
    let ctx = task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &task_fixture::issue_text(41)).unwrap();
    let _ = pipeline::run_worker(&ctx, &mut prepared);
    if let Some(lock) = main_moves {
        push_main_lock(&f, "main-moved", lock);
    }
    edit(&mut prepared.record);
    let review = pipeline::review_issue(&ctx.env(), &mut prepared).map(|_| ());
    (f, backend, review)
}

fn review_sandbox_made(backend: &FakeBackend) -> bool {
    backend
        .creates()
        .iter()
        .any(|c| c.name.contains("-review-"))
}

fn refused(review: anyhow::Result<()>) -> String {
    format!("{:#}", review.unwrap_err())
}

#[test]
fn an_issue_task_with_a_recorded_pr_behind_main_names_task_fix_with_that_pr() {
    traced(
        "an_issue_task_with_a_recorded_pr_behind_main_names_task_fix_with_that_pr",
        || {
            let (_f, backend, review) = review_41(&["a.txt"], Some(MOVED), |task| {
                task.pr = Some("https://github.com/o/r/pull/9".into());
            });

            let err = refused(review);
            assert!(err.contains("`sbxm task fix --pr 9`"), "{err}");
            assert!(err.contains("decision 290"), "{err}");
            assert!(!err.contains("template build"), "{err}");
            assert!(!err.contains("task resume"), "{err}");
            assert!(!err.contains("--restart"), "{err}");
            assert!(!review_sandbox_made(&backend));
        },
    );
}

#[test]
fn an_issue_task_without_a_pr_behind_main_names_task_run_restart_issue() {
    traced(
        "an_issue_task_without_a_pr_behind_main_names_task_run_restart_issue",
        || {
            let (_f, backend, review) = review_41(&["a.txt"], Some(MOVED), |_| {});

            let err = refused(review);
            assert!(
                err.contains("`sbxm task run --restart --issue 41`"),
                "{err}"
            );
            assert!(!err.contains("template build"), "{err}");
            assert!(!err.contains("task resume"), "{err}");
            assert!(!err.contains("task fix"), "{err}");
            assert!(!err.contains(BRANCH_CHANGED), "{err}");
            assert!(!review_sandbox_made(&backend));
        },
    );
}

#[test]
fn a_spec_task_without_a_pr_behind_main_names_task_run_restart_spec() {
    traced(
        "a_spec_task_without_a_pr_behind_main_names_task_run_restart_spec",
        || {
            let f = task_fixture::fixture();
            f.env.write_profile("default", PROFILE);
            let main = push_main_lock(&f, "main-lock", LOCK);
            let spec = task_fixture::spec_file(&f, "Build a thing.\n");
            let (id, _) = record::spec_id(&spec).unwrap();
            let worker = task_fixture::play(
                &f.env.base_dir().join("tasks").join(&id),
                "main",
                &id,
                task_fixture::Play {
                    commits: vec!["a.txt".into()],
                    result_md: Some(b"done\n".to_vec()),
                    bundle_bytes: None,
                },
            );
            let backend = backend().with_exec_hook(worker);
            build_warm(&f.env, &backend, &main, Harness::Claude);
            build_warm(&f.env, &backend, &main, Harness::Codex);
            let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
            let ctx = task_fixture::ctx(&f, &source, &backend, &github);
            let mut prepared = pipeline::prepare_spec(&ctx, &spec).unwrap();
            pipeline::run_worker(&ctx, &mut prepared).unwrap();
            push_main_lock(&f, "main-moved", MOVED);

            let err = refused(pipeline::review_issue(&ctx.env(), &mut prepared).map(|_| ()));

            assert!(
                err.contains("`sbxm task run --restart --spec <file>`"),
                "{err}"
            );
            assert!(!err.contains("template build"), "{err}");
            assert!(!err.contains("task resume"), "{err}");
            assert!(!err.contains("--issue"), "{err}");
            assert!(!review_sandbox_made(&backend));
        },
    );
}

#[test]
fn a_branch_whose_lock_equals_the_templates_is_not_refused() {
    let (_f, backend, review) = review_41(&["a.txt"], None, |_| {});

    let made = review_sandbox_made(&backend);
    if let Err(err) = review {
        let err = format!("{err:#}");
        assert!(!err.contains("task run --restart"), "{err}");
        assert!(!err.contains("decision 290"), "{err}");
    }
    assert!(made);
}

/// Decision 291, unchanged: the branch's own lock change is the hand-back, not the new hint.
#[test]
fn a_branch_that_changes_the_lock_itself_gets_the_hand_back_unchanged() {
    traced(
        "a_branch_that_changes_the_lock_itself_gets_the_hand_back_unchanged",
        || {
            let (_f, backend, review) = review_41(&["Cargo.lock"], Some(MOVED), |_| {});

            let err = refused(review);
            assert!(err.contains(BRANCH_CHANGED), "{err}");
            assert!(err.contains("Needs a decision"), "{err}");
            assert!(!err.contains("task run --restart"), "{err}");
            assert!(!review_sandbox_made(&backend));
        },
    );
}

/// `sbxm new` behind a strict warm template with other lock files: `template::choose`'s refusal,
/// word for word as before issue 424.
#[test]
fn sbxm_new_behind_a_template_with_other_lock_files_keeps_chooses_wording() {
    let env = Env::new();
    env.write_profile("default", PROFILE);
    let repo = env.tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    fs::write(repo.join("Cargo.lock"), LOCK).unwrap();
    let backend = backend();
    build_warm(&env, &backend, &repo, Harness::Claude);
    let workspace = env.base_dir().join("demo");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("Cargo.lock"), "version = 3\n").unwrap();

    let result = new::run(
        &env.config_dir(),
        "demo",
        &new::Options::default(),
        &backend,
        &mut Vec::new(),
    );

    assert_eq!(
        format!("{:#}", result.unwrap_err()),
        format!(
            "the template of profile 'default' for claude is stale: the lock files of {} \
             (Cargo.lock) differ from the ones it was warmed with; rebuild it with `sbxm \
             template build default --warm <repo>`",
            workspace.display()
        )
    );
}
