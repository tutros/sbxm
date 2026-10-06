//! Opens the `run.log` writer that the `sbxm task` commands in `main` tee their output into
//! (decision 170, part 1). The log logic itself is `task::runlog`; this adds what only a running
//! command knows: the command line, the executable and its hash, the clock and the base folder.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::GlobalConfig;
use crate::run::results::now;
use crate::task::runlog::{self, RunLog};

/// A log for a command working on the tasks `ids` (`issue-5`, `pr-7`). With `discover`, it also
/// logs into every task this process records (`task start` picks its issues itself).
pub fn open(config_dir: &Path, ids: Vec<String>, discover: bool) -> Result<Arc<Mutex<RunLog>>> {
    let base = GlobalConfig::load(config_dir)?.base_dir;
    let exe = std::env::current_exe().context("cannot find this executable")?;
    let bytes = std::fs::read(&exe).with_context(|| format!("cannot read {}", exe.display()))?;
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let pid = std::process::id();
    let args: Vec<String> = std::env::args().collect();
    let header = runlog::header(now(), &args, &exe, &sha256, option_env!("SBXM_COMMIT"), pid);
    Ok(Arc::new(Mutex::new(RunLog::new(
        &base,
        header,
        ids,
        discover.then_some(pid),
        Box::new(now),
        Box::new(runlog::recorded_stage),
    ))))
}
