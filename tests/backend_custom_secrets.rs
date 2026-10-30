//! Issue 285: the fake records custom-secret calls in its ordered log and can script a failure
//! for each, like every other backend call.

use std::path::PathBuf;

use sbxm::backend::{CustomSecret, CustomSecretSpec, FakeBackend, SandboxBackend, SecretSource};

fn spec() -> CustomSecretSpec {
    CustomSecretSpec {
        sandbox: "sbxm-run-x-jev-0".into(),
        hosts: vec!["api.typesafe.ai".into()],
        env: "JEV_API_KEY".into(),
        placeholder: "sbxm-jev-x-0".into(),
        source: SecretSource::Command(PathBuf::from("/opt/jev-token")),
    }
}

#[test]
fn set_and_remove_are_logged_in_order_and_change_the_listing() {
    let fake = FakeBackend::default();
    fake.set_custom_secret(&spec()).unwrap();
    assert_eq!(fake.custom_secrets().unwrap().len(), 1);
    fake.remove_custom_secret("sbxm-jev-x-0", "sbxm-run-x-jev-0")
        .unwrap();
    assert_eq!(
        fake.log(),
        [
            "secret-set sbxm-run-x-jev-0 sbxm-jev-x-0",
            "secret-rm sbxm-run-x-jev-0 sbxm-jev-x-0"
        ]
    );
    assert!(fake.custom_secrets().unwrap().is_empty());
    assert_eq!(fake.custom_secret_sets(), [spec()]);
    // Removing one that is already gone is not an error.
    fake.remove_custom_secret("sbxm-jev-x-0", "sbxm-run-x-jev-0")
        .unwrap();
}

#[test]
fn each_call_can_be_scripted_to_fail() {
    let listed = vec![CustomSecret {
        scope: "global".into(),
        targets: vec!["api.typesafe.ai".into()],
        env: "JEV_API_KEY".into(),
        placeholder: "p".into(),
    }];
    let fake = FakeBackend::default().with_custom_secrets(listed.clone());
    assert_eq!(fake.custom_secrets().unwrap(), listed);
    assert!(
        FakeBackend::default()
            .failing_custom_secrets()
            .custom_secrets()
            .is_err()
    );
    let fake = FakeBackend::default().failing_set_custom_secret();
    assert!(fake.set_custom_secret(&spec()).is_err());
    assert_eq!(fake.log(), ["secret-set sbxm-run-x-jev-0 sbxm-jev-x-0"]);
    let fake = FakeBackend::default().failing_remove_custom_secret();
    assert!(fake.remove_custom_secret("p", "s").is_err());
    assert_eq!(fake.log(), ["secret-rm s p"]);
}
