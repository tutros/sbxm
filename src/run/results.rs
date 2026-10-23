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
use super::id::{date_from_days, days_from_civil};
use super::kits::RunKits;
use super::orchestrate::PairOutcome;
use crate::eval::cosine::CosineEntry;
use crate::eval::jev::JevRun;
use crate::eval::judge::JudgeRun;
use crate::headless::RunStatus;
use std::collections::BTreeMap;

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

/// Inverse of [`format_timestamp`]; `None` if `s` isn't exactly that shape (used to read a
/// stamp `sbxm` itself wrote, e.g. a task's `Stamp::at`, never free-form input).
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add((hour * 3_600 + minute * 60 + second) as i64)?;
    u64::try_from(secs).ok()
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
            // Its own profile, else the run's (decision 115).
            "profile": c.profile.as_deref().unwrap_or(&kits.profile_name),
        })).collect::<Vec<_>>(),
        "harnesses": kits.harnesses.iter().map(|h| json!({
            "profile": h.profile,
            "harness": h.harness.as_str(),
            "config_hash": h.config_hash,
            // Always present: the tag, or null for the install steps (decision 246).
            "template": h.template,
            "kits": h.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    write_json(&meta.join("run.json"), &record)?;
    fs::copy(&config.path, meta.join("run-config.toml"))
        .with_context(|| format!("cannot copy {} into the run", config.path.display()))?;
    Ok(())
}

/// Adds `completed_at` to `run.json`, keeping everything else as it was, and with it
/// `jev_models` when the run has `[eval.jev]`: the distinct models that answered, sorted
/// (decision 241 c).
pub fn mark_completed(meta: &Path, completed_at: u64, jev_models: Option<&[String]>) -> Result<()> {
    let path = meta.join("run.json");
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut record: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    record["completed_at"] = json!(format_timestamp(completed_at));
    if let Some(models) = jev_models {
        record["jev_models"] = json!(models);
    }
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

    if !outcome.checks.is_empty() {
        let checks: Vec<Value> = outcome
            .checks
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "command": c.command,
                    "passed": c.passed,
                    "exit_code": c.exit_code,
                    "timed_out": c.timed_out,
                    "timeout_secs": c.timeout_secs,
                    "error": c.error,
                    "output_tail": c.output_tail,
                })
            })
            .collect();
        merge_evals(&dir, "checks", Value::Array(checks))?;
    }

    let contestant = &run_config.contestants[outcome.contestant];
    let record = json!({
        "contestant": outcome.contestant,
        "repeat": outcome.repeat,
        "harness": contestant.harness.as_str(),
        "model": contestant.model,
        "profile": outcome.profile,
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

/// Saves one judge call: `<meta>/judge/<repeat>/{judge.json, reply.txt,
/// transcript.jsonl}` (with the label mapping for that repeat index), and each
/// judged pair's verdict under the key `judge` of its `evals.json`. Every
/// judged pair gets an entry, an error included, so the mapping is never lost.
pub fn write_judge(meta: &Path, run_config: &RunConfig, run: &JudgeRun) -> Result<()> {
    let judge = run_config
        .eval
        .judge
        .as_ref()
        .context("a judge run without a judge in the config")?;
    let who = json!({"harness": judge.harness.as_str(), "model": judge.model});
    let dir = meta.join("judge").join(run.repeat.to_string());
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    if let Some(answer) = &run.answer {
        fs::write(dir.join("reply.txt"), answer)
            .with_context(|| format!("cannot write {}", dir.join("reply.txt").display()))?;
    }
    if let Some(transcript) = &run.transcript {
        fs::write(dir.join("transcript.jsonl"), transcript)
            .with_context(|| format!("cannot write {}", dir.join("transcript.jsonl").display()))?;
    }

    let labels: serde_json::Map<String, Value> = run
        .labels
        .iter()
        .map(|(contestant, label)| (label.to_string(), json!(contestant)))
        .collect();
    let error = run.result.as_ref().err();
    write_json(
        &dir.join("judge.json"),
        &json!({
            "repeat": run.repeat,
            "sandbox": run.sandbox,
            "judge": who,
            "status": if error.is_none() { "ok" } else { "error" },
            "error": error,
            "labels": labels,
            "remove_error": run.remove_error,
        }),
    )?;

    for &(contestant, label) in &run.labels {
        let entry = match &run.result {
            Ok(parsed) => {
                let scores = &parsed.candidates[&label];
                let criteria: serde_json::Map<String, Value> = scores
                    .criteria
                    .iter()
                    .map(|(id, s)| {
                        (
                            id.clone(),
                            json!({"value": s.value, "score": s.score, "reason": s.reason}),
                        )
                    })
                    .collect();
                json!({
                    "label": label.to_string(), "status": "ok", "error": null, "judge": who,
                    "criteria": criteria, "unscored": scores.unscored,
                })
            }
            Err(why) => json!({
                "label": label.to_string(), "status": "error", "error": why, "judge": who,
            }),
        };
        let pair = pair_dir(meta, contestant, run.repeat);
        fs::create_dir_all(&pair).with_context(|| format!("cannot create {}", pair.display()))?;
        merge_evals(&pair, "judge", entry)?;
    }
    Ok(())
}

/// Merges one repeat's cosine similarities into each contestant's
/// `evals.json` (key `cosine`), one per entry in `entries` (issue #63).
pub fn write_cosine(
    meta: &Path,
    repeat: u32,
    entries: &BTreeMap<usize, CosineEntry>,
) -> Result<()> {
    for (&contestant, entry) in entries {
        let dir = pair_dir(meta, contestant, repeat);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        merge_evals(&dir, "cosine", entry.to_json())?;
    }
    Ok(())
}

/// Merges one repeat's Jev results into each scored pair's `evals.json` (key `jev`, decision
/// 241): the model that answered, the chunks, each criterion's 0-1 score and confidence, or
/// `{"status": "error", "error": ...}`.
pub fn write_jev(meta: &Path, run: &JevRun) -> Result<()> {
    for (contestant, result) in &run.results {
        let entry = match result {
            Ok(scores) => {
                let answered = &scores.answered;
                let criteria: serde_json::Map<String, Value> = answered
                    .criteria
                    .iter()
                    .map(|(id, s)| {
                        let mut c = json!({"score": s.score});
                        if let Some(confidence) = s.confidence {
                            c["confidence"] = json!(confidence);
                        }
                        (id.clone(), c)
                    })
                    .collect();
                let mut entry = json!({
                    "status": "ok",
                    "model": answered.model,
                    "chunks": scores.chunks,
                    "criteria": criteria,
                    "unscored": answered.unscored,
                    "usage": {
                        "input_tokens": answered.input_tokens,
                        "output_tokens": answered.output_tokens,
                    },
                });
                if scores.truncated {
                    entry["truncated"] = json!(true);
                }
                entry
            }
            Err(why) => json!({"status": "error", "error": why}),
        };
        let dir = pair_dir(meta, *contestant, run.repeat);
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        merge_evals(&dir, "jev", entry)?;
    }
    Ok(())
}

/// Sets `key` in the pair's `evals.json` (creating it), keeping the other
/// evaluators' entries: checks, judge, cosine and jev each own one key.
pub fn merge_evals(pair_dir: &Path, key: &str, value: Value) -> Result<()> {
    let path = pair_dir.join("evals.json");
    let mut evals = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        Err(_) => json!({}),
    };
    evals[key] = value;
    write_json(&path, &evals)
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
