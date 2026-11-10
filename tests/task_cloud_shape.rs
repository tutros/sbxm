//! Issue 559: a cloud task's sandboxes get a billable shape. `[sandbox] cloud_shape` in
//! `sbxm-task.toml` names one of five (a fixed table, matched case-insensitively, refused at load
//! otherwise); unset, the local cpus/memory are used when they are exactly a shape, and a cloud
//! task is refused before anything is created when they aren't. Local sandboxes keep
//! `[resources]` and `[sandbox] cpus / memory`. Through `FakeBackend`.

mod common;

use common::task_fixture::fixture_with;
use sbxm::task::config::TaskConfig;
use sbxm::task::shape::Shape;

fn toml_with_shape(shape: &str) -> String {
    format!(
        "[sandbox]\nprofile = \"default\"\ncloud_shape = {shape:?}\n\n[gates]\nsandbox = [\"cargo test\"]\n"
    )
}

fn load(toml: &str) -> anyhow::Result<TaskConfig> {
    let f = fixture_with("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n");
    let repo = f.env.tmp.path().join("target-repo");
    std::fs::write(repo.join("sbxm-task.toml"), toml).unwrap();
    TaskConfig::load(&repo)
}

// The key at load.

#[test]
fn each_of_the_five_names_loads_as_its_shape_with_the_table_s_cpus_and_memory() {
    let table = [
        ("micro", Shape::Micro, 1, "2g"),
        ("small", Shape::Small, 2, "4g"),
        ("medium", Shape::Medium, 4, "8g"),
        ("large", Shape::Large, 8, "16g"),
        ("xl", Shape::Xl, 16, "32g"),
    ];
    for (name, shape, cpus, memory) in table {
        let config = load(&toml_with_shape(name)).unwrap();
        assert_eq!(config.sandbox.cloud_shape, Some(shape), "{name}");
        assert_eq!(shape.cpus(), cpus, "{name}");
        assert_eq!(shape.memory(), memory, "{name}");
    }
}

#[test]
fn a_name_is_matched_case_insensitively() {
    let config = load(&toml_with_shape("Large")).unwrap();
    assert_eq!(config.sandbox.cloud_shape, Some(Shape::Large));
    let config = load(&toml_with_shape("XL")).unwrap();
    assert_eq!(config.sandbox.cloud_shape, Some(Shape::Xl));
}

#[test]
fn an_unknown_or_empty_name_is_refused_at_load_naming_the_five() {
    for name in ["huge", ""] {
        let message = format!("{:#}", load(&toml_with_shape(name)).unwrap_err());
        assert!(message.contains("sandbox.cloud_shape"), "{message}");
        assert!(
            message.contains("micro, small, medium, large or xl"),
            "{message}"
        );
    }
}

#[test]
fn the_key_absent_loads_as_none() {
    let config = load("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n").unwrap();
    assert_eq!(config.sandbox.cloud_shape, None);
}
