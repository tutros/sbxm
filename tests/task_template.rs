//! Milestone 3 slice 8: `sbxm task`'s sandboxes follow the profile's template (decisions 200, 203,
//! 206, 216).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Env, git, task_fixture};
use sbxm::backend::FakeBackend;
use sbxm::commands::{task_resume, task_review, template_build};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrInfo, PrState};
use sbxm::harness::Harness;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::pipeline;
use sbxm::task::record;
use sbxm::template::Locks;

#[test]
fn lock_files_at_a_commit_hash_like_the_same_files_in_a_folder() {
    let env = Env::new();
    let repo = env.tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    fs::write(repo.join("Cargo.lock"), "version = 4\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "lock"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("Cargo.lock"), "version = 4\n# changed\n").unwrap();
    git(&repo, &["commit", "-am", "changed lock"]);
    let files = ["Cargo.lock".to_owned()];
    let repo_git = repo.join(".git");

    let at_head = Locks::Commit {
        repo_git: &repo_git,
        commit: &git(&repo, &["rev-parse", "HEAD"]),
    }
    .hash(&files)
    .unwrap();
    let at_first = Locks::Commit {
        repo_git: &repo_git,
        commit: &first,
    }
    .hash(&files)
    .unwrap();

    assert_eq!(at_head, Locks::Dir(&repo).hash(&files).unwrap());
    assert_ne!(at_first, at_head);
    let missing = Locks::Commit {
        repo_git: &repo_git,
        commit: &first,
    }
    .hash(&["Cargo.toml".to_owned()]);
    assert!(missing.is_err());
}

const PROFILE: &str = "[network]\n\
     allow = [\"archive.ubuntu.com\"]\n\
     \n\
     [[setup.install]]\n\
     command = \"apt-get install -y ripgrep\"\n\
     \n\
     [template]\n\
     allow = [\"crates.io\"]\n";

fn strict(profile: &str) -> String {
    profile.replace("[template]\n", "[template]\nstrict = true\n")
}

fn sbx_046() -> FakeBackend {
    task_fixture::backend().with_version("0.46.0")
}

/// Builds the cold template of `default` for claude and returns its tag.
fn build_template(env: &Env, backend: &FakeBackend) -> String {
    let mut out = Vec::new();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness: Harness::Claude,
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
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

#[test]
fn an_issue_worker_is_created_from_the_profiles_template() {
    let f = task_fixture::fixture();
    f.env.write_profile("default", PROFILE);
    let backend = sbx_046();
    let tag = build_template(&f.env, &backend);
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));

    let prepared = pipeline::prepare(
        &task_fixture::ctx(&f, &source, &backend, &github),
        &task_fixture::issue_text(41),
    )
    .unwrap();

    let create = backend.creates().pop().unwrap();
    assert_eq!(create.template.as_deref(), Some(tag.as_str()));
    let common = fs::read_to_string(create.kits[0].join("spec.yaml")).unwrap();
    assert!(!common.contains("apt-get install -y ripgrep"), "{common}");
    assert!(common.contains("crates.io"), "{common}");
    let worker = prepared.record.worker.as_ref().unwrap();
    assert_eq!(worker.template.as_deref(), Some(tag.as_str()));
    let saved = record::read(&prepared.meta.join("task.json")).unwrap();
    assert_eq!(
        saved.worker.unwrap().template.as_deref(),
        Some(tag.as_str())
    );
}

#[test]
fn a_strict_profile_without_a_template_refuses_the_worker_before_its_sandbox() {
    let f = task_fixture::fixture();
    f.env.write_profile("default", &strict(PROFILE));
    let backend = sbx_046();
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));

    let err = pipeline::prepare(
        &task_fixture::ctx(&f, &source, &backend, &github),
        &task_fixture::issue_text(41),
    )
    .err()
    .unwrap();

    assert!(
        format!("{err:#}").contains("sbxm template build default"),
        "{err:#}"
    );
    assert!(backend.creates().is_empty());
    assert!(!f.env.base_dir().join("tasks").join("issue-41").exists());
    assert!(
        !f.env
            .base_dir()
            .join(".sbxm")
            .join("tasks")
            .join("issue-41")
            .exists()
    );
}

