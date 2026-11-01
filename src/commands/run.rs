//! `sbxm run <config>`: validate, check secrets, reserve the run, build and
//! validate the kits, then run the pairs (sdlc/milestone-2.md, "Command behavior").
//! Everything that can be refused is refused before the first write.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::backend::SandboxBackend;
use crate::commands::task_log;
use crate::config::GlobalConfig;
use crate::disk;
use crate::eval::cosine::{self, CosineEntry, Embedder, FailingEmbedder, FastEmbedder};
use crate::eval::{jev, judge, score};
use crate::headless::RunStatus;
use crate::run::config::RunConfig;
use crate::run::log::{self as run_log, RunFolderLog};
use crate::run::orchestrate::{self, PairOutcome};
use crate::run::{id, kits, preflight, results};
use crate::task::review::agent_label;
use crate::task::runlog::Tee;

#[derive(Debug)]
pub struct Summary {
    pub run_id: String,
    pub outcomes: Vec<PairOutcome>,
}

impl Summary {
    /// Pairs sbxm could not run at all (`PairOutcome::result` is `Err`: seeding, sandbox
    /// creation or the exec itself failed), as opposed to one that ran and timed out or that the
    /// harness reported failing, which is a result, not an error (issue #126c).
    pub fn errored_pairs(&self) -> usize {
        self.outcomes.iter().filter(|o| o.result.is_err()).count()
    }
}

/// The error of a run that finished, printed and saved everything, but had pairs sbxm could not
/// run (decision 181). It carries the run's summary, which is otherwise complete.
#[derive(Debug)]
pub struct PairsErrored {
    pub summary: Summary,
}

impl std::fmt::Display for PairsErrored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} of {} contestant runs ended in error",
            self.summary.errored_pairs(),
            self.summary.outcomes.len()
        )
    }
}

impl std::error::Error for PairsErrored {}

