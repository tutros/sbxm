//! The Jev evaluator (M2a slice 14a, decision 241; spikes S9 and S12): TypeSafe's System One
//! model scores each contestant on the shared rubric, one candidate per request. The call runs
//! in a throwaway evaluator sandbox as one fixed `curl` command, never an agent (decisions 112,
//! 120); the token reaches it only as a header the `sbx` proxy swaps in, from a custom secret
//! scoped to that sandbox (decision 36).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::backend::{CreateSpec, CustomSecretSpec, SandboxBackend, SecretSource, SkillsStore};
use crate::commands::doctor;
use crate::config::{Profile, Resources, sha256_hex};
use crate::headless::hit_timeout;
use crate::kit;
use crate::run::checks::timed_exec;
use crate::run::config::{Criterion, CriterionKind, Jev, RunConfig};
use crate::run::id::RunRoots;
use crate::run::orchestrate::{PairOutcome, in_sandbox_path};
use crate::task::repo::{AgentFile, read_file_in};

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

/// The diff heading of a split diff's part `i` of `n`.
fn part_label(i: usize, n: usize) -> String {
    format!("part {i} of {n}")
}

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
pub fn estimate(state: &str, questions: &Value) -> usize {
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
    layout(answer, diff, part)
}

/// The state's fixed layout around `answer` and `diff`, with no placeholder for an empty answer.
fn layout(answer: &str, diff: &str, part: Option<&str>) -> String {
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
    let task_too_long = |tokens: usize| {
        format!(
            "the task is too long for Jev (about {tokens} tokens with the questions, the limit is \
             {TOKEN_CAP}); no call was made"
        )
    };
    // The task alone is the state's fixed headings with nothing in them; no placeholder is
    // counted, since a short real answer can be shorter than one (PR 289 review round 2, M-1).
    let bare = estimate(&layout("", "", None), &questions);
    if bare > TOKEN_CAP {
        return Err(task_too_long(bare));
    }
    // The answer is checked with the diff heading of the request that is actually sent. An empty
    // answer's request holds only sbxm's own placeholder, so its excess is the task's (PR 289
    // review M-2).
    let answer_fits = |part: Option<&str>| {
        let with_answer = estimate(&state(answer, "", part), &questions);
        if with_answer > TOKEN_CAP {
            if answer.trim().is_empty() {
                return Err(task_too_long(with_answer));
            }
            return Err(format!(
                "the answer is too long for Jev (about {with_answer} tokens with the task and \
                 the questions, the limit is {TOKEN_CAP}); no call was made"
            ));
        }
        Ok(())
    };
    answer_fits(None)?;
    let fits = |text: &str, part: Option<&str>| {
        estimate(&state(answer, text, part), &questions) <= TOKEN_CAP
    };
    let marker = match diff {
        None => Some("(no diff available)"),
        Some(d) if d.trim().is_empty() => Some("(no changes)"),
        Some(_) => None,
    };
    if let Some(marker) = marker {
        // No diff to cut: a marker that doesn't fit is left out, which is not a truncation
        // (PR 289 review M-3).
        let text = if fits(marker, None) { marker } else { "" };
        return Ok(Plan {
            requests: vec![Planned {
                state: state(answer, text, None),
                weight: text.chars().count().max(1),
            }],
            questions,
            truncated: false,
        });
    }
    let diff = diff.unwrap_or_default();
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

    // Every chunk holds at least one file of its own, so a split has at most as many parts as
    // files: the chunks are sized against that count's label, which no real label is wider than.
    let files = split_files(diff);
    let widest = part_label(files.len(), files.len());
    if answer_fits(Some(&widest)).is_err() {
        // The answer fits alone but leaves no room for a split's longer heading: one unsplit
        // request with the diff cut, never an error (decision 241 d; review M-1, rounds 1-4).
        let chunk = cut(diff, &|text| fits(text, None));
        return Ok(Plan {
            requests: vec![Planned {
                state: state(answer, &chunk, None),
                weight: chunk.chars().count().max(1),
            }],
            questions,
            truncated: true,
        });
    }
    let part = Some(widest.as_str());
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;
    for file in files {
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
    let requests: Vec<Planned> = chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| Planned {
            state: state(answer, &chunk, Some(&part_label(i + 1, n))),
            weight: chunk.chars().count().max(1),
        })
        .collect();
    // The chunks were sized against `widest`; a request is only ever sent within the cap.
    if let Some(over) = requests
        .iter()
        .map(|r| estimate(&r.state, &questions))
        .find(|&e| e > TOKEN_CAP)
    {
        return Err(format!(
            "the diff splits into too many parts for Jev (one is about {over} tokens, the limit \
             is {TOKEN_CAP}); no call was made"
        ));
    }
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

/// The longest start of `file` that, with a note saying it was cut, still `fits`; nothing at all
/// when even the note alone doesn't (an empty diff always fits: `plan` checked the answer alone
/// first, and a split's heading before cutting under it).
fn cut(file: &str, fits: &dyn Fn(&str) -> bool) -> String {
    let total = file.chars().count();
    let with_note = |keep: usize| {
        let kept: String = file.chars().take(keep).collect();
        format!(
            "{kept}\n[file truncated: {} more characters left out]\n",
            total - keep
        )
    };
    if !fits(&with_note(0)) {
        return String::new();
    }
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
    // One chunk keeps Jev's numbers exactly, without a mean's rounding.
    if let [(_, only)] = chunks {
        return only.clone();
    }
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
                SecretSource::CommandLine(line) => format!("eval.jev.token_command {line}"),
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

// ---- the evaluator sandbox -----------------------------------------------------------------

/// The `sbx` agent of the evaluator sandbox: a plain shell, since no agent runs in it.
pub const AGENT: &str = "shell";
/// The folder of the evaluator's workspace that holds `request.json` and `response.json`.
const CALL_DIR: &str = "call";
const REQUEST_FILE: &str = "request.json";
const RESPONSE_FILE: &str = "response.json";
/// The largest `response.json` read back (a response is a few kilobytes).
const RESPONSE_CAP: u64 = 4 * 1024 * 1024;
/// How long one call may take inside the sandbox (S12: about a second).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The pauses before each retry of a 429 or 529; then it gives up.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// The one fixed command (decision 241): the header in double quotes, so the sandbox shell
/// expands `$JEV_API_KEY` to the placeholder the proxy swaps (S12). Its stdout is the status.
const CURL: &str = "curl -sS -X POST https://api.typesafe.ai/v1/systemone \
                    -H \"Authorization: Bearer $JEV_API_KEY\" \
                    -H \"Content-Type: application/json\" \
                    --data @request.json -o response.json -w \"%{http_code}\"";

/// The evaluator sandbox's own kit, from a built-in profile whose only egress is [`HOST`]
/// (never a contestant's profile): one `common` mixin, as the agent is a plain shell.
#[derive(Debug, Clone)]
pub struct EvaluatorKit {
    pub dirs: Vec<PathBuf>,
    /// The run's resources, as every sandbox of the run gets.
    pub cpus: u32,
    pub memory: String,
}

/// Writes the evaluator's kit under `kits_root/jev/` and validates it, before any sandbox exists.
pub fn build_kit(
    kits_root: &Path,
    resources: &Resources,
    backend: &dyn SandboxBackend,
) -> Result<EvaluatorKit> {
    let mut profile = Profile::default();
    profile.network.allow = vec![HOST.to_owned()];
    let hash = sha256_hex(format!("sbxm jev evaluator, egress {HOST}").as_bytes());
    let dir = kits_root.join("jev").join("common");
    kit::write(&dir, &kit::common("jev-evaluator", &profile, &hash))?;
    let validation = backend.validate_kit(&dir)?;
    if !validation.valid {
        bail!(
            "the Jev evaluator's generated kit {} is invalid: {}; report this as an sbxm bug",
            dir.display(),
            validation.error.as_deref().unwrap_or("no details from sbx")
        );
    }
    Ok(EvaluatorKit {
        dirs: vec![dir],
        cpus: resources.cpus,
        memory: resources.memory.clone(),
    })
}

/// A pair's Jev result: its combined answer, how many requests it took, and whether a file of
/// its diff was cut.
#[derive(Debug, Clone, PartialEq)]
pub struct PairScores {
    pub answered: Answered,
    pub chunks: usize,
    pub truncated: bool,
}

/// One repeat's evaluation: its sandbox and secret, and each pair's result.
#[derive(Debug)]
pub struct JevRun {
    pub repeat: u32,
    pub sandbox: String,
    pub placeholder: String,
    /// Contestant index and its result; `Err` is a one-line reason that never holds a token or
    /// a raw error body.
    pub results: Vec<(usize, Result<PairScores, String>)>,
    /// The evaluator sandbox couldn't be removed; the results are kept.
    pub remove_error: Option<String>,
    /// Its scoped secret couldn't be removed.
    pub secret_remove_error: Option<String>,
}

/// The evaluator sandbox of one repeat, and its secret's placeholder: both carry the run id and
/// the repeat, so a rerun or another run never meets them.
pub fn names(run_id: &str, repeat: u32) -> (String, String) {
    (
        format!("sbxm-run-{run_id}-jev-{repeat}"),
        format!("sbxm-jev-{run_id}-{repeat}"),
    )
}

/// Scores one repeat's pairs with Jev, one call per pair (plus one per extra chunk). `None` when
/// there is no `[eval.jev]` or no pair that ran. Never fails: problems are in the results. The
/// scoped secret is created before the sandbox and removed after it, on every path once it was
/// asked for (decision 241). `sleep` waits between retries.
pub fn evaluate_repeat(
    backend: &dyn SandboxBackend,
    run_config: &RunConfig,
    kit: &EvaluatorKit,
    roots: &RunRoots,
    repeat: u32,
    pairs: &[&PairOutcome],
    sleep: &dyn Fn(Duration),
) -> Option<JevRun> {
    let jev = run_config.eval.jev.as_ref()?;
    let scored: Vec<&&PairOutcome> = pairs.iter().filter(|p| p.result.is_ok()).collect();
    if scored.is_empty() {
        return None;
    }
    let (sandbox, placeholder) = names(&roots.id, repeat);
    let mut run = JevRun {
        repeat,
        sandbox: sandbox.clone(),
        placeholder: placeholder.clone(),
        results: Vec::new(),
        remove_error: None,
        secret_remove_error: None,
    };
    // Every plan first: a pair too big for Jev is an error without a call.
    let mut planned: Vec<(usize, Plan)> = Vec::new();
    for pair in &scored {
        let answer = pair
            .result
            .as_ref()
            .map(|r| r.answer.as_str())
            .unwrap_or_default();
        let diff = match &pair.diff {
            Some(Ok(diff)) => Some(diff.patch.as_str()),
            _ => None,
        };
        match plan(
            &run_config.task.prompt,
            &run_config.eval.rubric,
            answer,
            diff,
        ) {
            Ok(p) => planned.push((pair.contestant, p)),
            Err(why) => run.results.push((pair.contestant, Err(why))),
        }
    }
    if planned.is_empty() {
        return Some(run);
    }

    let workspace = roots.workspaces.join("jev").join(repeat.to_string());
    let calls = workspace.join(CALL_DIR);
    let mut secret_asked = false;
    let mut created = false;
    let prepared = (|| {
        std::fs::create_dir_all(&calls).map_err(|e| {
            format!(
                "cannot prepare the Jev workspace {}: {e}",
                workspace.display()
            )
        })?;
        secret_asked = true;
        backend
            .set_custom_secret(&CustomSecretSpec {
                sandbox: sandbox.clone(),
                hosts: vec![HOST.to_owned()],
                env: ENV.to_owned(),
                placeholder: placeholder.clone(),
                source: jev.token.clone(),
            })
            .map_err(|e| format!("cannot store the Jev token's scoped secret: {e:#}"))?;
        created = true;
        backend
            .create(&CreateSpec {
                name: sandbox.clone(),
                agent: AGENT.into(),
                workspace: Some(workspace.clone()),
                cpus: kit.cpus,
                memory: kit.memory.clone(),
                skills: SkillsStore::Off,
                kits: kit.dirs.clone(),
                template: None,
                deny_network: Vec::new(),
                ttl: None,
            })
            .map_err(|e| format!("cannot create the Jev sandbox {sandbox}: {e:#}"))
    })();
    match prepared {
        Err(why) => {
            for (contestant, _) in &planned {
                run.results.push((*contestant, Err(why.clone())));
            }
        }
        Ok(()) => {
            let caller = Caller {
                backend,
                sandbox: &sandbox,
                calls: &calls,
                token: &jev.token,
                sleep,
            };
            for (contestant, p) in &planned {
                let result = score_pair(&caller, &jev.model, p, &run_config.eval.rubric);
                run.results.push((*contestant, result));
            }
        }
    }
    // Always, once asked for: the sandbox, then its secret.
    if created {
        run.remove_error = backend
            .remove(&sandbox)
            .err()
            .map(|e| format!("cannot remove sandbox {sandbox}: {e:#}; run `sbx rm -f {sandbox}`"));
    }
    if secret_asked {
        run.secret_remove_error = backend
            .remove_custom_secret(&placeholder, &sandbox)
            .err()
            .map(|e| {
                format!(
                    "cannot remove the Jev secret {placeholder}: {e:#}; run `sbx secret rm \
                     --placeholder {placeholder} --sandbox {sandbox} -f`"
                )
            });
    }
    run.results.sort_by_key(|(contestant, _)| *contestant);
    Some(run)
}

/// What every call of one repeat shares.
struct Caller<'a> {
    backend: &'a dyn SandboxBackend,
    sandbox: &'a str,
    /// The host folder of `request.json` and `response.json`.
    calls: &'a Path,
    token: &'a SecretSource,
    sleep: &'a dyn Fn(Duration),
}

/// One pair's requests in order: the first names the configured model, every later one the model
/// the first response named, and a response from another model is an error (decision 241 e).
fn score_pair(
    caller: &Caller,
    model: &str,
    plan: &Plan,
    rubric: &[Criterion],
) -> Result<PairScores, String> {
    let n = plan.requests.len();
    let mut answers: Vec<(usize, Answered)> = Vec::new();
    for (i, request) in plan.requests.iter().enumerate() {
        let asked = answers.first().map_or(model, |(_, a)| a.model.as_str());
        let body = request_body(asked, &request.state, &plan.questions);
        let answered = call(caller, &body, rubric).map_err(|why| {
            if n > 1 {
                format!("chunk {} of {n}: {why}", i + 1)
            } else {
                why
            }
        })?;
        if let Some((_, first)) = answers.first()
            && answered.model != first.model
        {
            return Err(format!(
                "chunk {} of {n} was answered by {}, chunk 1 by {}; one pair's score must come \
                 from one model",
                i + 1,
                answered.model,
                first.model
            ));
        }
        answers.push((request.weight, answered));
    }
    Ok(PairScores {
        answered: combine(&answers, rubric),
        chunks: n,
        truncated: plan.truncated,
    })
}

/// Removes Jev's response file when dropped. An error body can echo the whole request (S12), so
/// it is read at most once and never kept, however a call attempt ends: a timeout, no usable
/// status, an oversized or unreadable file, or an answer (PR 289 review M-1).
struct ResponseGone<'a>(&'a Path);

