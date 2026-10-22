//! The Jev evaluator (M2a slice 14a, decision 241; spikes S9 and S12): TypeSafe's System One
//! model scores each contestant on the shared rubric, one candidate per request. The call runs
//! in a throwaway evaluator sandbox as one fixed `curl` command, never an agent (decisions 112,
//! 120); the token reaches it only as a header the `sbx` proxy swaps in, from a custom secret
//! scoped to that sandbox (decision 36).

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::backend::{SandboxBackend, SecretSource};
use crate::commands::doctor;
use crate::run::config::{Criterion, CriterionKind, Jev};

/// The one host the evaluator sandbox can reach, and the one the secret is swapped for.
pub const HOST: &str = "api.typesafe.ai";
/// The env var holding the placeholder inside the evaluator sandbox.
pub const ENV: &str = "JEV_API_KEY";
/// The `sbx` that spike S12 verified `secret set-custom` on (it is experimental).
pub const MIN_SBX_VERSION: &str = "0.46.0";

/// What `sbxm run` checks for `[eval.jev]` before anything is written or created: the `sbx`
/// version, the helper's existence (not run: `sbx` verifies it at `set-custom`), and that no
/// global custom secret for the same env and host exists, which would make `sbx create` fail
/// with `400 invalid custom secrets` (S9). Read-only.
pub fn preflight(jev: &Jev, backend: &dyn SandboxBackend) -> Result<()> {
    let version = backend.version()?;
    if doctor::older(&version, MIN_SBX_VERSION) {
        bail!(
            "eval.jev needs sbx {MIN_SBX_VERSION} or newer (its custom secrets), and this is sbx \
             {version}; upgrade sbx or remove [eval.jev]"
        );
    }
    if let SecretSource::Command(helper) = &jev.token
        && !helper.is_file()
    {
        bail!(
            "eval.jev.token_command {} does not exist; create the helper that prints the token \
             there or fix the path",
            helper.display()
        );
    }
    let clash = backend
        .custom_secrets()?
        .into_iter()
        .find(|s| s.is_global() && s.env == ENV && s.targets.iter().any(|t| t == HOST));
    if let Some(clash) = clash {
        bail!(
            "a global custom secret for {ENV} on {HOST} exists, and it would make the Jev \
             evaluator's sandbox fail to start; remove it with `sbx secret rm --placeholder {} \
             -f`",
            clash.placeholder
        );
    }
    Ok(())
}

// ---- requests: mapping, budget, chunking --------------------------------------------------

/// Estimated tokens per request are kept at or under this (Jev's limit is 32k on the state plus
/// the longest question; S12 saw 31,958 pass).
pub const TOKEN_CAP: usize = 28_000;
/// Fixed tokens of every request, and the extra each question beyond the first adds (S12: about
/// 295, and 50 to 70).
const OVERHEAD_TOKENS: usize = 300;
const PER_EXTRA_QUESTION_TOKENS: usize = 70;
/// Characters per token: right for code, conservative for prose (S12).
const CHARS_PER_TOKEN: usize = 3;

/// The diff heading of a request whose diff was split, sized for the estimate before the number
/// of parts is known.
const WIDEST_PART: &str = "part 9999 of 9999";

/// One planned request of a pair: its `state`, and its weight when chunk results are combined
/// (its diff's size in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub state: String,
    pub weight: usize,
}

/// A pair's requests: one, or one per chunk of a split diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub questions: Value,
    pub requests: Vec<Planned>,
    /// A single file was bigger than the budget and was cut.
    pub truncated: bool,
}

