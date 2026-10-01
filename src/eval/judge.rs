//! The LLM judge (P3, decisions 18, 19, 21): blind labels, the prompt, parsing
//! the reply, and [`judge_repeat`], which runs the judge like a contestant,
//! through the shared headless primitive in a throwaway sandbox.

use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::rubric;
use crate::backend::{CreateSpec, SandboxBackend};
use crate::headless::{self, HeadlessOpts, RunStatus};
use crate::run::config::{Criterion, CriterionKind, RunConfig};
use crate::run::id::RunRoots;
use crate::run::kits::RunKits;
use crate::run::orchestrate::{PairOutcome, in_sandbox_path};

/// The file in the judge's workspace that holds its instructions. The prompt
/// carries whole answers and diffs, which can be longer than a command line
/// may be, so only a one-line pointer to this file is passed as the prompt.
const PROMPT_FILE: &str = "judge-prompt.md";

/// Longest answer and diff sent to the judge, in bytes; more is cut with a note.
const ANSWER_CAP: usize = 20 * 1024;
const DIFF_CAP: usize = 60 * 1024;

/// One contestant's work, under its blind label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: char,
    pub answer: String,
    /// `None` when no diff could be captured.
    pub diff: Option<String>,
}

/// Gives `contestants` the labels A, B, C, ... in an order that depends on
/// the run, the repeat and each index, and on nothing else: reproducible, but
/// no contestant is always first. Returned in label order.
pub fn assign_labels(run_id: &str, repeat: u32, contestants: &[usize]) -> Vec<(usize, char)> {
    let mut keyed: Vec<([u8; 32], usize)> = contestants
        .iter()
        .map(|&c| {
            let digest = Sha256::digest(format!("{run_id}:{repeat}:{c}").as_bytes());
            (digest.into(), c)
        })
        .collect();
    keyed.sort();
    keyed
        .into_iter()
        .enumerate()
        .map(|(position, (_, contestant))| (contestant, (b'A' + position as u8) as char))
        .collect()
}

/// The judge's instructions: the task, the rubric and every candidate. Nothing
/// that identifies a contestant (harness, model, index, status) goes in.
pub fn build_prompt(task: &str, rubric: &[Criterion], candidates: &[Candidate]) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are an impartial judge. Several candidates solved the same task, and you see \
         their work under anonymous labels. Judge each candidate on its own merits; do not \
         guess who wrote it.\n\n## Task\n\n",
    );
    prompt.push_str(task.trim());
    prompt.push_str("\n\n## Rubric\n\nScore every candidate on every criterion:\n\n");
    for criterion in rubric {
        prompt.push_str(&format!(
            "- `{}` ({}",
            criterion.id,
            kind_name(criterion.kind)
        ));
        if criterion.kind == CriterionKind::Scale {
            prompt.push_str(&format!(
                "; levels from worst to best: {}",
                criterion.levels.join(", ")
            ));
        }
        prompt.push(')');
        if let Some(notes) = &criterion.notes {
            prompt.push_str(&format!(": {notes}"));
        }
        match criterion.kind {
            CriterionKind::PassFail => prompt.push_str(" Answer true if it passes, else false."),
            CriterionKind::Scale => prompt.push_str(" Answer with exactly one of the levels."),
        }
        prompt.push('\n');
    }
    prompt.push_str("\n## Candidates\n");
    for candidate in candidates {
        prompt.push_str(&format!(
            "\n### Candidate {}\n\n#### Answer\n\n",
            candidate.label
        ));
        prompt.push_str(&section(
            &candidate.answer,
            ANSWER_CAP,
            "answer",
            "(no answer)",
        ));
        prompt.push_str("\n#### Diff\n\n");
        match &candidate.diff {
            None => prompt.push_str("(no diff available)\n"),
            Some(diff) if diff.trim().is_empty() => prompt.push_str("(no changes)\n"),
            Some(diff) => {
                prompt.push_str("```diff\n");
                prompt.push_str(&section(diff, DIFF_CAP, "diff", ""));
                prompt.push_str("```\n");
            }
        }
    }
    prompt.push_str(
        "\n## Reply\n\nReply with only one JSON object and no other text. Its keys are the \
         candidate labels; each holds one entry per criterion id with your `value` and a short \
         `reason`. For example:\n\n```json\n",
    );
    prompt.push_str(&example_reply(rubric, candidates));
    prompt.push_str("\n```\n");
    prompt
}

fn kind_name(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::PassFail => "pass_fail",
        CriterionKind::Scale => "scale",
    }
}

