use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Where `sbxm run init` writes when no path is given.
const DEFAULT_PATH: &str = "run.toml";

/// Valid as written: two contestants are live, the third (Antigravity) is a
/// commented block to switch on. Everything under `[eval]` is optional
/// (milestone-2.md, P1). Models are ones verified on real `sbx`; `agy models`
/// lists Antigravity's.
const STARTER: &str = r##"# A comparison: the same task, run headless and in parallel by 2 to 4 contestants.
# Run it with: sbxm run <this file>

[task]
prompt = "Describe the task here, e.g. \"Implement feature X per spec.md\"."
# Optional: a folder copied into each contestant's fresh workspace (with a new git
# repo and one baseline commit). Omit it for an empty workspace.
# seed = "./seed-dir"

[run]
# Profile applied to every contestant (default: default_profile in your global config).
# profile = "default"
timeout = "10m"
# Best-effort cost cap per contestant; only Claude has a budget flag, the others warn.
# budget_usd = 2.00
# Sandbox size overrides (default: [resources] in your global config).
# cpus = 2
# memory = "4g"
# Run each contestant this many times (default 1).
# repeat = 1

# Each contestant needs its provider's secret stored in sbx (see `sbx secret ls`):
# claude -> anthropic, codex -> openai, antigravity -> google.
# A contestant can also set `profile = "<name>"` to use a different profile than [run].
[[contestants]]
harness = "claude"
model = "claude-opus-5-5"

[[contestants]]
harness = "codex"
model = "gpt-5.6-sol"

# A third contestant: remove the leading "# " from these three lines. Antigravity
# needs a stored `google` secret and a sign-in inside its sandbox (`agy models`
# lists its models).
# [[contestants]]
# harness = "antigravity"
# model = "gemini-3.1-pro-high"

# Optional scoring. With none of this, the run only captures answers, diffs and transcripts.
#
# Executable checks run in each contestant's sandbox after the agent finishes:
# [[eval.checks]]
# id = "tests-pass"
# command = "cargo test"
#
# A rubric is scored by an LLM judge (and, later, by humans):
# [[eval.rubric]]
# id = "correctness"
# kind = "pass_fail"
# weight = 1.0
#
# [[eval.rubric]]
# id = "code-quality"
# kind = "scale"
# levels = ["poor", "fair", "good", "excellent"]
# weight = 0.5
# notes = "Idiomatic for the language, no dead code"
#
# [eval.judge]
# harness = "claude"
# model = "claude-opus-5-5"
"##;

/// Writes the starter to `path` (or `./run.toml`) and returns where it went.
pub fn run(path: Option<&Path>) -> Result<PathBuf> {
    let path = path.unwrap_or_else(|| Path::new(DEFAULT_PATH));
    if path.exists() {
        bail!(
            "{} already exists; not overwriting it, pick another path or delete it",
            path.display()
        );
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, STARTER).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path.to_path_buf())
}