/// The questions for `rubric`, one per criterion (decision 241): a `pass_fail` criterion is a
/// Noul, a `scale` criterion a Score over its levels in order. `instructions` carry the
/// criterion's id, its notes and the task.
pub fn questions(task: &str, rubric: &[Criterion]) -> Value {
    let mut all = serde_json::Map::new();
    for criterion in rubric {
        let subject = criterion.notes.as_deref().unwrap_or(&criterion.id);
        let instructions = format!(
            "Criterion `{}`{}. The state holds one candidate's final answer and the diff of its \
             changes, for the task below. Judge this criterion only.\n\nTask:\n{}",
            criterion.id,
            criterion
                .notes
                .as_deref()
                .map(|n| format!(": {n}"))
                .unwrap_or_default(),
            task.trim()
        );
        let question = match criterion.kind {
            CriterionKind::PassFail => json!({
                "type": "noul",
                "instructions": instructions,
                "criteria": {
                    "true": format!("Satisfied: {subject}"),
                    "false": format!("Not satisfied: {subject}"),
                },
            }),
            CriterionKind::Scale => json!({
                "type": "score",
                "instructions": instructions,
                "criteria": criterion.levels,
            }),
        };
        all.insert(criterion.id.clone(), question);
    }
    Value::Object(all)
}

/// The JSON body of one call; `model` is always sent, since the API requires it (S12).
pub fn request_body(model: &str, state: &str, questions: &Value) -> Value {
    json!({"model": model, "state": state, "questions": questions})
}

fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// The estimate for one request: the state plus the longest question, the fixed overhead and
/// the extra questions.
fn estimate(state: &str, questions: &Value) -> usize {
    let map = questions.as_object();
    let longest = map
        .into_iter()
        .flat_map(|m| m.values())
        .map(|q| tokens(&q.to_string()))
        .max()
        .unwrap_or(0);
    let extra = map.map_or(0, |m| m.len().saturating_sub(1));
    tokens(state) + longest + OVERHEAD_TOKENS + PER_EXTRA_QUESTION_TOKENS * extra
}

/// What Jev sees of a candidate: its final answer and (part of) its diff, never its
/// transcript (decision 20). The task is in each question's instructions.
fn state(answer: &str, diff: &str, part: Option<&str>) -> String {
    let answer = answer.trim_end();
    let answer = if answer.trim().is_empty() {
        "(no answer)"
    } else {
        answer
    };
    let heading = match part {
        Some(part) => format!("## Diff ({part}, split per file)"),
        None => "## Diff".to_owned(),
    };
    format!("## Final answer\n\n{answer}\n\n{heading}\n\n{diff}\n")
}

/// Plans one pair's requests within [`TOKEN_CAP`] (decision 241 d). `Err` names the part that is
/// too big on its own (`task` or `answer`): then no call is made. A diff that doesn't fit is
/// split per file, and a single file bigger than the budget is cut.
pub fn plan(
    task: &str,
    rubric: &[Criterion],
    answer: &str,
    diff: Option<&str>,
) -> Result<Plan, String> {
    let questions = questions(task, rubric);
    let bare = estimate("", &questions);
    if bare > TOKEN_CAP {
        return Err(format!(
            "the task is too long for Jev (about {bare} tokens with the questions, the limit is \
             {TOKEN_CAP}); no call was made"
        ));
    }
    let with_answer = estimate(&state(answer, "", Some(WIDEST_PART)), &questions);
    if with_answer > TOKEN_CAP {
        return Err(format!(
            "the answer is too long for Jev (about {with_answer} tokens with the task and the \
             questions, the limit is {TOKEN_CAP}); no call was made"
        ));
    }
    let fits = |text: &str, part: Option<&str>| {
        estimate(&state(answer, text, part), &questions) <= TOKEN_CAP
    };
    let diff = match diff {
        None => "(no diff available)",
        Some(d) if d.trim().is_empty() => "(no changes)",
        Some(d) => d,
    };
    if fits(diff, None) {
        let weight = diff.chars().count().max(1);
        return Ok(Plan {
            requests: vec![Planned {
                state: state(answer, diff, None),
                weight,
            }],
            questions,
            truncated: false,
        });
    }

    let part = Some(WIDEST_PART);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;
    for file in split_files(diff) {
        let joined = format!("{current}{file}");
        if fits(&joined, part) {
            current = joined;
            continue;
        }
        if !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if fits(file, part) {
            current = file.to_owned();
        } else {
            chunks.push(cut(file, &|text| fits(text, part)));
            truncated = true;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    let n = chunks.len();
    let requests = chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| Planned {
            state: state(answer, &chunk, Some(&format!("part {} of {n}", i + 1))),
            weight: chunk.chars().count().max(1),
        })
        .collect();
    Ok(Plan {
        questions,
        requests,
        truncated,
    })
}

/// A unified diff split before each `diff --git` line; anything before the first stays with it.
fn split_files(diff: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = diff
        .match_indices("diff --git ")
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || diff.as_bytes()[i - 1] == b'\n')
        .collect();
    if starts.first() != Some(&0) {
        if starts.is_empty() {
            return vec![diff];
        }
        starts[0] = 0;
    }
    starts.push(diff.len());
    starts.windows(2).map(|w| &diff[w[0]..w[1]]).collect()
}

