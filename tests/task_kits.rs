//! M2b slice 5: kits can be built from a profile and a harness list, not only from a
//! run-config (spec §1), so `sbxm task` can build the worker's and reviewer's kits.

mod common;

use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::harness::Harness;
use sbxm::run::kits::{self, Overrides};

fn root(env: &Env) -> std::path::PathBuf {
    env.base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-1")
        .join("kits")
}

#[test]
fn one_kit_set_per_harness_under_the_named_profile() {
    let env = Env::new();
    env.write_profile("task-profile", "description = \"for tasks\"\n");

    let built = kits::build_for(
        &env.config_dir(),
        "task-profile",
        &[Harness::Claude, Harness::Codex],
        &Overrides::default(),
        &root(&env),
        &FakeBackend::default(),
    )
    .unwrap();

    assert_eq!(built.profile_name, "task-profile");
    let harnesses: Vec<_> = built.harnesses.iter().map(|h| h.harness).collect();
    assert_eq!(harnesses, [Harness::Claude, Harness::Codex]);
    assert!(built.harnesses.iter().all(|h| h.profile == "task-profile"));
    let claude = built.get(Harness::Claude).unwrap();
    assert!(
        claude
            .dirs
            .iter()
            .all(|d| d.starts_with(root(&env)) && d.exists())
    );
}

#[test]
fn a_repeated_harness_is_built_once() {
    let env = Env::new();

    let built = kits::build_for(
        &env.config_dir(),
        "default",
        &[Harness::Claude, Harness::Claude],
        &Overrides::default(),
        &root(&env),
        &FakeBackend::default(),
    )
    .unwrap();

    assert_eq!(built.harnesses.len(), 1);
}

#[test]
fn cpus_and_memory_overrides_change_the_hash_and_the_resources() {
    let env = Env::new();
    let build = |cpus, memory: Option<&str>| {
        kits::build_for(
            &env.config_dir(),
            "default",
            &[Harness::Claude],
            &Overrides {
                cpus,
                memory: memory.map(str::to_owned),
            },
            &root(&env),
            &FakeBackend::default(),
        )
        .unwrap()
    };

    let plain = build(None, None);
    let bigger = build(Some(8), Some("16g"));

    assert_ne!(
        plain.get(Harness::Claude).unwrap().config_hash,
        bigger.get(Harness::Claude).unwrap().config_hash
    );
    assert_eq!(bigger.resources.cpus, 8);
    assert_eq!(bigger.resources.memory, "16g");
}

#[test]
fn an_unknown_profile_writes_nothing() {
    let env = Env::new();

    let error = kits::build_for(
        &env.config_dir(),
        "no-such-profile",
        &[Harness::Claude],
        &Overrides::default(),
        &root(&env),
        &FakeBackend::default(),
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("no-such-profile"),
        "{error:#}"
    );
    assert!(!root(&env).exists());
}

#[test]
fn an_invalid_kit_is_refused_before_any_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad mixin");

    let error = kits::build_for(
        &env.config_dir(),
        "default",
        &[Harness::Claude],
        &Overrides::default(),
        &root(&env),
        &backend,
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("bad mixin"), "{error:#}");
}
