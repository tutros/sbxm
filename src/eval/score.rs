//! Scoring and ranking in code (P9, decisions 19, 120), provisional until real
//! results show whether the rules fit. Every criterion is already 0-1 in
//! `evals.json`; a pair's score is the weight-normalised mean over the
//! criteria the judge scored, a contestant's score is the mean over its
//! repeats that have one, and contestants are ranked by it. Criteria and
//! repeats the judge didn't score are excluded and listed, never counted as 0;
//! ties rank equal; executable checks are reported alongside, not folded in.
//! Nothing here calls anything: the raw scores stay on disk, so the ranking can
//! be recomputed under other weights or rules.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::run::config::{Criterion, RunConfig};

/// Scores closer than this are a tie (floating-point noise).
const TIE_EPSILON: f64 = 1e-9;

/// What the ranking needs to know about one (contestant, repeat) pair.
#[derive(Debug, Clone)]
pub struct PairInput {
    pub contestant: usize,
    pub repeat: u32,
    /// `completed`, `timed_out`, `failed` or `error` (from `result.json`).
    pub status: String,
    /// The evaluator's 0-1 score per criterion id (the judge's, or Jev's for
    /// [`load_jev`]); `None` if it didn't score this pair at all (none, or it failed).
    pub judge: Option<BTreeMap<String, f64>>,
    pub checks_passed: usize,
    pub checks_total: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContestantScore {
    pub contestant: usize,
    /// Mean over the repeats that have a score; `None` if none does.
    pub score: Option<f64>,
    /// 1-based, equal scores share a rank (1, 2, 2, 4); `None` when unscored.
    pub rank: Option<usize>,
    pub repeats_scored: usize,
    pub repeats_total: usize,
    /// Repeats that timed out, or failed / never ran; still scored on what they left.
    pub timed_out: usize,
    pub failed: usize,
    /// Criteria the judge left unscored in at least one repeat.
    pub unscored: Vec<String>,
    pub checks_passed: usize,
    pub checks_total: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ranking {
    /// Best first; unscored contestants last, in index order.
    pub contestants: Vec<ContestantScore>,
}

/// Scores and ranks `contestants` contestants over `repeats` repeats from
/// `pairs`, under `rubric`'s weights.
pub fn rank(
    rubric: &[Criterion],
    contestants: usize,
    repeats: u32,
    pairs: &[PairInput],
) -> Ranking {
    let mut all: Vec<ContestantScore> = (0..contestants)
        .map(|contestant| {
            let mine: Vec<&PairInput> = pairs
                .iter()
                .filter(|p| p.contestant == contestant)
                .collect();
            let mut scores = Vec::new();
            let mut unscored: Vec<String> = Vec::new();
            for pair in &mine {
                let Some(judge) = &pair.judge else { continue };
                let (score, missing) = weighted(rubric, judge);
                scores.extend(score);
                for id in missing {
                    if !unscored.contains(&id) {
                        unscored.push(id);
                    }
                }
            }
            ContestantScore {
                contestant,
                score: (!scores.is_empty())
                    .then(|| scores.iter().sum::<f64>() / scores.len() as f64),
                rank: None,
                repeats_scored: scores.len(),
                repeats_total: repeats as usize,
                timed_out: mine.iter().filter(|p| p.status == "timed_out").count(),
                failed: mine
                    .iter()
                    .filter(|p| p.status == "failed" || p.status == "error")
                    .count(),
                unscored,
                checks_passed: mine.iter().map(|p| p.checks_passed).sum(),
                checks_total: mine.iter().map(|p| p.checks_total).sum(),
            }
        })
        .collect();

    all.sort_by(|a, b| match (a.score, b.score) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let mut previous: Option<(f64, usize)> = None;
    for (position, entry) in all.iter_mut().enumerate() {
        let Some(score) = entry.score else { continue };
        let rank = match previous {
            Some((last, rank)) if (last - score).abs() < TIE_EPSILON => rank,
            _ => position + 1,
        };
        entry.rank = Some(rank);
        previous = Some((score, rank));
    }
    Ranking { contestants: all }
}

/// The weight-normalised mean over the rubric criteria present in `judged`,
/// and the ids of those that aren't. `None` when none is present.
fn weighted(rubric: &[Criterion], judged: &BTreeMap<String, f64>) -> (Option<f64>, Vec<String>) {
    let (mut sum, mut weight) = (0.0, 0.0);
    let mut missing = Vec::new();
    for criterion in rubric {
        match judged.get(&criterion.id) {
            Some(score) => {
                sum += criterion.weight * score;
                weight += criterion.weight;
            }
            None => missing.push(criterion.id.clone()),
        }
    }
    ((weight > 0.0).then(|| sum / weight), missing)
}

/// Ranks a saved run from its files: the `run-config.toml` copy for the
/// rubric and weights (edit it to re-rank), and each pair's `result.json` and
/// `evals.json`. `None` when the run has no judge, so nothing was scored.
pub fn load(meta: &Path) -> Result<Option<Ranking>> {
    let config_path = meta.join("run-config.toml");
    let config = RunConfig::load(&config_path).with_context(|| {
        format!(
            "cannot read the run's config copy {}",
            config_path.display()
        )
    })?;
    if config.eval.judge.is_none() || config.eval.rubric.is_empty() {
        return Ok(None);
    }
    let mut pairs = Vec::new();
    for contestant in 0..config.contestants.len() {
        for repeat in 0..config.run.repeat {
            let dir = meta.join(contestant.to_string()).join(repeat.to_string());
            let Some(result) = read_json(&dir.join("result.json")) else {
                continue;
            };
            let evals = read_json(&dir.join("evals.json")).unwrap_or(Value::Null);
            let judge = (evals["judge"]["status"] == "ok").then(|| {
                evals["judge"]["criteria"]
                    .as_object()
                    .map(|criteria| {
                        criteria
                            .iter()
                            .filter_map(|(id, c)| Some((id.clone(), c["score"].as_f64()?)))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            let checks = evals["checks"].as_array().cloned().unwrap_or_default();
            pairs.push(PairInput {
                contestant,
                repeat,
                status: result["status"].as_str().unwrap_or("").to_owned(),
                judge,
                checks_passed: checks.iter().filter(|c| c["passed"] == true).count(),
                checks_total: checks.len(),
            });
        }
    }
    Ok(Some(rank(
        &config.eval.rubric,
        config.contestants.len(),
        config.run.repeat,
        &pairs,
    )))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Jev's own ranking (decision 241 a): ranked by the same rules as the judge's, never blended
/// with it, and the models that answered.
#[derive(Debug, Clone, PartialEq)]
pub struct JevRanking {
    pub ranking: Ranking,
    /// The distinct models that answered, sorted (`jev-1.13.0` for `jev-latest`).
    pub models: Vec<String>,
}

/// Ranks a saved run by Jev's scores alone, from the same files as [`load`] (the `jev` key of
/// each pair's `evals.json`). `None` when the run has no `[eval.jev]`.
pub fn load_jev(meta: &Path) -> Result<Option<JevRanking>> {
    let config_path = meta.join("run-config.toml");
    let config = RunConfig::load(&config_path).with_context(|| {
        format!(
            "cannot read the run's config copy {}",
            config_path.display()
        )
    })?;
    if config.eval.jev.is_none() || config.eval.rubric.is_empty() {
        return Ok(None);
    }
    let mut pairs = Vec::new();
    let mut models = std::collections::BTreeSet::new();
    for contestant in 0..config.contestants.len() {
        for repeat in 0..config.run.repeat {
            let dir = meta.join(contestant.to_string()).join(repeat.to_string());
            let Some(result) = read_json(&dir.join("result.json")) else {
                continue;
            };
            let evals = read_json(&dir.join("evals.json")).unwrap_or(Value::Null);
            let jev = &evals["jev"];
            let scores = (jev["status"] == "ok").then(|| {
                if let Some(model) = jev["model"].as_str() {
                    models.insert(model.to_owned());
                }
                jev["criteria"]
                    .as_object()
                    .map(|criteria| {
                        criteria
                            .iter()
                            .filter_map(|(id, c)| Some((id.clone(), c["score"].as_f64()?)))
                            .collect()
                    })
                    .unwrap_or_default()
            });
            let checks = evals["checks"].as_array().cloned().unwrap_or_default();
            pairs.push(PairInput {
                contestant,
                repeat,
                status: result["status"].as_str().unwrap_or("").to_owned(),
                judge: scores,
                checks_passed: checks.iter().filter(|c| c["passed"] == true).count(),
                checks_total: checks.len(),
            });
        }
    }
    Ok(Some(JevRanking {
        ranking: rank(
            &config.eval.rubric,
            config.contestants.len(),
            config.run.repeat,
            &pairs,
        ),
        models: models.into_iter().collect(),
    }))
}

/// Jev's ranking as text, under its own heading naming the models that answered; printed
/// after the judge's, which leads (decision 241 a).
pub fn render_jev(jev: &JevRanking, labels: &[String]) -> String {
    let models = if jev.models.is_empty() {
        "no model answered".to_owned()
    } else {
        jev.models.join(", ")
    };
    render_as(
        &format!(
            "Jev ranking ({models}; scores 0-1, repeats averaged, not blended with the judge's)"
        ),
        "Jev",
        &jev.ranking,
        labels,
    )
}

/// The ranking as text, `labels[i]` being `harness (model)` of contestant i.
pub fn render(ranking: &Ranking, labels: &[String]) -> String {
    render_as(
        "Ranking (judge scores 0-1, repeats averaged)",
        "the judge",
        ranking,
        labels,
    )
}

/// A ranking under `heading`, `scorer` naming who left a contestant unscored.
fn render_as(heading: &str, scorer: &str, ranking: &Ranking, labels: &[String]) -> String {
    let mut out = format!("{heading}\n");
    for c in &ranking.contestants {
        let who = format!(
            "contestants[{}] {}",
            c.contestant,
            labels.get(c.contestant).map_or("?", String::as_str)
        );
        let Some((score, rank)) = c.score.zip(c.rank) else {
            let _ = writeln!(
                out,
                "  -  {who}: not scored ({scorer} scored none of its repeats)"
            );
            continue;
        };
        let mut notes = vec![format!(
            "{}/{} repeats scored",
            c.repeats_scored, c.repeats_total
        )];
        if c.checks_total > 0 {
            notes.push(format!(
                "checks {}/{} passed",
                c.checks_passed, c.checks_total
            ));
        }
        if c.timed_out > 0 {
            notes.push(format!("{} timed out", c.timed_out));
        }
        if c.failed > 0 {
            notes.push(format!("{} failed", c.failed));
        }
        if !c.unscored.is_empty() {
            notes.push(format!("unscored: {}", c.unscored.join(", ")));
        }
        let _ = writeln!(out, "  {rank}. {who}: {score:.2} ({})", notes.join("; "));
    }
    out
}
