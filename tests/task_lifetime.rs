//! Milestone 4 slice 3 (`sdlc/milestone-4.md` P5 (d), P6, P7; decisions 259, 262, 263, 265, 267):
//! cloud sandbox lifetimes. The TTLs, the `--ttl` text, the removal retry and the `sandboxes`
//! list in `task.json`.

use std::time::Duration;

use sbxm::backend::ttl_arg;
use sbxm::task::config::TaskConfig;
use sbxm::task::lifetime::{reviewer_ttl, worker_ttl};

fn config(toml: &str) -> TaskConfig {
    let dir = tempfile::TempDir::new().unwrap();
    let toml = format!("[sandbox]\nprofile = \"default\"\n{toml}");
    std::fs::write(dir.path().join("sbxm-task.toml"), toml).unwrap();
    TaskConfig::load(dir.path()).unwrap()
}

const MIN: u64 = 60;

/// W + G + F x (R + W + G) + 30m, with G = the gate timeout x the number of sandbox gates.
#[test]
fn the_worker_ttl_adds_the_gates_and_every_fix_round() {
    let c = config(
        "[worker]\ntime_limit = \"1h\"\nfix_rounds = 2\n[reviewer]\ntime_limit = \"10m\"\n\
         [gates]\nsandbox = [\"a\", \"b\"]\ntimeout = \"5m\"\n",
    );
    // 60 + 10 + 2 x (10 + 60 + 10) + 30 = 260 minutes.
    assert_eq!(worker_ttl(&c), Duration::from_secs(260 * MIN));
}

#[test]
fn with_no_fix_rounds_the_worker_ttl_is_the_worker_and_its_gates() {
    let c = config(
        "[worker]\ntime_limit = \"1h\"\nfix_rounds = 0\n[reviewer]\ntime_limit = \"10m\"\n\
         [gates]\nsandbox = [\"a\"]\ntimeout = \"5m\"\n",
    );
    assert_eq!(worker_ttl(&c), Duration::from_secs((60 + 5 + 30) * MIN));
}

#[test]
fn with_no_sandbox_gates_the_gates_count_nothing() {
    let c = config(
        "[worker]\ntime_limit = \"20m\"\nfix_rounds = 1\n[reviewer]\ntime_limit = \"10m\"\n\
         [gates]\nsandbox = []\nhost = [\"h\"]\ntimeout = \"5m\"\n",
    );
    // 20 + 0 + 1 x (10 + 20 + 0) + 30 = 80 minutes; a host gate doesn't run in the sandbox.
    assert_eq!(worker_ttl(&c), Duration::from_secs(80 * MIN));
}

/// The large case: W = 2h, three gates of 20m, F = 3, R = 45m.
#[test]
fn a_large_worker_ttl() {
    let c = config(
        "[worker]\ntime_limit = \"2h\"\nfix_rounds = 3\n[reviewer]\ntime_limit = \"45m\"\n\
         [gates]\nsandbox = [\"a\", \"b\", \"c\"]\ntimeout = \"20m\"\n",
    );
    // 120 + 60 + 3 x (45 + 120 + 60) + 30 = 885 minutes = 14h45m.
    assert_eq!(worker_ttl(&c), Duration::from_secs(885 * MIN));
    assert_eq!(ttl_arg(worker_ttl(&c)), "14h45m");
}

#[test]
fn the_reviewer_ttl_is_its_time_limit_and_half_an_hour() {
    let c = config("[reviewer]\ntime_limit = \"45m\"\n[gates]\nsandbox = []\n");
    assert_eq!(reviewer_ttl(&c), Duration::from_secs(75 * MIN));
    assert_eq!(ttl_arg(reviewer_ttl(&c)), "1h15m");
}

/// Time limits that aren't whole minutes give a TTL rounded up to the next minute.
#[test]
fn a_ttl_that_is_not_whole_minutes_is_rounded_up() {
    let c = config("[worker]\ntime_limit = \"90s\"\nfix_rounds = 0\n[gates]\nsandbox = []\n");
    // 1m30s + 30m = 31m30s, so 32m.
    assert_eq!(worker_ttl(&c), Duration::from_secs(32 * MIN));
    assert_eq!(ttl_arg(worker_ttl(&c)), "32m");
}