/// The longest start of `file` that, with a note saying it was cut, still `fits`.
fn cut(file: &str, fits: &dyn Fn(&str) -> bool) -> String {
    let total = file.chars().count();
    let with_note = |keep: usize| {
        let kept: String = file.chars().take(keep).collect();
        format!(
            "{kept}\n[file truncated: {} more characters left out]\n",
            total - keep
        )
    };
    let (mut low, mut high) = (0, total);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(&with_note(mid)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    with_note(low)
}

// ---- responses -----------------------------------------------------------------------------

/// One criterion's answer, mapped to 0-1.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub score: f64,
    /// Score answers only; recorded and shown, not used in scoring.
    pub confidence: Option<f64>,
}

/// One response, or a pair's combined responses.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    /// The model that answered (`jev-1.13.0` for `jev-latest`), from the response.
    pub model: String,
    pub criteria: BTreeMap<String, Scored>,
    /// Criteria with no valid answer, in rubric order: never counted as 0.
    pub unscored: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Reads a 200 response. `Err` (one line, no response text) when it isn't the documented
/// shape; an answer that is missing or of the wrong type leaves its criterion unscored, and an
/// out-of-range value is clamped.
pub fn parse_response(text: &str, rubric: &[Criterion]) -> Result<Answered, String> {
    let malformed = |what: &str| format!("Jev's response is malformed: {what}");
    let root: Value = serde_json::from_str(text).map_err(|_| malformed("not JSON"))?;
    let model = root["model"]
        .as_str()
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| malformed("no model"))?
        .to_owned();
    let answers = root["answers"]
        .as_object()
        .ok_or_else(|| malformed("no answers object"))?;
    let mut criteria = BTreeMap::new();
    let mut unscored = Vec::new();
    for criterion in rubric {
        match answers
            .get(&criterion.id)
            .and_then(|a| scored(criterion, a))
        {
            Some(s) => {
                criteria.insert(criterion.id.clone(), s);
            }
            None => unscored.push(criterion.id.clone()),
        }
    }
    Ok(Answered {
        model,
        criteria,
        unscored,
        input_tokens: root["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: root["usage"]["output_tokens"].as_u64().unwrap_or(0),
    })
}

fn scored(criterion: &Criterion, answer: &Value) -> Option<Scored> {
    match criterion.kind {
        CriterionKind::PassFail => {
            if answer["type"] != "noul" {
                return None;
            }
            let noul = answer["noul"].as_f64()?;
            Some(Scored {
                score: noul.clamp(0.0, 1.0),
                confidence: None,
            })
        }
        CriterionKind::Scale => {
            if answer["type"] != "score" {
                return None;
            }
            let top = (criterion.levels.len() - 1) as f64;
            let score = answer["score"].as_f64()?;
            Some(Scored {
                score: (score / top).clamp(0.0, 1.0),
                confidence: answer["confidence"].as_f64().map(|c| c.clamp(0.0, 1.0)),
            })
        }
    }
}

