//! `sbxm run show <run-id>`: prints a saved run from its files (decision 100).
//! Nothing is created or called; the id is validated before any path is built
//! from it (P2).

use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::GlobalConfig;
use crate::eval::score;
use crate::run::id;

#[derive(Debug, Default)]
pub struct Options {
    /// Print every pair's full patch, not only its summary.
    pub full_diff: bool,
}

pub fn render(config_dir: &Path, run_id: &str, options: &Options) -> Result<String> {
    if !id::is_valid(run_id) {
        bail!(
            "{run_id:?} isn't a run id; run ids look like 2026-09-30-a1b2c3 (`sbxm run` prints \
             one as `Run <id>`)"
        );
    }
    let global = GlobalConfig::load(config_dir)?;
    let meta = global.base_dir.join(".sbxm").join("runs").join(run_id);
    if !meta.is_dir() {
        bail!(
            "no run {run_id} under {}; check the id `sbxm run` printed",
            meta.parent().unwrap_or(&meta).display()
        );
    }
    let record = read_json(&meta.join("run.json"))?;

    let mut out = String::new();
    let text = |value: &Value| value.as_str().unwrap_or("unknown").to_owned();
    let contestants = record["contestants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let repeat = record["repeat"].as_u64().unwrap_or(1) as u32;
    writeln!(out, "Run {run_id}")?;
    match record["completed_at"].as_str() {
        Some(done) => writeln!(
            out,
            "Started {}, completed {done}",
            text(&record["started_at"])
        )?,
        None => writeln!(
            out,
            "Started {}, not completed (still running, or interrupted)",
            text(&record["started_at"])
        )?,
    }
    writeln!(
        out,
        "Profile {}, sbx {}, {} contestants, repeat {repeat}",
        text(&record["profile"]),
        text(&record["sbx_version"]),
        contestants.len()
    )?;

    // The ranking, recomputed from the saved files (decision 120); a run
    // without a judge has none, and a damaged config copy only loses this block.
    if let Ok(Some(ranking)) = score::load(&meta) {
        let labels: Vec<String> = contestants
            .iter()
            .map(|c| format!("{}/{}", text(&c["harness"]), text(&c["model"])))
            .collect();
        writeln!(out)?;
        write!(out, "{}", score::render(&ranking, &labels))?;
    }

    for (n, contestant) in contestants.iter().enumerate() {
        let i = contestant["index"].as_u64().unwrap_or(n as u64);
        writeln!(out)?;
        writeln!(
            out,
            "contestants[{i}] {}/{}",
            text(&contestant["harness"]),
            text(&contestant["model"])
        )?;
        for r in 0..repeat {
            let dir = meta.join(i.to_string()).join(r.to_string());
            let label = format!("  repeat {}/{repeat}", r + 1);
            let result_path = dir.join("result.json");
            if !result_path.is_file() {
                writeln!(
                    out,
                    "{label}: no results saved (the run is still going, or was interrupted)"
                )?;
                continue;
            }
            let result = read_json(&result_path)?;
            writeln!(out, "{label}: {}", describe(&result))?;
            write_answer(&mut out, &dir)?;
            write_diff(&mut out, &dir, &result["diff"], options.full_diff)?;
            write_checks(&mut out, &dir)?;
            write_judge(&mut out, &dir)?;
        }
    }
    Ok(out)
}

fn read_json(path: &Path) -> Result<Value> {
    if !path.is_file() {
        bail!(
            "{} is missing; that folder isn't a saved run",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn describe(result: &Value) -> String {
    let error = result["error"].as_str().unwrap_or("no details");
    match result["status"].as_str().unwrap_or("unknown") {
        "completed" => {
            let usage = &result["usage"];
            let cost = usage["cost_usd"]
                .as_f64()
                .map(|c| format!(", ${c:.4}"))
                .unwrap_or_default();
            format!(
                "completed ({} in, {} out tokens{cost})",
                usage["input_tokens"].as_u64().unwrap_or(0),
                usage["output_tokens"].as_u64().unwrap_or(0)
            )
        }
        "timed_out" => "timed out (partial output kept)".to_owned(),
        "failed" => format!("failed: {error}"),
        "error" => format!("error: {error}"),
        other => other.to_owned(),
    }
}

fn write_answer(out: &mut String, dir: &Path) -> Result<()> {
    let answer = fs::read_to_string(dir.join("answer.md")).unwrap_or_default();
    if answer.trim().is_empty() {
        writeln!(out, "    Answer: (none)")?;
    } else {
        writeln!(out, "    Answer:")?;
        for line in answer.trim_end().lines() {
            writeln!(out, "      {line}")?;
        }
    }
    Ok(())
}

fn write_diff(out: &mut String, dir: &Path, diff: &Value, full: bool) -> Result<()> {
    match diff["status"].as_str().unwrap_or("none") {
        "ok" => {
            let path = dir.join("diff.patch");
            match fs::read_to_string(&path) {
                Err(_) => writeln!(out, "    Diff: (diff.patch is missing)")?,
                Ok(patch) if patch.trim().is_empty() => writeln!(out, "    Diff: no changes")?,
                Ok(patch) => {
                    let files = summarize(&patch);
                    let (added, removed) = files
                        .iter()
                        .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
                    let noun = if files.len() == 1 { "file" } else { "files" };
                    writeln!(
                        out,
                        "    Diff: {} {noun} changed (+{added} -{removed})",
                        files.len()
                    )?;
                    for file in &files {
                        writeln!(out, "      {}", file.line())?;
                    }
                    writeln!(out, "    Patch: {}", path.display())?;
                    if full {
                        for line in patch.lines() {
                            writeln!(out, "      {line}")?;
                        }
                    }
                }
            }
            let skipped: Vec<&str> = diff["skipped"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !skipped.is_empty() {
                writeln!(out, "    Left out (links): {}", skipped.join(", "))?;
            }
        }
        "error" => writeln!(
            out,
            "    Diff: could not be captured: {}",
            diff["error"].as_str().unwrap_or("no details")
        )?,
        _ => writeln!(out, "    Diff: none (the pair never got a sandbox)")?,
    }
    Ok(())
}

/// The executable checks' verdicts, if the pair has any (slice 10).
fn write_checks(out: &mut String, dir: &Path) -> Result<()> {
    let path = dir.join("evals.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let evals: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let Some(checks) = evals["checks"].as_array().filter(|c| !c.is_empty()) else {
        return Ok(());
    };
    let passed = checks.iter().filter(|c| c["passed"] == true).count();
    writeln!(out, "    Checks: {passed}/{} passed", checks.len())?;
    for check in checks {
        let id = check["id"].as_str().unwrap_or("?");
        if check["passed"] == true {
            writeln!(out, "      passed {id}")?;
            continue;
        }
        let why = if check["timed_out"] == true {
            format!(
                "timed out after {}s",
                check["timeout_secs"].as_u64().unwrap_or(0)
            )
        } else if let Some(code) = check["exit_code"].as_i64() {
            format!("exit code {code}")
        } else {
            format!(
                "could not run: {}",
                check["error"].as_str().unwrap_or("no details")
            )
        };
        writeln!(out, "      failed {id} ({why})")?;
    }
    Ok(())
}

/// The judge's verdict on the pair, with the blind label it was judged under
/// (the mapping is revealed here, not during evaluation; decision 21).
fn write_judge(out: &mut String, dir: &Path) -> Result<()> {
    let path = dir.join("evals.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let evals: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let judge = &evals["judge"];
    if !judge.is_object() {
        return Ok(());
    }
    let label = judge["label"].as_str().unwrap_or("?");
    if judge["status"] != "ok" {
        writeln!(
            out,
            "    Judge (candidate {label}): not scored: {}",
            judge["error"].as_str().unwrap_or("no details")
        )?;
        return Ok(());
    }
    let criteria = judge["criteria"].as_object().cloned().unwrap_or_default();
    let mut line = format!("    Judge (candidate {label}):");
    let scored: Vec<String> = criteria
        .iter()
        .map(|(id, c)| {
            let value = match &c["value"] {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            format!("{id} {value} ({:.2})", c["score"].as_f64().unwrap_or(0.0))
        })
        .collect();
    if !scored.is_empty() {
        line.push(' ');
        line.push_str(&scored.join(", "));
    }
    let unscored: Vec<&str> = judge["unscored"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !unscored.is_empty() {
        line.push_str(&format!("; unscored: {}", unscored.join(", ")));
    }
    writeln!(out, "{line}")?;
    for (id, c) in &criteria {
        if let Some(reason) = c["reason"].as_str().filter(|r| !r.is_empty()) {
            writeln!(out, "      {id}: {reason}")?;
        }
    }
    Ok(())
}

/// One file of a unified diff.
struct FileChange {
    status: char,
    path: String,
    from: Option<String>,
    added: usize,
    removed: usize,
    binary: bool,
}

impl FileChange {
    fn line(&self) -> String {
        let path = match &self.from {
            Some(from) => format!("{from} -> {}", self.path),
            None => self.path.clone(),
        };
        let counts = if self.binary {
            "binary".to_owned()
        } else {
            match (self.added, self.removed) {
                (0, 0) => "no line changes".to_owned(),
                (a, 0) => format!("+{a}"),
                (0, r) => format!("-{r}"),
                (a, r) => format!("+{a} -{r}"),
            }
        };
        format!("{} {path} ({counts})", self.status)
    }
}

/// Files, status and line counts from a unified diff, as `git diff` prints it.
fn summarize(patch: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut in_hunk = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            in_hunk = false;
            let path = header
                .rsplit_once(" b/")
                .map_or(header, |(_, path)| path)
                .to_owned();
            files.push(FileChange {
                status: 'M',
                path,
                from: None,
                added: 0,
                removed: 0,
                binary: false,
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk {
            match line.chars().next() {
                Some('+') => file.added += 1,
                Some('-') => file.removed += 1,
                _ => {}
            }
        } else if line.starts_with("new file mode") {
            file.status = 'A';
        } else if line.starts_with("deleted file mode") {
            file.status = 'D';
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.status = 'R';
            file.from = Some(from.to_owned());
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        }
    }
    files
}