#[test]
fn the_ttl_text_takes_the_forms_sbx_names() {
    assert_eq!(ttl_arg(Duration::from_secs(45 * MIN)), "45m");
    assert_eq!(ttl_arg(Duration::from_secs(59 * MIN)), "59m");
    // Exactly an hour is `1h`, not `60m`.
    assert_eq!(ttl_arg(Duration::from_secs(60 * MIN)), "1h");
    assert_eq!(ttl_arg(Duration::from_secs(61 * MIN)), "1h1m");
    assert_eq!(ttl_arg(Duration::from_secs(2 * 60 * MIN)), "2h");
    assert_eq!(ttl_arg(Duration::from_secs(195 * MIN)), "3h15m");
    // Part of a minute counts as a whole one.
    assert_eq!(ttl_arg(Duration::from_secs(59 * MIN + 1)), "1h");
    assert_eq!(ttl_arg(Duration::from_millis(1)), "1m");
}

// The removal retry (P6 (a), decisions 262, 267): only "temporarily unavailable" is retried, up to
// 3 times after the first attempt, after 5, 10 and 20 seconds.

use std::sync::Mutex;

use sbxm::backend::FakeBackend;
use sbxm::task::lifetime::{Removal, remove_sandbox};

const SANDBOX: &str = "sbxm-task-x-claude";

/// S14 Q5's wording of the one error worth retrying, as `SbxBackend::remove` reports a failure.
fn unavailable() -> String {
    format!(
        "`sbx rm` failed for sandbox {SANDBOX} (exit status: 1): error: sandbox {SANDBOX} is \
         temporarily unavailable; retry the delete"
    )
}

/// Removes [`SANDBOX`] through `backend`; returns the outcome and the waits it asked for.
fn remove(backend: &FakeBackend, name: &str) -> (Removal, Vec<Duration>) {
    let waits = Mutex::new(Vec::new());
    let removal = remove_sandbox(backend, name, &|d| waits.lock().unwrap().push(d));
    (removal, waits.into_inner().unwrap())
}

fn secs(list: &[u64]) -> Vec<Duration> {
    list.iter().map(|s| Duration::from_secs(*s)).collect()
}

#[test]
fn a_removal_that_succeeds_at_once_waits_for_nothing() {
    let backend = FakeBackend::default();

    let (removal, waits) = remove(&backend, SANDBOX);

    assert_eq!(removal, Removal::Removed);
    assert_eq!(backend.removes(), [SANDBOX]);
    assert!(waits.is_empty());
}

#[test]
fn a_removal_that_succeeds_on_the_third_retry_waits_5_10_and_20_seconds() {
    let error = unavailable();
    let backend = FakeBackend::default().with_remove_errors(&[&error, &error, &error]);

    let (removal, waits) = remove(&backend, SANDBOX);

    assert_eq!(removal, Removal::Removed);
    assert_eq!(backend.removes().len(), 4);
    assert_eq!(waits, secs(&[5, 10, 20]));
}

#[test]
fn a_removal_that_still_fails_after_three_retries_stops_at_four_calls_and_is_reported() {
    let error = unavailable();
    let backend = FakeBackend::default().with_remove_errors(&[&error, &error, &error, &error]);

    let (removal, waits) = remove(&backend, SANDBOX);

    match removal {
        Removal::Failed(why) => assert!(why.contains("temporarily unavailable"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(backend.removes().len(), 4);
    assert_eq!(waits, secs(&[5, 10, 20]));
}

#[test]
fn any_other_error_is_reported_after_one_call() {
    let backend =
        FakeBackend::default().with_remove_errors(&["`sbx rm` failed: permission denied"]);

    let (removal, waits) = remove(&backend, SANDBOX);

    match removal {
        Removal::Failed(why) => assert!(why.contains("permission denied"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(backend.removes().len(), 1);
    assert!(waits.is_empty());
}

/// `sbx-cloud-rm-not-found.txt`: a sandbox already gone counts as removed.
#[test]
fn a_sandbox_that_is_not_found_counts_as_removed() {
    let fixture = include_str!("../src/backend/fixtures/sbx-cloud-rm-not-found.txt");
    let error = format!("`sbx rm` failed for sandbox sbxm-p2-capture (exit status: 1): {fixture}");
    let backend = FakeBackend::default().with_remove_errors(&[&error]);

    let (removal, waits) = remove(&backend, "sbxm-p2-capture");

    assert_eq!(removal, Removal::AlreadyGone);
    assert_eq!(backend.removes().len(), 1);
    assert!(waits.is_empty());

    // Another sandbox's "not found" isn't this one's.
    let backend = FakeBackend::default().with_remove_errors(&[&error]);
    let (removal, _) = remove(&backend, SANDBOX);
    assert!(matches!(removal, Removal::Failed(_)), "{removal:?}");
}
