//! Milestone 3 slice 8: `sbxm task`'s sandboxes follow the profile's template (decisions 200, 203,
//! 206, 216).

mod common;

use std::fs;

use common::{Env, git, task_fixture};
use sbxm::backend::FakeBackend;
use sbxm::commands::template_build;
use sbxm::github::fake::FakeGitHub;
use sbxm::harness::Harness;
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
