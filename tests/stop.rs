mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::{new, stop};
use sbxm::harness::Harness;

#[test]
fn stops_the_projects_sandbox() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::default();

    stop::run(&env.config_dir(), "demo", Harness::Claude, &backend).unwrap();

    assert_eq!(backend.stops(), ["sbxm-demo-claude"]);
}

#[test]
fn unknown_project_says_how_to_create_it() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = stop::run(&env.config_dir(), "demo", Harness::Claude, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("no sbxm sandbox for project 'demo'; create one with `sbxm new demo`"),
        "{message}"
    );
    assert!(backend.stops().is_empty());
}

#[test]
fn invalid_name_makes_no_backend_calls() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = stop::run(&env.config_dir(), "Demo", Harness::Claude, &backend).unwrap_err();

    assert!(format!("{err:#}").contains("invalid project name"));
    assert!(backend.stops().is_empty());
}

fn codex() -> new::Options {
    new::Options {
        harness: Harness::Codex,
        ..Default::default()
    }
}

#[test]
fn stops_the_selected_harness_only() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    env.run_with("demo", &codex(), &FakeBackend::default())
        .unwrap();
    let backend = FakeBackend::default();

    stop::run(&env.config_dir(), "demo", Harness::Codex, &backend).unwrap();

    assert_eq!(backend.stops(), ["sbxm-demo-codex"]);
}

#[test]
fn missing_harness_says_how_to_create_it() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::default();

    let err = stop::run(&env.config_dir(), "demo", Harness::Codex, &backend).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains(
            "no sbxm codex sandbox for project 'demo'; create one with \
             `sbxm new demo --harness codex`"
        ),
        "{message}"
    );
    assert!(backend.stops().is_empty());
}