/// A PR review task's reviewer is `codex`, as in the PR review tests.
fn pr_fixture() -> task_fixture::Fixture {
    task_fixture::fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

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
    fs::write(scratch.join("pr.txt"), format!("{name}\n")).unwrap();
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

/// Builds the warm template of `default` for codex from `repo`.
fn build_warm_codex(env: &Env, backend: &FakeBackend, repo: &Path) -> String {
    build_warm(env, backend, repo, Harness::Codex)
}

/// Builds the warm template of `default` for `harness` from `repo`.
fn build_warm(env: &Env, backend: &FakeBackend, repo: &Path, harness: Harness) -> String {
    let mut out = Vec::new();
    template_build::run(
        &env.config_dir(),
        &template_build::Options {
            profile: "default".into(),
            harness,
            warm: Some(repo.to_path_buf()),
            ..Default::default()
        },
        backend,
        &sbxm::task::record::SystemProbe,
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

fn pr_7() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        state: PrState::Open,
        title: "Add the x feature".into(),
        body: "Adds x.".into(),
        closing_issues: vec![],
    }
}

fn pr_backend(f: &task_fixture::Fixture) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("codex", task_fixture::ok(task_fixture::CODEX_DONE))
        // The warm build's own step, as in `tests/template_build.rs`.
        .with_exec_output_matching(
            "cargo metadata",
            task_fixture::ok(r#"{"packages":[{"name":"x"}],"version":1}"#),
        )
        .with_exec_hook(task_fixture::play_reviews(
            &f.env.base_dir(),
            "pr-7",
            vec!["Must-fix findings: 0\n\nNothing found.\n".to_owned()],
        ))
}

/// A warm codex template built from `main`'s lock file, then PR 7 with `head_lock` at its head,
/// prepared and reviewed.
fn review_pr_7(
    profile: &str,
    head_lock: &str,
) -> (
    task_fixture::Fixture,
    FakeBackend,
    String,
    anyhow::Result<pipeline::PrReview>,
) {
    let (f, backend, tag, github) = setup_pr_7(profile, head_lock);
    let source = task_fixture::source(&f);
    let host = FakeHostRunner::default();
    let ctx = task_fixture::ctx_with_host(&f, &source, &backend, &github, &host);
    let mut prepared = pipeline::prepare_pr(&ctx, &pr_7()).unwrap();
    let result = pipeline::review_pr(&ctx.env(), &github, &mut prepared, false);
    (f, backend, tag, result)
}

fn setup_pr_7(
    profile: &str,
    head_lock: &str,
) -> (task_fixture::Fixture, FakeBackend, String, FakeGitHub) {
    let f = pr_fixture();
    f.env.write_profile("default", profile);
    let main = push_lock(&f, "main-lock", "version = 4\n", "refs/heads/main");
    let backend = pr_backend(&f);
    let tag = build_warm_codex(&f.env, &backend, &main);
    push_lock(&f, "pr-head", head_lock, "refs/pull/7/head");
    (f, backend, tag, FakeGitHub::default().with_pr(pr_7()))
}

#[test]
fn task_review_pr_prints_the_warning_of_a_template_it_could_not_use() {
    let (f, backend, _tag, github) = setup_pr_7(PROFILE, "version = 4\n# a new dependency\n");
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    task_review::run(
        &f.env.config_dir(),
        &task_review::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: task_review::Target::Pr(7),
            repo: Some("o/r".into()),
            base: Some("main".into()),
            clone_source: Some(task_fixture::source(&f)),
            reviewer_harness: None,
            reviewer_model: None,
            reviewer_time_limit: None,
            time_limit: None,
            profile: None,
            resolved_spec_id: None,
            force: false,
            disk: Default::default(),
        },
        &backend,
        &github,
        &task_fixture::Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )
    .unwrap();

    let warn = String::from_utf8(warn).unwrap();
    assert!(
        warn.lines().any(|l| l.starts_with("warning: ")
            && l.contains("stale")
            && l.contains("install steps")),
        "{warn}"
    );
}

fn reviewer_creates(backend: &FakeBackend) -> Vec<sbxm::backend::CreateSpec> {
    backend
        .creates()
        .into_iter()
        .filter(|c| c.name.contains("-review-"))
        .collect()
}

#[test]
fn a_pr_reviewer_is_created_from_the_template_its_head_matches() {
    let (f, backend, tag, result) = review_pr_7(PROFILE, "version = 4\n");

    let review = result.unwrap();
    assert!(review.reviewed.is_some(), "{review:?}");
    let creates = reviewer_creates(&backend);
    assert_eq!(creates.len(), 1, "{creates:?}");
    assert_eq!(creates[0].template.as_deref(), Some(tag.as_str()));
    let saved =
        record::read(&record::task_dir(&f.env.base_dir(), "pr-7").join("task.json")).unwrap();
    assert_eq!(
        saved.reviewer.unwrap().template.as_deref(),
        Some(tag.as_str())
    );
}

#[test]
fn a_pr_head_that_changes_the_lock_file_refuses_a_strict_reviewer_before_its_sandbox() {
    let (_f, backend, _tag, result) =
        review_pr_7(&strict(PROFILE), "version = 4\n# a new dependency\n");

    let err = result.err().unwrap();
    assert!(format!("{err:#}").contains("--warm"), "{err:#}");
    assert!(reviewer_creates(&backend).is_empty());
}

#[test]
fn a_non_strict_reviewer_whose_pr_head_changes_the_lock_file_warns_and_installs() {
    let (_f, backend, _tag, result) = review_pr_7(PROFILE, "version = 4\n# a new dependency\n");

    let review = result.unwrap();
    assert!(
        review
            .warnings
            .iter()
            .any(|w| w.contains("stale") && w.contains("install steps")),
        "{:?}",
        review.warnings
    );
    let creates = reviewer_creates(&backend);
    assert_eq!(creates.len(), 1, "{creates:?}");
    assert_eq!(creates[0].template, None);
}

const WORKER: &str = "sbxm-task-issue-41-claude";

