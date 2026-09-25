mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::stop;

#[test]
fn stops_the_projects_sandbox() {
    let env = Env::new();
    env.run("demo", &FakeBackend::default()).unwrap();
    let backend = FakeBackend::default();

    stop::run(&env.config_dir(), "demo", &backend).unwrap();

    assert_eq!(backend.stops(), ["sbxm-demo-claude"]);
}

#[test]
fn unknown_project_says_how_to_create_it() {
    let env = Env::new();
    let backend = FakeBackend::default();

    let err = stop::run(&env.config_dir(), "demo", &backend).unwrap_err();

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

    let err = stop::run(&env.config_dir(), "Demo", &backend).unwrap_err();

    assert!(format!("{err:#}").contains("invalid project name"));
    assert!(backend.stops().is_empty());
}