/// `text`, cut to `cap` bytes with a note saying how much was left out.
fn section(text: &str, cap: usize, what: &str, empty: &str) -> String {
    let text = text.trim_end();
    if text.is_empty() {
        return format!("{empty}\n");
    }
    if text.len() <= cap {
        return format!("{text}\n");
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[{what} truncated: {} more bytes omitted]\n",
        &text[..end],
        text.len() - end
    )
}

fn example_reply(rubric: &[Criterion], candidates: &[Candidate]) -> String {
    let mut root = serde_json::Map::new();
    for candidate in candidates {
        let mut entries = serde_json::Map::new();
        for criterion in rubric {
            let value = match criterion.kind {
                CriterionKind::PassFail => Value::Bool(true),
                CriterionKind::Scale => {
                    Value::String(criterion.levels.last().cloned().unwrap_or_default())
                }
            };
            entries.insert(
                criterion.id.clone(),
                serde_json::json!({"value": value, "reason": "one short sentence"}),
            );
        }
        root.insert(candidate.label.to_string(), Value::Object(entries));
    }
    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

/// One judge call: the sandbox, who was who, and what came of it.
#[derive(Debug)]
pub struct JudgeRun {
    pub repeat: u32,
    pub sandbox: String,
    /// Contestant index to blind label, in label order.
    pub labels: Vec<(usize, char)>,
    /// `Err` is a one-line reason: the sandbox, the judge's command or its
    /// reply failed. A criterion the judge skipped is not an error but `unscored`.
    pub result: Result<Parsed, String>,
    /// The judge's final reply, if it ran.
    pub answer: Option<String>,
    /// The judge's raw output, if it ran.
    pub transcript: Option<String>,
    /// The judge sandbox couldn't be removed; the result is kept.
    pub remove_error: Option<String>,
}

/// Judges one repeat index: the pairs in `pairs` (all of that index) that
/// produced output are compared together under blind labels, so answers from
/// different indices are never mixed (P9). `None` when there is no judge, no
/// rubric or nothing to judge. Never fails: problems are in the result.
pub fn judge_repeat(
    backend: &dyn SandboxBackend,
    run_config: &RunConfig,
    kits: &RunKits,
    roots: &RunRoots,
    repeat: u32,
    pairs: &[&PairOutcome],
) -> Option<JudgeRun> {
    let judge = run_config.eval.judge.as_ref()?;
    if run_config.eval.rubric.is_empty() {
        return None;
    }
    // A pair that couldn't run has no answer; a timed-out or failed one is
    // judged on whatever it left (decision 16), its status not shown.
    let judged: Vec<&&PairOutcome> = pairs.iter().filter(|p| p.result.is_ok()).collect();
    if judged.is_empty() {
        return None;
    }
    let contestants: Vec<usize> = judged.iter().map(|p| p.contestant).collect();
    let labels = assign_labels(&roots.id, repeat, &contestants);
    let candidates: Vec<Candidate> = labels
        .iter()
        .map(|&(contestant, label)| {
            let pair = judged.iter().find(|p| p.contestant == contestant).unwrap();
            Candidate {
                label,
                answer: pair
                    .result
                    .as_ref()
                    .map(|r| r.answer.clone())
                    .unwrap_or_default(),
                diff: match &pair.diff {
                    Some(Ok(diff)) => Some(diff.patch.clone()),
                    _ => None,
                },
            }
        })
        .collect();

    let sandbox = format!("sbxm-run-{}-judge-{repeat}", roots.id);
    let workspace = roots.workspaces.join("judge").join(repeat.to_string());
    let mut run = JudgeRun {
        repeat,
        sandbox: sandbox.clone(),
        labels: labels.clone(),
        result: Err(String::new()),
        answer: None,
        transcript: None,
        remove_error: None,
    };
    let mut created = false;
    run.result = (|| {
        std::fs::create_dir_all(&workspace)
            .and_then(|()| {
                let prompt = build_prompt(
                    &run_config.task.prompt,
                    &run_config.eval.rubric,
                    &candidates,
                );
                std::fs::write(workspace.join(PROMPT_FILE), prompt)
            })
            .map_err(|e| {
                format!(
                    "cannot prepare the judge workspace {}: {e}",
                    workspace.display()
                )
            })?;
        let harness_kits = kits
            .get(judge.harness)
            .ok_or_else(|| format!("no kits were built for {}", judge.harness.as_str()))?;
        created = true;
        backend
            .create(&CreateSpec {
                name: sandbox.clone(),
                agent: judge.harness.agent_arg().into(),
                workspace: workspace.clone(),
                cpus: kits.resources.cpus,
                memory: kits.resources.memory.clone(),
                skills: harness_kits.skills_store,
                kits: harness_kits.dirs.clone(),
            })
            .map_err(|e| format!("cannot create the judge sandbox {sandbox}: {e:#}"))?;
        let opts = HeadlessOpts {
            model: Some(judge.model.clone()),
            high_effort: false,
            budget_usd: None,
            is_git_repo: false,
        };
        let reply = headless::run(
            backend,
            &sandbox,
            &in_sandbox_path(&workspace),
            judge.harness,
            &format!(
                "Read the file {PROMPT_FILE} in the current directory and follow it exactly. \
                 Your final reply must be only the JSON object it asks for."
            ),
            &opts,
            run_config.run.timeout,
        )
        .map_err(|e| format!("cannot run the judge (exec) in {sandbox}: {e:#}"))?;
        run.answer = Some(reply.answer.clone());
        run.transcript = Some(reply.transcript.clone());
        match reply.status {
            RunStatus::Completed => {}
            RunStatus::TimedOut => {
                return Err(format!(
                    "the judge timed out after {}s",
                    run_config.run.timeout.as_secs()
                ));
            }
            RunStatus::Failed(why) => return Err(format!("the judge failed: {why}")),
        }
        parse_response(
            &reply.answer,
            &run_config.eval.rubric,
            &labels.iter().map(|&(_, l)| l).collect::<Vec<_>>(),
        )
    })();
    // Always, once create was asked for, whatever became of the judge.
    if created {
        run.remove_error = backend
            .remove(&sandbox)
            .err()
            .map(|e| format!("cannot remove sandbox {sandbox}: {e:#}; run `sbx rm -f {sandbox}`"));
    }
    Some(run)
}

/// What the judge said about every candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub candidates: BTreeMap<char, CandidateScores>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CandidateScores {
    /// The criteria the judge scored validly, by id.
    pub criteria: BTreeMap<String, Scored>,
    /// Criteria it didn't score or answered invalidly, in rubric order:
    /// excluded from ranking and listed, never counted as 0 (P9).
    pub unscored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// `true`/`false`, or the level's name.
    pub value: Value,
    /// 0 to 1, from [`rubric::score`].
    pub score: f64,
    pub reason: String,
}