/// Plays issue 41's worker (one commit) and a clean review, with `sbx` 0.46.0. The fake lists
/// no sandboxes, so a resumed worker's sandbox is always gone.
fn resume_backend(f: &task_fixture::Fixture) -> FakeBackend {
    let worker = task_fixture::play_tasks(
        &f.env.base_dir(),
        "main",
        task_fixture::Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = task_fixture::play_reviews(
        &f.env.base_dir(),
        "issue-41",
        vec!["Must-fix findings: 0\n\nNothing found.\n".to_owned()],
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("claude", task_fixture::ok(task_fixture::CLAUDE_DONE))
        .with_exec_output_matching("codex", task_fixture::ok(task_fixture::CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

/// Issue 41's worker created from the template and run once, then left failed as a crash would
/// leave it.
fn failed_worker(f: &task_fixture::Fixture, backend: &FakeBackend) -> String {
    f.env.write_profile("default", PROFILE);
    let tag = build_template(&f.env, backend);
    let mut prepared = task_fixture::worked_task(f, backend);
    assert_eq!(
        prepared.record.worker.as_ref().unwrap().template.as_deref(),
        Some(tag.as_str())
    );
    prepared.record.stage = record::Stage::Working;
    prepared.record.status = record::Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    tag
}

/// The process a record names is gone (the `sbxm` that wrote it was killed).
struct Gone;

impl record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn resume_41(
    f: &task_fixture::Fixture,
    backend: &FakeBackend,
) -> (anyhow::Result<()>, String, String) {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_resume::run(
        &f.env.config_dir(),
        &task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: task_resume::Target::Issue(41),
            rounds: None,
            disk: Default::default(),
        },
        backend,
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap(),
    )
}

#[test]
fn a_gone_worker_sandbox_is_made_again_from_the_profiles_template() {
    let f = task_fixture::fixture();
    let backend = resume_backend(&f);
    let tag = failed_worker(&f, &backend);

    let (result, out, _) = resume_41(&f, &backend);

    result.unwrap();
    assert!(out.contains("sandbox was gone"), "{out}");
    let last = backend
        .creates()
        .into_iter()
        .rfind(|c| c.name == WORKER)
        .unwrap();
    assert_eq!(last.template.as_deref(), Some(tag.as_str()));
}

#[test]
fn a_resumed_worker_whose_template_is_gone_warns_and_installs() {
    let f = task_fixture::fixture();
    let backend = resume_backend(&f);
    let tag = failed_worker(&f, &backend);
    sbxm::backend::SandboxBackend::template_rm(&backend, &tag).unwrap();

    let (result, _, warn) = resume_41(&f, &backend);

    result.unwrap();
    assert!(
        warn.lines().any(|l| l.starts_with("warning: ")
            && l.contains("has no template")
            && l.contains("install steps")),
        "{warn}"
    );
    let last = backend
        .creates()
        .into_iter()
        .rfind(|c| c.name == WORKER)
        .unwrap();
    assert_eq!(last.template, None);
}

/// Gap G1 (decisions 203, 216): a worker that changes `Cargo.lock` after it started is not
/// checked again inside its sandbox (a missing dependency fails there, clearly), but the
/// reviewer checks out the changed lock file, so a strict profile refuses it before its sandbox.
#[test]
fn a_worker_that_changes_the_lock_file_keeps_its_sandbox_and_a_strict_reviewer_is_refused() {
    let f = task_fixture::fixture();
    f.env.write_profile("default", &strict(PROFILE));
    let main = push_lock(&f, "main-lock", "version = 4\n", "refs/heads/main");
    let worker = task_fixture::play_tasks(
        &f.env.base_dir(),
        "main",
        task_fixture::Play {
            commits: vec!["Cargo.lock".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_exec_output_matching("claude", task_fixture::ok(task_fixture::CLAUDE_DONE))
        .with_exec_output_matching(
            "cargo metadata",
            task_fixture::ok(r#"{"packages":[{"name":"x"}],"version":1}"#),
        )
        .with_exec_hook(worker);
    let worker_tag = build_warm(&f.env, &backend, &main, Harness::Claude);
    build_warm(&f.env, &backend, &main, Harness::Codex);
    let (github, source) = (FakeGitHub::default(), task_fixture::source(&f));
    let ctx = task_fixture::ctx(&f, &source, &backend, &github);
    let mut prepared = pipeline::prepare(&ctx, &task_fixture::issue_text(41)).unwrap();

    let worked = pipeline::run_worker(&ctx, &mut prepared).unwrap();
    let review = pipeline::review_issue(&ctx.env(), &mut prepared);

    assert_eq!(worked.commits, 1);
    let workers: Vec<_> = backend
        .creates()
        .into_iter()
        .filter(|c| c.name == WORKER)
        .collect();
    assert_eq!(workers.len(), 1, "the worker's sandbox is never remade");
    assert_eq!(workers[0].template.as_deref(), Some(worker_tag.as_str()));
    let err = review.err().unwrap();
    assert!(format!("{err:#}").contains("--warm"), "{err:#}");
    assert!(reviewer_creates(&backend).is_empty());
}
