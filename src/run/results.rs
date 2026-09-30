//! What a run leaves on disk under `<base>/.sbxm/runs/<run-id>/` (decisions
//! 16, 25, 28, 46, 55, 111, 112). Plain files are the source of truth:
//!
//! ```text
//! run.json            identity, written before any sandbox exists; gains
//!                     `completed_at` only when everything finished and saved
//! run-config.toml     a verbatim copy of the run-config (rubric included)
//! kits/               the generated kits (slice 5a)
//! <contestant>/<repeat>/
//!     answer.md  diff.patch  transcript.jsonl  result.json
//! ```
//!
//! Each pair's files are written as soon as that pair finishes, `result.json`
//! last, so its presence means the pair's files are complete. Contestant
//! directories are named by index, never by harness or model text (P2).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::config::RunConfig;
use super::id::date_from_days;
use super::kits::RunKits;
use super::orchestrate::PairOutcome;
use crate::headless::RunStatus;

/// Seconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `2026-09-30T14:13:20Z`.
pub fn format_timestamp(secs: u64) -> String {
    let (h, m, s) = (secs % 86_400 / 3_600, secs % 3_600 / 60, secs % 60);
    format!("{}T{h:02}:{m:02}:{s:02}Z", date_from_days(secs / 86_400))
}

/// What `run.json` records about the run itself.
pub struct RunStart<'a> {
    pub run_id: &'a str,
    pub started_at: u64,
    /// `None` if `sbx version` failed; recorded as `null`.
    pub sbx_version: Option<String>,
    /// `sbx skills ls --json`, verbatim (decision 46).
    pub skills: Value,
    pub run_config: &'a RunConfig,
    pub kits: &'a RunKits,
}

/// Writes `run.json` and the run-config copy into `meta`, before any sandbox
/// exists, so an interrupted run still keeps its identity and config hashes.
pub fn write_run_start(meta: &Path, start: &RunStart) -> Result<()> {
    let config = start.run_config;
    let kits = start.kits;
    let record = json!({
        "run_id": start.run_id,
        "started_at": format_timestamp(start.started_at),
        "completed_at": null,
        "sbxm_version": env!("CARGO_PKG_VERSION"),
        "sbx_version": start.sbx_version,
        "skills": start.skills,
        "profile": kits.profile_name,
        "resources": {"cpus": kits.resources.cpus, "memory": kits.resources.memory},
        "repeat": config.run.repeat,
        "contestants": config.contestants.iter().enumerate().map(|(i, c)| json!({
            "index": i,
            "harness": c.harness.as_str(),
            "model": c.model,
            // Per-contestant profiles arrive in slice 12b; until then it is the run's.
            "profile": kits.profile_name,
        })).collect::<Vec<_>>(),
        "harnesses": kits.harnesses.iter().map(|h| json!({
            "harness": h.harness.as_str(),
            "config_hash": h.config_hash,
            "kits": h.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    write_json(&meta.join("run.json"), &record)?;
    fs::copy(&config.path, meta.join("run-config.toml"))
        .with_context(|| format!("cannot copy {} into the run", config.path.display()))?;
    Ok(())
}

/// Adds `completed_at` to `run.json`, keeping everything else as it was.
pub fn mark_completed(meta: &Path, completed_at: u64) -> Result<()> {
    let path = meta.join("run.json");
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut record: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    record["completed_at"] = json!(format_timestamp(completed_at));
    write_json(&path, &record)
}

/// Writes one pair's files under `<meta>/<contestant>/<repeat>/`.
pub fn write_pair(meta: &Path, run_config: &RunConfig, outcome: &PairOutcome) -> Result<()> {
    let dir = pair_dir(meta, outcome.contestant, outcome.repeat);
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
    };

    let (status, error, usage) = match &outcome.result {
        Ok(result) => {
            write("answer.md", &result.answer)?;
            write("transcript.jsonl", &result.transcript)?;
            let usage = json!({
                "input_tokens": result.usage.input_tokens,
                "output_tokens": result.usage.output_tokens,
                "cost_usd": result.usage.cost_usd,
            });
            match &result.status {
                RunStatus::Completed => ("completed", Value::Null, usage),
                RunStatus::TimedOut => ("timed_out", Value::Null, usage),
                RunStatus::Failed(why) => ("failed", json!(why), usage),
            }
        }
        Err(why) => ("error", json!(why), Value::Null),
    };
    let diff = match &outcome.diff {
        Some(Ok(diff)) => {
            write("diff.patch", &diff.patch)?;
            json!({
                "status": "ok",
                "skipped": diff.skipped.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            })
        }
        Some(Err(why)) => json!({"status": "error", "error": why}),
        None => json!({"status": "none"}),
    };

    let contestant = &run_config.contestants[outcome.contestant];
    let record = json!({
        "contestant": outcome.contestant,
        "repeat": outcome.repeat,
        "harness": contestant.harness.as_str(),
        "model": contestant.model,
        "sandbox": outcome.sandbox,
        "status": status,
        "error": error,
        "usage": usage,
        "diff": diff,
        "remove_error": outcome.remove_error,
    });
    // Last: its presence means the pair's other files are all in place.
    write_json(&dir.join("result.json"), &record)
}

fn pair_dir(meta: &Path, contestant: usize, repeat: u32) -> PathBuf {
    meta.join(contestant.to_string()).join(repeat.to_string())
}

/// Pretty JSON written through a temp file and a rename, so a reader (or a
/// killed run) never sees half a file.
fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}
