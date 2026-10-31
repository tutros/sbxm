//! Opens the `run.log` writer that the `sbxm task` commands in `main` tee their output into
//! (decision 170, part 1). The log logic itself is `task::runlog`; this adds what only a running
//! command knows: the command line, the executable and its hash, the clock and the base folder.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::GlobalConfig;
use crate::run::results::now;
use crate::task::record::Process;
use crate::task::runlog::{self, RunLog};

/// A log for a command working on the tasks `ids` (`issue-5`, `pr-7`). With `identity`, it also
/// logs into every task whose record carries that exact process (pid and start time; decision
/// 163): `task start` picks its own issues itself, so it discovers its tasks instead of naming them.
pub fn open(
    config_dir: &Path,
    ids: Vec<String>,
    identity: Option<Process>,
) -> Result<Arc<Mutex<RunLog>>> {
    let base = GlobalConfig::load(config_dir)?.base_dir;
    let header = header()?;
    Ok(Arc::new(Mutex::new(RunLog::new(
        &base,
        header,
        ids,
        identity,
        Box::new(now),
        Box::new(runlog::recorded_stage),
        Box::new(|msg| eprintln!("{msg}")),
    ))))
}

/// This invocation's header line (decision 170 (c)): now, the command line, the executable and its
/// SHA-256, the commit if the build knows it, and the process id. `sbxm run`'s `run.log` uses it
/// too (part 3). The executable is hashed once per process: it doesn't change while it runs.
pub fn header() -> Result<String> {
    static SHA256: OnceLock<String> = OnceLock::new();
    let exe = std::env::current_exe().context("cannot find this executable")?;
    let sha256 = match SHA256.get() {
        Some(sha256) => sha256,
        None => {
            let bytes =
                std::fs::read(&exe).with_context(|| format!("cannot read {}", exe.display()))?;
            SHA256.get_or_init(|| {
                Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            })
        }
    };
    let args: Vec<String> = std::env::args().collect();
    Ok(runlog::header(
        now(),
        &args,
        &exe,
        sha256,
        option_env!("SBXM_COMMIT"),
        std::process::id(),
    ))
}

/// A log that writes nowhere: for a command with no task folder to log into, and for a command
/// whose log could not be opened (a missing config, say), which then reports that itself.
pub fn none() -> Arc<Mutex<RunLog>> {
    Arc::new(Mutex::new(RunLog::new(
        Path::new("."),
        String::new(),
        Vec::new(),
        None,
        Box::new(now),
        Box::new(runlog::recorded_stage),
        Box::new(|_| {}),
    )))
}
