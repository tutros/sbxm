//! `sbxm run <config>`: validate, check secrets, reserve the run, build and
//! validate the kits, then run the pairs (sdlc/milestone-2.md, "Command behavior").
//! Everything that can be refused is refused before the first write.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::config::GlobalConfig;
use crate::eval::cosine::{self, CosineEntry, Embedder, FailingEmbedder, FastEmbedder};
use crate::eval::{judge, score};
use crate::headless::RunStatus;
use crate::run::config::RunConfig;
use crate::run::orchestrate::{self, PairOutcome};
use crate::run::{id, kits, preflight, results};

#[derive(Debug)]
pub struct Summary {
    pub run_id: String,
    pub outcomes: Vec<PairOutcome>,
}

/// Runs the comparison in `config_path`, printing the run ID and one line per
/// pair to `out` and warnings to `warn`.
pub fn run(
    config_dir: &Path,
    config_path: &Path,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<Summary> {
    run_with(config_dir, config_path, backend, out, warn, None)
}

/// Like [`run`], but runs `[eval.cosine]` (if configured) against `embedder`
/// instead of loading `FastEmbedder` from `model_dir`: a seam for testing the
/// runtime embedding-failure path (an already-loaded model whose `embed`
/// call fails) without a real model file (issue #63 review finding M-1).
pub fn run_with_embedder(
    config_dir: &Path,
    config_path: &Path,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    embedder: &dyn Embedder,
) -> Result<Summary> {
    run_with(config_dir, config_path, backend, out, warn, Some(embedder))
}

fn run_with(
    config_dir: &Path,
    config_path: &Path,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    embedder_override: Option<&dyn Embedder>,
) -> Result<Summary> {
    let run_config = RunConfig::load(config_path)?;
    let checked = preflight::check(config_dir, &run_config, backend)?;
    for warning in &checked.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    let global = GlobalConfig::load(config_dir)?;
    if !global.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            global.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }

    let roots = id::reserve(&global.base_dir)?;
    let run_kits = kits::build(config_dir, &run_config, &roots.meta.join("kits"), backend)?;

    // The run's identity goes to disk before any sandbox exists, so an
    // interrupted run still keeps its config hashes (decisions 28, 55).
    let started_at = results::now();
    let sbx_version = match backend.version() {
        Ok(version) => Some(version),
        Err(e) => {
            writeln!(
                warn,
                "warning: cannot read the sbx version for run.json: {e:#}"
            )?;
            None
        }
    };
    let skills = backend.skills().unwrap_or_else(|e| {
        let _ = writeln!(
            warn,
            "warning: cannot list the sbx skills for run.json: {e:#}"
        );
        serde_json::Value::Null
    });
    results::write_run_start(
        &roots.meta,
        &results::RunStart {
            run_id: &roots.id,
            started_at,
            sbx_version,
            skills,
            run_config: &run_config,
            kits: &run_kits,
        },
    )?;

    writeln!(out, "Run {}", roots.id)?;
    // Each pair's files are saved the moment it finishes (decision 16).
    let save = |outcome: &PairOutcome| {
        results::write_pair(&roots.meta, &run_config, outcome).map_err(|e| {
            format!(
                "cannot save the results of contestants[{}]{}: {e:#}",
                outcome.contestant,
                repeat_label(&run_config, outcome.repeat)
            )
        })
    };
    let outcomes = orchestrate::execute_with(backend, &run_config, &run_kits, &roots, &save);

    for outcome in &outcomes {
        let contestant = &run_config.contestants[outcome.contestant];
        let repeat = repeat_label(&run_config, outcome.repeat);
        writeln!(
            out,
            "contestants[{}] {}/{}{repeat}: {}{}",
            outcome.contestant,
            contestant.harness.as_str(),
            contestant.model,
            describe(&outcome.result),
            checks_suffix(&outcome.checks)
        )?;
        if let Some(problem) = &outcome.remove_error {
            writeln!(warn, "warning: {problem}")?;
        }
        match &outcome.diff {
            Some(Err(problem)) => writeln!(
                warn,
                "warning: contestants[{}]{repeat}: {problem}",
                outcome.contestant
            )?,
            Some(Ok(diff)) if !diff.skipped.is_empty() => {
                let links: Vec<String> = diff
                    .skipped
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                writeln!(
                    warn,
                    "warning: contestants[{}]{repeat}: left links out of the diff: {}",
                    outcome.contestant,
                    links.join(", ")
                )?;
            }
            _ => {}
        }
        if let Some(problem) = &outcome.save_error {
            writeln!(warn, "warning: {problem}")?;
        }
    }

    // Once every pair is done, the judge scores each repeat index on its own
    // (P9); a judge that fails is recorded and warned about, not fatal.
    let mut judge_saved = true;
    if let Some(judge) = &run_config.eval.judge {
        let repeats = run_config.run.repeat;
        for repeat in 0..repeats {
            let pairs: Vec<&PairOutcome> = outcomes.iter().filter(|o| o.repeat == repeat).collect();
            let Some(judged) =
                judge::judge_repeat(backend, &run_config, &run_kits, &roots, repeat, &pairs)
            else {
                continue;
            };
            let who = format!("{}/{}", judge.harness.as_str(), judge.model);
            match &judged.result {
                Ok(_) => writeln!(
                    out,
                    "Judge {who}: repeat {}/{repeats} scored {} contestants",
                    repeat + 1,
                    judged.labels.len()
                )?,
                Err(why) => {
                    writeln!(
                        out,
                        "Judge {who}: repeat {}/{repeats} failed: {why}",
                        repeat + 1
                    )?;
                    writeln!(
                        warn,
                        "warning: the judge failed for repeat {}/{repeats}: {why}",
                        repeat + 1
                    )?;
                }
            }
            if let Some(problem) = &judged.remove_error {
                writeln!(warn, "warning: {problem}")?;
            }
            if let Err(e) = results::write_judge(&roots.meta, &run_config, &judged) {
                judge_saved = false;
                writeln!(
                    warn,
                    "warning: cannot save the judge's results for repeat {}/{repeats}: {e:#}",
                    repeat + 1
                )?;
            }
        }
    }

    // Cosine compares same-repeat-index answers, like the judge, but needs no
    // sandbox: a model-load failure is one warning, recorded on every
    // participant, never fatal (decision 166, issue #63). An already-loaded
    // model whose `embed` call fails at runtime gets the same treatment: one
    // warning for the whole run, not duplicated with the load-failure
    // warning above (review finding M-1).
    let mut cosine_saved = true;
    if let Some(cosine_cfg) = &run_config.eval.cosine {
        let (embedder, report_runtime_errors): (Box<dyn Embedder>, bool) = match embedder_override {
            Some(e) => (Box::new(e), true),
            None => {
                let model_dir = cosine_cfg.resolve_model_dir(config_dir);
                let loaded = FastEmbedder::from_dir(&model_dir);
                let model_loaded = loaded.is_ok();
                if let Err(e) = &loaded {
                    writeln!(warn, "warning: {e:#}")?;
                }
                let embedder = loaded
                    .map(|e| Box::new(e) as Box<dyn Embedder>)
                    .unwrap_or_else(|e| Box::new(FailingEmbedder(format!("{e:#}"))));
                (embedder, model_loaded)
            }
        };
        let repeats = run_config.run.repeat;
        // One warning for the whole run: the first runtime error. Every repeat
        // still keeps its own message in its pairs' evals.json (PR 69 review M-2).
        let mut first_runtime_error: Option<String> = None;
        for repeat in 0..repeats {
            let pairs: Vec<&PairOutcome> = outcomes.iter().filter(|o| o.repeat == repeat).collect();
            let entries = cosine::cosine_repeat(embedder.as_ref(), &pairs);
            if report_runtime_errors && first_runtime_error.is_none() {
                first_runtime_error = entries.values().find_map(|entry| match entry {
                    CosineEntry::Error(message) => Some(message.clone()),
                    _ => None,
                });
            }
            if let Err(e) = results::write_cosine(&roots.meta, repeat, &entries) {
                cosine_saved = false;
                writeln!(
                    warn,
                    "warning: cannot save the cosine results for repeat {}/{repeats}: {e:#}",
                    repeat + 1
                )?;
            }
        }
        if let Some(message) = &first_runtime_error {
            writeln!(warn, "warning: {message}")?;
        }
    }

    // The ranking is computed from what was just saved, exactly as `run show`
    // will later, so the two can't disagree (decisions 19, 120).
    match score::load(&roots.meta) {
        Ok(Some(ranking)) => {
            let labels: Vec<String> = run_config
                .contestants
                .iter()
                .map(|c| format!("{}/{}", c.harness.as_str(), c.model))
                .collect();
            write!(out, "{}", score::render(&ranking, &labels))?;
        }
        Ok(None) => {}
        Err(e) => writeln!(warn, "warning: cannot rank the run: {e:#}")?,
    }
    if run_config.eval.cosine.is_some() {
        write!(
            out,
            "{}",
            cosine::render(
                &roots.meta,
                run_config.contestants.len(),
                run_config.run.repeat
            )
        )?;
    }

    // Complete only if every pair's results (and the judge's and cosine's) are on disk.
    if outcomes.iter().all(|o| o.save_error.is_none()) && judge_saved && cosine_saved {
        if let Err(e) = results::mark_completed(&roots.meta, results::now()) {
            writeln!(warn, "warning: cannot mark the run completed: {e:#}")?;
        }
    } else {
        writeln!(
            warn,
            "warning: some results could not be saved, so run.json has no completed_at"
        )?;
    }
    writeln!(out, "Results: {}", roots.meta.display())?;
    Ok(Summary {
        run_id: roots.id,
        outcomes,
    })
}

/// ` repeat 2/3` when the run repeats, empty otherwise.
fn repeat_label(run_config: &RunConfig, repeat: u32) -> String {
    if run_config.run.repeat > 1 {
        format!(" repeat {}/{}", repeat + 1, run_config.run.repeat)
    } else {
        String::new()
    }
}

/// `; checks 1/2 passed`, or nothing when the pair had no checks.
fn checks_suffix(checks: &[crate::run::checks::CheckOutcome]) -> String {
    if checks.is_empty() {
        return String::new();
    }
    let passed = checks.iter().filter(|c| c.passed).count();
    format!("; checks {passed}/{} passed", checks.len())
}

fn describe(result: &Result<crate::headless::HeadlessResult, String>) -> String {
    match result {
        Ok(r) => match &r.status {
            RunStatus::Completed => format!(
                "completed ({} in, {} out tokens{})",
                r.usage.input_tokens,
                r.usage.output_tokens,
                r.usage
                    .cost_usd
                    .map(|c| format!(", ${c:.4}"))
                    .unwrap_or_default()
            ),
            RunStatus::TimedOut => "timed out (partial output kept)".to_owned(),
            RunStatus::Failed(why) => format!("failed: {why}"),
        },
        Err(why) => format!("error: {why}"),
    }
}
