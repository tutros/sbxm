//! Human review (issue 347, decisions 18, 21, 22): a person scores a saved run's contestants on
//! the run's rubric, under blind labels of its own, in a `scores.toml` per repeat that
//! [`parse_scores`] checks. Blank values are unscored, never 0, like the judge's.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use toml::Spanned;

use super::{judge, rubric};
use crate::run::config::{Criterion, CriterionKind};

/// Gives `contestants` the labels A, B, C, ... like the judge's [`judge::assign_labels`], but
/// shuffled with a seed of its own, so the judge's mapping (which `sbxm run` and `run show`
/// reveal) gives nothing away (answer 6 of issue 347). Returned in label order.
pub fn assign_labels(run_id: &str, repeat: u32, contestants: &[usize]) -> Vec<(usize, char)> {
    judge::shuffle_labels(&format!("human:{run_id}"), repeat, contestants)
}

/// The blank `scores.toml` of one repeat: a table per label, every criterion `""`, with its
/// allowed values and notes in comments above it.
pub fn scores_template(
    run_id: &str,
    repeat: u32,
    repeats: u32,
    rubric: &[Criterion],
    labels: &[char],
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Human review of run {run_id}, repeat {} of {repeats}.",
        repeat + 1
    );
    out.push_str(
        "# Each candidate's work is in the folder named after its label: answer.md and diff.patch,\n\
         # or no-output.md when it left neither. The task is in ../task.md.\n\
         # Fill in the values you can; a value left \"\" stays unscored (never counted as 0).\n\
         # Scoring can take several sittings: this file is only read when you run\n",
    );
    let _ = writeln!(out, "#   sbxm run review {run_id} --submit");
    for label in labels {
        let _ = writeln!(out, "\n[{label}]");
        for criterion in rubric {
            if let Some(notes) = &criterion.notes {
                for line in notes.lines() {
                    let _ = writeln!(out, "# {line}");
                }
            }
            let _ = writeln!(out, "# {}", allowed(criterion));
            let _ = writeln!(out, "{} = \"\"", toml_key(&criterion.id));
        }
    }
    out
}

/// What a criterion accepts, as `scores.toml`'s comments and errors word it.
fn allowed(criterion: &Criterion) -> String {
    match criterion.kind {
        CriterionKind::PassFail => "pass_fail: true or false".to_owned(),
        CriterionKind::Scale => format!(
            "scale, worst to best: {}",
            criterion
                .levels
                .iter()
                .map(|level| toml_string(level))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn toml_string(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

/// A bare key when the id allows one, else a quoted one.
fn toml_key(id: &str) -> String {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        id.to_owned()
    } else {
        toml_string(id)
    }
}

/// One label's checked scores.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelScores {
    /// Per scored criterion id: the value as saved (`true`/`false`, or the level as the rubric
    /// spells it) and its 0-1 score.
    pub criteria: BTreeMap<String, (Value, f64)>,
    /// The rubric's criteria left blank or out, in rubric order.
    pub unscored: Vec<String>,
}

type Entries = BTreeMap<Spanned<String>, Spanned<toml::Value>>;

/// Checks one repeat's `scores.toml` (`text`, read from `path`) against the rubric and the
/// repeat's labels. Every label gets an entry, a label or criterion left out counting as blank;
/// a mistake is an error naming the file and line, and a file with nothing scored at all is
/// refused.
pub fn parse_scores(
    path: &Path,
    text: &str,
    rubric: &[Criterion],
    labels: &[char],
) -> Result<BTreeMap<char, LabelScores>> {
    let at = |offset: usize| {
        let line = text[..offset.min(text.len())].matches('\n').count() + 1;
        format!("{} line {line}", path.display())
    };
    let top: Entries = toml::from_str(text).map_err(|e| {
        let place = match e.span() {
            Some(span) => at(span.start),
            None => path.display().to_string(),
        };
        anyhow!(
            "{place}: {}; fix it, then run --submit again",
            e.message().replace('\n', " ")
        )
    })?;
    let names = labels
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    for (key, value) in &top {
        if !labels.iter().any(|l| key.get_ref() == &l.to_string()) {
            bail!(
                "{}: unknown label {:?}; the labels of this repeat are {names}",
                at(key.span().start),
                key.get_ref()
            );
        }
        if !value.get_ref().is_table() {
            bail!(
                "{}: {} must be a table ([{}]) of criterion values",
                at(key.span().start),
                key.get_ref(),
                key.get_ref()
            );
        }
    }
    // Every top-level value is a table now, so this reading can't fail.
    let tables: BTreeMap<String, Entries> = toml::from_str(text)?;
    let ids = rubric
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = BTreeMap::new();
    let mut any = false;
    for &label in labels {
        let table = tables.get(&label.to_string());
        let entry = |id: &str| table.and_then(|t| t.iter().find(|(key, _)| key.get_ref() == id));
        for key in table.into_iter().flat_map(|t| t.keys()) {
            if !rubric.iter().any(|c| &c.id == key.get_ref()) {
                bail!(
                    "{}: [{label}] has no criterion {:?}; the rubric's criteria are {ids}",
                    at(key.span().start),
                    key.get_ref()
                );
            }
        }
        let mut scores = LabelScores {
            criteria: BTreeMap::new(),
            unscored: Vec::new(),
        };
        for criterion in rubric {
            let found = entry(&criterion.id);
            match check(criterion, found.map(|(_, value)| value.get_ref())) {
                Ok(None) => scores.unscored.push(criterion.id.clone()),
                Ok(Some(saved)) => {
                    any = true;
                    scores.criteria.insert(criterion.id.clone(), saved);
                }
                Err(must) => bail!(
                    "{}: [{label}] {} {must} (or \"\" to leave it unscored)",
                    at(found.map_or(0, |(_, value)| value.span().start)),
                    criterion.id
                ),
            }
        }
        out.insert(label, scores);
    }
    if !any {
        bail!(
            "{}: no criterion is scored; fill in at least one value, then run --submit again",
            path.display()
        );
    }
    Ok(out)
}

/// `Ok(None)` for a blank or missing value, the saved value and its score for a valid one, and
/// what it must be otherwise.
fn check(
    criterion: &Criterion,
    value: Option<&toml::Value>,
) -> std::result::Result<Option<(Value, f64)>, String> {
    let must = match criterion.kind {
        CriterionKind::PassFail => "must be true or false".to_owned(),
        CriterionKind::Scale => format!(
            "must be one of {}",
            criterion
                .levels
                .iter()
                .map(|level| toml_string(level))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    match value {
        None => Ok(None),
        Some(toml::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(toml::Value::Boolean(pass)) if criterion.kind == CriterionKind::PassFail => {
            let saved = Value::Bool(*pass);
            let score = rubric::score(criterion, &saved).ok_or(must)?;
            Ok(Some((saved, score)))
        }
        Some(toml::Value::String(s)) if criterion.kind == CriterionKind::Scale => {
            let index = rubric::level_index(criterion, s).ok_or_else(|| must.clone())?;
            let saved = Value::String(criterion.levels[index].clone());
            let score = rubric::score(criterion, &saved).ok_or(must)?;
            Ok(Some((saved, score)))
        }
        Some(_) => Err(must),
    }
}
