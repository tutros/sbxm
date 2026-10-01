//! Turning a judged value into a 0-1 score (P9, decision 19). The criteria
//! themselves are `run::config::Criterion`: one shape for the LLM judge, Jev
//! and human review (decision 18).

use serde_json::Value;

use crate::run::config::{Criterion, CriterionKind};

/// `pass_fail`: `true` is 1, `false` is 0. `scale` with n levels: the level's
/// index over n-1, so the worst level is 0 and the best is 1. `None` for a
/// value that isn't a valid answer for the criterion, which callers leave
/// unscored instead of counting as 0.
pub fn score(criterion: &Criterion, value: &Value) -> Option<f64> {
    match criterion.kind {
        CriterionKind::PassFail => value.as_bool().map(|pass| if pass { 1.0 } else { 0.0 }),
        CriterionKind::Scale => {
            let index = level_index(criterion, value.as_str()?)?;
            Some(index as f64 / (criterion.levels.len() - 1) as f64)
        }
    }
}

/// Where `text` sits in the criterion's levels, ignoring case and outer spaces.
pub fn level_index(criterion: &Criterion, text: &str) -> Option<usize> {
    let wanted = text.trim().to_lowercase();
    criterion
        .levels
        .iter()
        .position(|level| level.trim().to_lowercase() == wanted)
}