/// Runs the comparison in `config_path`, printing the run ID and one line per
/// pair to `out` and warnings to `warn`. Fails with [`PairsErrored`], after all of that, when a
/// pair could not be run.
pub fn run(
    config_dir: &Path,
    config_path: &Path,
    disk: &disk::Guard,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<Summary> {
    run_with(
        config_dir,
        config_path,
        disk,
        backend,
        out,
        warn,
        None,
        &std::thread::sleep,
    )
}

/// Like [`run`], with `sleep` called for each pause between Jev retries instead of really
/// sleeping: a seam for testing the backoff (issue 285).
pub fn run_with_sleep(
    config_dir: &Path,
    config_path: &Path,
    disk: &disk::Guard,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    sleep: &dyn Fn(Duration),
) -> Result<Summary> {
    run_with(
        config_dir,
        config_path,
        disk,
        backend,
        out,
        warn,
        None,
        sleep,
    )
}

/// Like [`run`], but runs `[eval.cosine]` (if configured) against `embedder`
/// instead of loading `FastEmbedder` from `model_dir`: a seam for testing the
/// runtime embedding-failure path (an already-loaded model whose `embed`
/// call fails) without a real model file (issue #63 review finding M-1).
pub fn run_with_embedder(
    config_dir: &Path,
    config_path: &Path,
    disk: &disk::Guard,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    embedder: &dyn Embedder,
) -> Result<Summary> {
    run_with(
        config_dir,
        config_path,
        disk,
        backend,
        out,
        warn,
        Some(embedder),
        &std::thread::sleep,
    )
}

/// Copies everything printed to `out` and `warn` into the run folder's `run.log` (decision 170,
/// part 3), and ends it with the error `main` prints for a failed run, so the screen is unchanged.
#[allow(clippy::too_many_arguments)]
fn run_with(
    config_dir: &Path,
    config_path: &Path,
    disk: &disk::Guard,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    embedder_override: Option<&dyn Embedder>,
    sleep: &dyn Fn(Duration),
) -> Result<Summary> {
    let log = match task_log::header() {
        Ok(header) => RunFolderLog::new(
            header,
            Box::new(results::now),
            Box::new(|msg| eprintln!("{msg}")),
        ),
        Err(e) => {
            writeln!(warn, "warning: cannot open the run log: {e:#}")?;
            RunFolderLog::disabled()
        }
    };
    let log = Arc::new(Mutex::new(log));
    let result = {
        let mut out = Tee::new(out, Arc::clone(&log));
        let mut warn = Tee::new(warn, Arc::clone(&log));
        run_logged(
            config_dir,
            config_path,
            disk,
            backend,
            &mut out,
            &mut warn,
            &log,
            embedder_override,
            sleep,
        )
    };
    if let Err(err) = &result
        && let Ok(mut log) = log.lock()
    {
        log.lines(&format!("Error: {err:?}"));
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_logged(
    config_dir: &Path,
    config_path: &Path,
    disk: &disk::Guard,
    backend: &dyn SandboxBackend,
    out: &mut dyn Write,
    warn: &mut dyn Write,
    log: &Mutex<RunFolderLog>,
    embedder_override: Option<&dyn Embedder>,
    sleep: &dyn Fn(Duration),
) -> Result<Summary> {
    let stage = |stage| {
        if let Ok(mut log) = log.lock() {
            log.set_stage(stage);
        }
    };
    let run_config = RunConfig::load(config_path)?;
    let global = GlobalConfig::load(config_dir)?;
    // Before preflight's calls to `sbx`, like every other refusal (decision 194).
    disk::require(&global, config_dir, disk)?;
    let checked = preflight::check(config_dir, &run_config, backend)?;
    for warning in &checked.warnings {
        writeln!(warn, "warning: {warning}")?;
    }
    if !global.base_dir.is_dir() {
        bail!(
            "base dir {} does not exist; create it or change base_dir in {}",
            global.base_dir.display(),
            config_dir.join("config.toml").display()
        );
    }

    let roots = id::reserve(&global.base_dir)?;
    if let Ok(mut log) = log.lock() {
        log.attach(&roots.meta);
    }
    let run_kits = kits::build_with_templates(
        config_dir,
        &run_config,
        &checked.templates,
        &roots.meta.join("kits"),
        backend,
    )?;
    // The Jev evaluator's own kit, validated with the others before any sandbox exists.
    let jev_kit = match &run_config.eval.jev {
        Some(_) => Some(jev::build_kit(
            &roots.meta.join("kits"),
            &run_kits.resources,
            backend,
        )?),
        None => None,
    };

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

    stage(run_log::PAIRS);
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
            "contestants[{}] {}{repeat}: {}{}",
            outcome.contestant,
            agent_label(contestant.harness.as_str(), Some(&contestant.model)),
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
        stage(run_log::JUDGE);
        let repeats = run_config.run.repeat;
        for repeat in 0..repeats {
            let pairs: Vec<&PairOutcome> = outcomes.iter().filter(|o| o.repeat == repeat).collect();
            let Some(judged) =
                judge::judge_repeat(backend, &run_config, &run_kits, &roots, repeat, &pairs)
            else {
                continue;
            };
            let who = agent_label(judge.harness.as_str(), Some(&judge.model));
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
        stage(run_log::COSINE);
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

    // Jev scores each pair on its own, in one evaluator sandbox per repeat; a failure is that
    // pair's result and a warning, never fatal and never the judge's business (decision 241).
    let mut jev_saved = true;
    let mut jev_models = std::collections::BTreeSet::new();
    if let (Some(jev_config), Some(jev_kit)) = (&run_config.eval.jev, &jev_kit) {
        stage(run_log::JEV);
        let repeats = run_config.run.repeat;
        for repeat in 0..repeats {
            let pairs: Vec<&PairOutcome> = outcomes.iter().filter(|o| o.repeat == repeat).collect();
            let Some(scored) =
                jev::evaluate_repeat(backend, &run_config, jev_kit, &roots, repeat, &pairs, sleep)
            else {
                continue;
            };
            let ok = scored.results.iter().filter(|(_, r)| r.is_ok()).count();
            writeln!(
                out,
                "Jev {}: repeat {}/{repeats} scored {ok} of {} contestants",
                jev_config.model,
                repeat + 1,
                scored.results.len()
            )?;
            for (contestant, result) in &scored.results {
                match result {
                    Ok(pair) => {
                        jev_models.insert(pair.answered.model.clone());
                    }
                    Err(why) => writeln!(
                        warn,
                        "warning: Jev could not score contestants[{contestant}]{}: {why}",
                        repeat_label(&run_config, repeat)
                    )?,
                }
            }
            for problem in [&scored.remove_error, &scored.secret_remove_error]
                .into_iter()
                .flatten()
            {
                writeln!(warn, "warning: {problem}")?;
            }
            if let Err(e) = results::write_jev(&roots.meta, &scored) {
                jev_saved = false;
                writeln!(
                    warn,
                    "warning: cannot save the Jev results for repeat {}/{repeats}: {e:#}",
                    repeat + 1
                )?;
            }
        }
    }

    stage(run_log::RESULTS);
    // The ranking is computed from what was just saved, exactly as `run show`
    // will later, so the two can't disagree (decisions 19, 120).
    let labels: Vec<String> = run_config
        .contestants
        .iter()
        .map(|c| agent_label(c.harness.as_str(), Some(&c.model)))
        .collect();
    match score::load(&roots.meta) {
        Ok(Some(ranking)) => write!(out, "{}", score::render(&ranking, &labels))?,
        Ok(None) => {}
        Err(e) => writeln!(warn, "warning: cannot rank the run: {e:#}")?,
    }
    // Jev's ranking after the judge's, which leads; never blended (decision 241 a).
    match score::load_jev(&roots.meta) {
        Ok(Some(ranking)) => write!(out, "{}", score::render_jev(&ranking, &labels))?,
        Ok(None) => {}
        Err(e) => writeln!(warn, "warning: cannot rank the run by Jev: {e:#}")?,
    }
    // Jev's raw answers after its ranking, shown only (issue 343).
    match score::render_jev_questions(&roots.meta, &labels) {
        Ok(Some(text)) => write!(out, "{text}")?,
        Ok(None) => {}
        Err(e) => writeln!(warn, "warning: cannot show Jev's raw answers: {e:#}")?,
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

    // Complete only if every pair's results (and the judge's, cosine's and Jev's) are on disk.
    if outcomes.iter().all(|o| o.save_error.is_none()) && judge_saved && cosine_saved && jev_saved {
        let jev_models: Option<Vec<String>> = run_config
            .eval
            .jev
            .as_ref()
            .map(|_| jev_models.into_iter().collect());
        if let Err(e) = results::mark_completed(&roots.meta, results::now(), jev_models.as_deref())
        {
            writeln!(warn, "warning: cannot mark the run completed: {e:#}")?;
        }
    } else {
        writeln!(
            warn,
            "warning: some results could not be saved, so run.json has no completed_at"
        )?;
    }
    writeln!(out, "Results: {}", roots.meta.display())?;
    let summary = Summary {
        run_id: roots.id,
        outcomes,
    };
    // Last, after everything is printed and saved: the run fails if a pair could not be run, so
    // the process exits non-zero the way every failing command does (decision 181).
    if summary.errored_pairs() > 0 {
        return Err(PairsErrored { summary }.into());
    }
    Ok(summary)
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