/// Reads the judge's reply. Fails only when there is no JSON object at all;
/// a candidate or criterion the judge left out or got wrong is `unscored`.
pub fn parse_response(text: &str, rubric: &[Criterion], labels: &[char]) -> Result<Parsed, String> {
    let root = extract_object(text).ok_or_else(|| {
        let seen: String = text.trim().chars().take(80).collect();
        format!("the judge's reply has no JSON object (it began: {seen:?})")
    })?;
    let mut candidates = BTreeMap::new();
    for &label in labels {
        let entry = root
            .get(label.to_string().as_str())
            .and_then(Value::as_object);
        let mut scores = CandidateScores::default();
        for criterion in rubric {
            let raw = entry.and_then(|e| e.get(&criterion.id));
            match raw.and_then(|raw| scored(criterion, raw)) {
                Some(scored) => {
                    scores.criteria.insert(criterion.id.clone(), scored);
                }
                None => scores.unscored.push(criterion.id.clone()),
            }
        }
        candidates.insert(label, scores);
    }
    Ok(Parsed { candidates })
}

/// `{"value": v, "reason": r}` or a bare `v`; `None` if `v` isn't valid for the criterion.
fn scored(criterion: &Criterion, raw: &Value) -> Option<Scored> {
    let (value, reason) = match raw.as_object() {
        Some(object) => (
            object.get("value")?,
            object.get("reason").and_then(Value::as_str).unwrap_or(""),
        ),
        None => (raw, ""),
    };
    // A level is matched loosely but stored as the rubric spells it.
    let value = match (criterion.kind, value.as_str()) {
        (CriterionKind::Scale, Some(text)) => {
            Value::String(criterion.levels[rubric::level_index(criterion, text)?].clone())
        }
        _ => value.clone(),
    };
    let score = rubric::score(criterion, &value)?;
    Some(Scored {
        value,
        score,
        reason: reason.to_owned(),
    })
}

/// The first JSON object in `text`: the whole text, a fenced block, or
/// whatever sits between the first `{` and the last `}`.
fn extract_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    let text = text.trim();
    let mut attempts = vec![text.to_owned()];
    if let Some(fence) = text.find("```") {
        let rest = &text[fence + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            attempts.push(rest[..end].trim().to_owned());
        }
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
    {
        attempts.push(text[start..=end].to_owned());
    }
    attempts
        .iter()
        .find_map(|attempt| match serde_json::from_str(attempt) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        })
}
