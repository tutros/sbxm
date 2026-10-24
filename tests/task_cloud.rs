//! Milestone 4 slice 1 (`sdlc/milestone-4.md`, decisions 252-267): `task start --spec FILE
//! --cloud`, non-strict, through `FakeBackend`.

mod common;

use common::task_fixture::fixture_with;
use sbxm::backend::Location;
use sbxm::task::pipeline::choose_location;

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const CLOUD_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"cloud\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";
const NO_LOCATION_TOML: &str =
    "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

/// P14 (decision 262(k)): the flag, else the config.
#[test]
fn the_flag_wins_over_the_config_and_the_config_decides_without_one() {
    let local = fixture_with(LOCAL_TOML).config;
    let cloud = fixture_with(CLOUD_TOML).config;
    let absent = fixture_with(NO_LOCATION_TOML).config;

    assert_eq!(
        choose_location(Some(Location::Cloud), &local),
        Location::Cloud
    );
    assert_eq!(
        choose_location(Some(Location::Local), &cloud),
        Location::Local
    );
    assert_eq!(choose_location(None, &cloud), Location::Cloud);
    assert_eq!(choose_location(None, &local), Location::Local);
    assert_eq!(choose_location(None, &absent), Location::Local);
}

fn identity() -> sbxm::task::repo::Identity {
    sbxm::task::repo::Identity {
        name: "Dev".into(),
        email: "dev@example.com".into(),
    }
}

fn assert_refused_before_anything(
    result: anyhow::Result<()>,
    f: &common::task_fixture::Fixture,
    backend: &sbxm::backend::FakeBackend,
    github: &sbxm::github::fake::FakeGitHub,
    command: &str,
) {
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains(command) && message.contains("cloud"),
        "{message}"
    );
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(github.calls().is_empty(), "{:?}", github.calls());
    assert!(!f.env.base_dir().join(".sbxm").join("tasks").exists());
}

/// Decision 261: a command whose sandboxes can't run in the cloud yet refuses a cloud location
/// before anything, instead of running them here.
#[test]
fn an_issue_start_in_a_cloud_repo_is_refused_before_anything_and_local_overrides_it() {
    use sbxm::commands::task_start::{self, Options};

    let f = fixture_with(CLOUD_TOML);
    let options = |location| Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        issues: vec![41],
        workers: None,
        worker_harness: None,
        worker_model: None,
        time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(common::task_fixture::source(&f)),
        identity: Some(identity()),
        disk: Default::default(),
        location,
    };
    let run = |opts: &Options,
               backend: &sbxm::backend::FakeBackend,
               github: &sbxm::github::fake::FakeGitHub| {
        task_start::run_with(
            &f.env.config_dir(),
            opts,
            None,
            backend,
            github,
            &common::task_fixture::Probe,
            &mut Vec::new(),
            &mut Vec::new(),
        )
    };

    for location in [None, Some(Location::Cloud)] {
        let backend = common::task_fixture::backend();
        let github = sbxm::github::fake::FakeGitHub::default();
        let result = run(&options(location), &backend, &github);
        assert_refused_before_anything(result, &f, &backend, &github, "task start --issue");
    }

    // `--local` gets past it: the start goes on to ask GitHub for the issue.
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let _ = run(&options(Some(Location::Local)), &backend, &github);
    assert!(!github.calls().is_empty());
}

#[test]
fn task_run_in_a_cloud_repo_is_refused_before_anything() {
    use sbxm::commands::task_run::{self, SpecOptions};

    let f = fixture_with(CLOUD_TOML);
    let spec = common::task_fixture::spec_file(&f, "Build a thing.\n");
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let result = task_run::run_spec(
        &f.env.config_dir(),
        &SpecOptions {
            repo_root: f.env.tmp.path().join("target-repo"),
            spec,
            worker_harness: None,
            worker_model: None,
            reviewer_harness: None,
            reviewer_model: None,
            time_limit: None,
            reviewer_time_limit: None,
            profile: None,
            base: Some("main".into()),
            repo: Some("o/r".into()),
            clone_source: Some(common::task_fixture::source(&f)),
            identity: Some(identity()),
            disk: Default::default(),
        },
        None,
        &backend,
        &github,
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    assert_refused_before_anything(result, &f, &backend, &github, "task run");
}

#[test]
fn a_pr_review_in_a_cloud_repo_is_refused_before_anything() {
    use sbxm::commands::task_review::{self, Options, Target};

    let f = fixture_with(CLOUD_TOML);
    let backend = common::task_fixture::backend();
    let github = sbxm::github::fake::FakeGitHub::default();
    let result = task_review::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: Target::Pr(7),
            repo: Some("o/r".into()),
            base: Some("main".into()),
            clone_source: Some(common::task_fixture::source(&f)),
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
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut Vec::new(),
        &mut Vec::new(),
    );

    assert_refused_before_anything(result, &f, &backend, &github, "task review --pr");
}
