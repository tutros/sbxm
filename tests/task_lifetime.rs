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