impl Drop for ResponseGone<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// One request: written, sent by the fixed command, retried on 429 and 529 after each pause of
/// [`BACKOFF`], each retry a fresh exec in the same sandbox.
fn call(caller: &Caller, body: &Value, rubric: &[Criterion]) -> Result<Answered, String> {
    let request = caller.calls.join(REQUEST_FILE);
    let response = caller.calls.join(RESPONSE_FILE);
    let spec = timed_exec(&in_sandbox_path(caller.calls), CALL_TIMEOUT.as_secs(), CURL);
    let mut attempt = 0;
    loop {
        std::fs::write(&request, body.to_string())
            .map_err(|e| format!("cannot write {}: {e}", request.display()))?;
        // A response from an earlier call must never be read as this one's.
        match std::fs::remove_file(&response) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot clear {}: {e}", response.display())),
        }
        let _gone = ResponseGone(&response);
        let output = caller.backend.exec(caller.sandbox, &spec).map_err(|e| {
            format!(
                "cannot run the Jev call (exec) in {}: {e:#}",
                caller.sandbox
            )
        })?;
        if hit_timeout(output.exit_code, &output.stderr) {
            return Err(format!(
                "the Jev call timed out after {}s",
                CALL_TIMEOUT.as_secs()
            ));
        }
        // `sbx exec` may print its own lines first; the status is the last word.
        let status = output
            .stdout
            .split_whitespace()
            .last()
            .and_then(|w| w.parse::<u16>().ok())
            .filter(|&s| s != 0);
        let Some(status) = status else {
            let why = output
                .stderr
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .map(one_line)
                .unwrap_or_else(|| "no details".to_owned());
            return Err(format!(
                "the Jev call failed (exit code {}): {why}",
                output
                    .exit_code
                    .map_or_else(|| "none".to_owned(), |c| c.to_string())
            ));
        };
        let text = match read_file_in(
            caller.calls.parent().unwrap_or(caller.calls),
            CALL_DIR,
            RESPONSE_FILE,
            RESPONSE_CAP,
        ) {
            Ok(AgentFile::Text(text)) => Some(text),
            Ok(AgentFile::Missing) => None,
            Ok(AgentFile::TooLarge(n)) => {
                return Err(format!(
                    "Jev's response is too large ({n} bytes, the limit is {RESPONSE_CAP})"
                ));
            }
            Err(e) => return Err(format!("cannot read Jev's response: {e:#}")),
        };
        match classify(status, text.as_deref().unwrap_or(""), caller.token) {
            Outcome::Answered => {
                let text = text.ok_or_else(|| {
                    format!("Jev answered {status}, but no {RESPONSE_FILE} was written")
                })?;
                return parse_response(&text, rubric);
            }
            Outcome::Retry(code) => {
                let Some(pause) = BACKOFF.get(attempt) else {
                    return Err(format!(
                        "Jev answered {code} {} times; gave up after {} retries",
                        attempt + 1,
                        BACKOFF.len()
                    ));
                };
                (caller.sleep)(*pause);
                attempt += 1;
            }
            Outcome::Failed(why) => return Err(why),
        }
    }
}
