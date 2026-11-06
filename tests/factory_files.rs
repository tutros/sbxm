//! The committed files the scheduled factory check uses (issue 327, decision 269): the
//! `factory` profile in `profiles/` and the `sbxm-task.toml` that `scripts/factory-check.ps1`
//! puts into its dedicated checkout. The script itself is tested with Pester
//! (`scripts/tests/FactoryCheck.Tests.ps1`).

use std::path::Path;

use sbxm::config::Profile;
use sbxm::harness::Harness;
use sbxm::task::config::{Sink, TaskConfig};

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn factory_profile_allows_only_the_model_provider_and_installs_nothing() {
    let profile = Profile::load(&repo().join("profiles"), "factory").unwrap();

    // Nothing beyond the harness's own hosts: no crates.io, no GitHub (decision 4).
    assert!(
        profile.network.allow.is_empty(),
        "{:?}",
        profile.network.allow
    );
    assert!(profile.setup.install.is_empty());
    assert_eq!(profile.secrets.services, ["anthropic"]);
}

#[test]
fn factory_checkout_task_config_has_no_gates_and_claude_with_its_default_model() {
    // A repo with a Cargo.toml gets the default cargo gates unless `[gates] sandbox` is set.
    let checkout = tempfile::tempdir().unwrap();
    std::fs::write(
        checkout.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n",
    )
    .unwrap();
    std::fs::copy(
        repo().join("scripts/factory/sbxm-task.toml"),
        checkout.path().join("sbxm-task.toml"),
    )
    .unwrap();

    let config = TaskConfig::load(checkout.path()).unwrap();

    assert!(
        config.gates.sandbox.is_empty(),
        "{:?}",
        config.gates.sandbox
    );
    assert!(config.gates.host.is_empty());
    assert_eq!(config.worker.harness, Harness::Claude);
    assert_eq!(config.worker.model, None);
    assert_eq!(config.sandbox.profile, "factory");
    // The draft is read out of repo.git; nothing is pushed or opened.
    assert_eq!(config.sink, Sink::Local);
}
