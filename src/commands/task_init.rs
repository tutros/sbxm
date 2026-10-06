use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::task::config::FILE_NAME;

/// Valid as written for a Rust repo (the gates default applies, the reviewer
/// differs from the worker). Models are ones verified on real `sbx`; the
/// profile must exist in your profiles dir (spec §2).
const STARTER: &str = r##"# How `sbxm task` works on this repo: a worker agent takes a GitHub issue, gates check
# its change, an independent reviewer reads it, and a fix round follows for as long as the
# round budget below allows (a gate failure spends a round too).
# See what would run with: sbxm task gates --issue N --dry-run

[worker]
harness = "claude"             # claude | codex | antigravity
# model = "claude-opus-5-5"    # the harness's default when absent
time_limit = "2h"
# fix_rounds = 3                # the round budget; 0 stops at the first must-fix finding or gate failure

# The reviewer defaults to a harness different from the worker's.
[reviewer]
# harness = "codex"
# model = "gpt-5.6-sol"
time_limit = "45m"

# Each harness needs its provider's secret stored in sbx (see `sbx secret ls`):
# claude -> anthropic, codex -> openai, antigravity -> google.

[sandbox]
# A profile in your profiles dir (`sbxm config profiles-dir`); required.
profile = "sbxm-dev"
# cpus = 4
# memory = "8g"

# Without [gates] sandbox, a repo with a Cargo.toml gets the three cargo commands below;
# any other repo must list its own. An empty list means no sandbox gates.
[gates]
# sandbox = ["cargo fmt --check", "cargo clippy --all-targets -- -D warnings", "cargo test"]
# Optional second tier, run on this machine after the sandbox tier passes. Off while empty.
# WARNING: host gates run agent-written code on this machine, outside any sandbox. List
# only commands you would run on a stranger's pull request.
# host = []
timeout = "20m"                # per command

# Replace a built-in prompt with a file inside this repo:
# [prompts]
# worker = "prompts/worker.md"
# reviewer = "prompts/reviewer.md"
# fix = "prompts/fix.md"
"##;

/// Writes the starter into `dir` (default: the working directory) and returns
/// the file's path.
pub fn run(dir: Option<&Path>) -> Result<PathBuf> {
    let path = dir.unwrap_or_else(|| Path::new(".")).join(FILE_NAME);
    if path.exists() {
        bail!(
            "{} already exists; not overwriting it, edit it or delete it first",
            path.display()
        );
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(&path, STARTER).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}
