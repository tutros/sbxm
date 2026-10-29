//! Milestone 4 slice 4 (decision 256): the fake records the cloud's scoped-secret calls in its
//! ordered log, never the value, and can script a failure for each, like every other backend call.

use sbxm::backend::{FakeBackend, SandboxBackend, ScopedSecret};

fn fake() -> ScopedSecret {
    ScopedSecret {
        host: "postman-echo.com".into(),
        env: "P2_FAKE".into(),
        value: "fake-value-NOT-REAL".into(),
    }
}

#[test]
fn set_list_and_remove_are_logged_in_order_without_the_value() {
    let backend = FakeBackend::default().with_cloud_secret("sbxm-other", "postman-echo-com");
    backend
        .set_scoped_secret("sbxm-task-x-claude", &fake())
        .unwrap();
    assert_eq!(
        backend.scoped_secrets("sbxm-task-x-claude").unwrap(),
        ["postman-echo-com"]
    );
    backend
        .remove_secret("postman-echo-com", "sbxm-task-x-claude")
        .unwrap();
    assert_eq!(
        backend.scoped_secrets("sbxm-task-x-claude").unwrap(),
        Vec::<String>::new()
    );
    // Another sandbox's secret of the same name is left alone.
    assert_eq!(
        backend.cloud_secrets(),
        [("sbxm-other".to_owned(), "postman-echo-com".to_owned())]
    );
    assert_eq!(
        backend.log(),
        [
            "secret-set sbxm-task-x-claude postman-echo-com",
            "secret-ls sbxm-task-x-claude",
            "secret-rm sbxm-task-x-claude postman-echo-com",
            "secret-ls sbxm-task-x-claude",
        ]
    );
    assert!(!backend.log().join("\n").contains("NOT-REAL"));
    assert_eq!(
        backend.scoped_secret_sets(),
        [("sbxm-task-x-claude".to_owned(), fake())]
    );
    // Removing one that is already gone is not an error.
    backend
        .remove_secret("postman-echo-com", "sbxm-task-x-claude")
        .unwrap();
}

#[test]
fn each_call_can_be_scripted_to_fail() {
    let backend = FakeBackend::default().failing_set_scoped_secret_for("postman-echo.com");
    let error = backend
        .set_scoped_secret("sbxm-task-x-claude", &fake())
        .unwrap_err();
    assert!(!format!("{error:#}").contains("NOT-REAL"));
    assert_eq!(
        backend.log(),
        ["secret-set sbxm-task-x-claude postman-echo-com"]
    );
    // A failed set stored nothing.
    assert_eq!(
        backend.scoped_secrets("sbxm-task-x-claude").unwrap(),
        Vec::<String>::new()
    );

    assert!(
        FakeBackend::default()
            .failing_scoped_secrets()
            .scoped_secrets("sbxm-task-x-claude")
            .is_err()
    );

    let backend = FakeBackend::default()
        .with_cloud_secret("s", "a")
        .with_cloud_secret("s", "b")
        .failing_remove_secret_for("a");
    assert!(backend.remove_secret("a", "s").is_err());
    backend.remove_secret("b", "s").unwrap();
    assert_eq!(backend.log(), ["secret-rm s a", "secret-rm s b"]);
    assert_eq!(backend.cloud_secrets(), [("s".to_owned(), "a".to_owned())]);
}