/// A pair's chunk answers combined per criterion as the mean weighted by each chunk's size; a
/// criterion no chunk scored stays unscored. The model is the first chunk's (the caller has
/// checked they agree).
pub fn combine(chunks: &[(usize, Answered)], rubric: &[Criterion]) -> Answered {
    let mut criteria = BTreeMap::new();
    let mut unscored = Vec::new();
    for criterion in rubric {
        let scored: Vec<(f64, &Scored)> = chunks
            .iter()
            .filter_map(|(w, a)| Some((*w as f64, a.criteria.get(&criterion.id)?)))
            .collect();
        let weight: f64 = scored.iter().map(|(w, _)| w).sum();
        if scored.is_empty() || weight <= 0.0 {
            unscored.push(criterion.id.clone());
            continue;
        }
        let score = scored.iter().map(|(w, s)| w * s.score).sum::<f64>() / weight;
        let confident: Vec<(f64, f64)> = scored
            .iter()
            .filter_map(|(w, s)| Some((*w, s.confidence?)))
            .collect();
        let confidence_weight: f64 = confident.iter().map(|(w, _)| w).sum();
        let confidence = (!confident.is_empty() && confidence_weight > 0.0)
            .then(|| confident.iter().map(|(w, c)| w * c).sum::<f64>() / confidence_weight);
        criteria.insert(criterion.id.clone(), Scored { score, confidence });
    }
    Answered {
        model: chunks
            .first()
            .map(|(_, a)| a.model.clone())
            .unwrap_or_default(),
        criteria,
        unscored,
        input_tokens: chunks.iter().map(|(_, a)| a.input_tokens).sum(),
        output_tokens: chunks.iter().map(|(_, a)| a.output_tokens).sum(),
    }
}

// ---- HTTP status ---------------------------------------------------------------------------

/// What to do with one call's HTTP status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 200: the body is the response.
    Answered,
    /// 429 or 529: try again after a pause.
    Retry(u16),
    /// Anything else: a one-line reason that never repeats the raw body (a 422 echoes the
    /// whole request, decision 241).
    Failed(String),
}

/// Sorts a call's result by its HTTP status, keeping from an error body only `error_type` (and
/// `message` for 401 and a 400 `api_usage_error`), or each 422 entry's `loc` and `msg`.
pub fn classify(status: u16, body: &str, token: &SecretSource) -> Outcome {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .map(|v| v["detail"].clone())
        .unwrap_or(Value::Null);
    let error_type = detail["error_type"].as_str().map(one_line);
    let message = detail["message"].as_str().map(one_line);
    match status {
        200 => Outcome::Answered,
        429 | 529 => Outcome::Retry(status),
        401 => {
            let source = match token {
                SecretSource::Command(path) => format!("eval.jev.token_command {}", path.display()),
                SecretSource::Ref(_) => "eval.jev.token_ref".to_owned(),
            };
            Outcome::Failed(format!(
                "Jev rejected the token (401{}); check the token {source} gives",
                message.map(|m| format!(": {m}")).unwrap_or_default()
            ))
        }
        400 => {
            let reason = match (error_type.as_deref(), message) {
                (Some("api_usage_error"), Some(m)) => format!("api_usage_error: {m}"),
                (Some(t), _) => t.to_owned(),
                (None, _) => "no details".to_owned(),
            };
            Outcome::Failed(format!("Jev refused the request (400 {reason})"))
        }
        422 => {
            let entries: Vec<String> = detail
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| {
                    let loc: Vec<String> = entry["loc"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|part| match part {
                            Value::String(s) => one_line(s),
                            other => one_line(&other.to_string()),
                        })
                        .collect();
                    format!(
                        "{}: {}",
                        loc.join("."),
                        entry["msg"].as_str().map(one_line).unwrap_or_default()
                    )
                })
                .collect();
            let reason = if entries.is_empty() {
                "no details".to_owned()
            } else {
                entries.join("; ")
            };
            Outcome::Failed(format!("Jev refused the request (422 {reason})"))
        }
        other => Outcome::Failed(format!(
            "Jev answered HTTP {other}{}",
            error_type.map(|t| format!(" ({t})")).unwrap_or_default()
        )),
    }
}

/// Text from a response, made safe for a one-line message and kept short.
fn one_line(text: &str) -> String {
    const MAX: usize = 200;
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= MAX {
        flat.to_owned()
    } else {
        format!("{}...", flat.chars().take(MAX).collect::<String>())
    }
}
