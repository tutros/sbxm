//! How long a cloud sandbox of a task lives (milestone 4 slice 3; P5 (d), P6, P7; decisions 259,
//! 262, 263, 265): the TTL each role's sandbox is created with.

use std::time::Duration;

use super::config::TaskConfig;
use crate::backend::whole_minutes;

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
