//! How long a cloud sandbox of a task lives (milestone 4 slice 3; P5 (d), P6, P7; decisions 259,
//! 262, 263, 265): the TTL each role's sandbox is created with.

use std::time::Duration;

use super::config::TaskConfig;
use crate::backend::{SandboxBackend, whole_minutes};

/// What every TTL adds to the time its sandbox is expected to need (decision 259).
const MARGIN: Duration = Duration::from_secs(30 * 60);

/// The worker sandbox lives through the task's automatic run (P5 (d), decision 263): the worker,
/// its sandbox gates, and `fix_rounds` times a review, a fix round and the gates again, plus the
/// margin, rounded up to whole minutes as `sbx` takes it.
pub fn worker_ttl(config: &TaskConfig) -> Duration {
    let worker = config.worker.time_limit;
    let gates = sandbox_gates(config);
    let round = config.reviewer.time_limit + worker + gates;
    rounded(worker + gates + round * config.fix_rounds + MARGIN)
}

/// A reviewer sandbox lives for one review round (P5 (d)): the reviewer's time limit plus the
/// margin, rounded up to whole minutes.
pub fn reviewer_ttl(config: &TaskConfig) -> Duration {
    rounded(config.reviewer.time_limit + MARGIN)
}

/// Every sandbox gate's timeout: `[gates] timeout` once per `[gates] sandbox` command.
fn sandbox_gates(config: &TaskConfig) -> Duration {
    config.gates.timeout * u32::try_from(config.gates.sandbox.len()).unwrap_or(u32::MAX)
}

fn rounded(ttl: Duration) -> Duration {
    Duration::from_secs(whole_minutes(ttl) * 60)
}

/// The waits before each retry of a removal that `sbx` says to retry (P6 (a), decision 262): up
/// to 3 retries after the first attempt.
const REMOVAL_WAITS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];

/// The one removal error worth retrying (S14 Q5's wording, decision 267).
const RETRYABLE: &str = "temporarily unavailable";

/// How a cloud sandbox's removal ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// `sbx` didn't know the sandbox (`sbx-cloud-rm-not-found.txt`): it counts as removed.
    AlreadyGone,
    /// The last error, after any retries: the sandbox may still be there.
    Failed(String),
}

impl Removal {
    /// Whether the sandbox is gone now.
    pub fn gone(&self) -> bool {
        !matches!(self, Removal::Failed(_))
    }
}

/// Removes the sandbox `name` (`sbx rm <name> -f` on the backend's instance), retrying only an
/// error that says it is temporarily unavailable, after 5, 10 and 20 seconds through `sleep`;
/// any other error ends it at once. Never fails: the caller reports what it returns.
pub fn remove_sandbox(
    backend: &dyn SandboxBackend,
    name: &str,
    sleep: &dyn Fn(Duration),
) -> Removal {
    let mut waits = REMOVAL_WAITS.iter();
    loop {
        let error = match backend.remove(name) {
            Ok(()) => return Removal::Removed,
            Err(e) => format!("{e:#}"),
        };
        if error.contains(&format!("sandbox '{name}' not found")) {
            return Removal::AlreadyGone;
        }
        match waits.next() {
            Some(wait) if error.contains(RETRYABLE) => sleep(*wait),
            _ => return Removal::Failed(error),
        }
    }
}
