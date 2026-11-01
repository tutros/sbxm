//! The run-config file behind `sbxm run` (decisions 15, 103, 107, 109, 115):
//! a standalone TOML file, parsed and checked without touching the disk or
//! `sbx`. Every struct denies unknown keys, as in `config.rs` (decision 11).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::Deserialize;

use crate::backend::SecretSource;
use crate::harness::Harness;

const CONTESTANT_MIN: usize = 2;
const CONTESTANT_MAX: usize = 4;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Where the file was read from, for messages.
    pub path: PathBuf,
    pub task: Task,
    pub run: RunLimits,
    pub contestants: Vec<Contestant>,
    pub eval: EvalConfig,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub prompt: String,
    /// Absolute or resolved against the run-config's folder.
    pub seed: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RunLimits {
    pub profile: Option<String>,
    pub timeout: Duration,
    pub budget_usd: Option<f64>,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    pub repeat: u32,
}

#[derive(Debug, Clone)]
pub struct Contestant {
    pub harness: Harness,
    pub model: String,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EvalConfig {
    pub checks: Vec<Check>,
    pub rubric: Vec<Criterion>,
    pub judge: Option<Judge>,
    pub cosine: Option<Cosine>,
    pub jev: Option<Jev>,
}

/// `[eval.jev]` (decision 241): where `sbx` reads the token from, never the token itself
/// (decision 36), and the model every request names (the API requires one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jev {
    pub token: SecretSource,
    pub model: String,
    /// `[[eval.jev.questions]]` (slice 14b, issue 343): raw questions asked next to the rubric,
    /// shown and never ranked.
    pub questions: Vec<JevQuestion>,
}

/// A raw question's Jev primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Yes or no; Jev gives P(yes).
    Noul,
    /// One of 2 to 255 options, with a distribution.
    Choice,
    /// One of 2 to 10 ordered levels.
    Score,
}

/// Jev's limits on a Choice's options and a Score's levels.
pub const CHOICE_OPTIONS: std::ops::RangeInclusive<usize> = 2..=255;
pub const SCORE_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;

/// One `[[eval.jev.questions]]` entry (issue 343).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevQuestion {
    pub id: String,
    pub kind: QuestionKind,
    pub question: String,
    /// Choice only: option id -> description (S12: Choice's `criteria` is this object).
    pub options: BTreeMap<String, String>,
    /// Score only: worst to best.
    pub levels: Vec<String>,
}

/// The model a request names when `[eval.jev]` sets none.
pub const JEV_DEFAULT_MODEL: &str = "jev-latest";

#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub command: String,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKind {
    PassFail,
    Scale,
}

#[derive(Debug, Clone)]
pub struct Criterion {
    pub id: String,
    pub kind: CriterionKind,
    /// Ordered worst to best; only for `Scale`.
    pub levels: Vec<String>,
    pub weight: f64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Judge {
    pub harness: Harness,
    pub model: String,
}

/// `[eval.cosine]` (decision 166): `model_dir` is resolved against the
/// run-config's folder, like `task.seed`; `None` means "use the default",
/// which only [`crate::run::preflight`] and the runner can resolve (they
/// know the sbxm config dir, which this module never touches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cosine {
    pub model_dir: Option<PathBuf>,
}

impl Cosine {
    /// `model_dir`, or `<config_dir>/models/all-minilm-l6-v2` (decision 166).
    pub fn resolve_model_dir(&self, config_dir: &Path) -> PathBuf {
        self.model_dir
            .clone()
            .unwrap_or_else(|| config_dir.join("models").join("all-minilm-l6-v2"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    task: RawTask,
    #[serde(default)]
    run: RawRun,
    #[serde(default)]
    contestants: Vec<RawContestant>,
    #[serde(default)]
    eval: RawEval,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTask {
    prompt: String,
    seed: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRun {
    profile: Option<String>,
    timeout: Option<String>,
    budget_usd: Option<f64>,
    cpus: Option<u32>,
    memory: Option<String>,
    repeat: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContestant {
    harness: String,
    model: Option<String>,
    profile: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawEval {
    #[serde(default)]
    checks: Vec<RawCheck>,
    #[serde(default)]
    rubric: Vec<RawCriterion>,
    judge: Option<RawJudge>,
    cosine: Option<RawCosine>,
    jev: Option<RawJev>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJev {
    token_command: Option<PathBuf>,
    token_ref: Option<String>,
    model: Option<String>,
    #[serde(default)]
    questions: Vec<RawJevQuestion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJevQuestion {
    id: String,
    kind: QuestionKind,
    question: String,
    options: Option<BTreeMap<String, String>>,
    levels: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    id: String,
    command: String,
    timeout: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCriterion {
    id: String,
    kind: CriterionKind,
    #[serde(default)]
    levels: Vec<String>,
    weight: f64,
    notes: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJudge {
    harness: String,
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCosine {
    model_dir: Option<PathBuf>,
}

impl RunConfig {
    /// Reads and validates `path`. Nothing else is touched: the seed, the
    /// profiles and the secrets are checked by [`crate::run::preflight`].
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "cannot read run-config {}; create one with `sbxm run init {}` or fix the path",
                path.display(),
                path.display()
            )
        })?;
        let raw: RawConfig = toml::from_str(&text).map_err(|e| {
            let line = e
                .span()
                .map(|s| format!(" (line {})", text[..s.start].matches('\n').count() + 1))
                .unwrap_or_default();
            anyhow::anyhow!(
                "run-config {}{line}: {}",
                path.display(),
                e.message().replace('\n', " ")
            )
        })?;
        Validator { path }.validate(raw)
    }
}

struct Validator<'a> {
    path: &'a Path,
}

impl Validator<'_> {
    /// `run-config <file>: <problem>; <fix>`, on one line (decision 48).
    fn err(&self, problem: impl std::fmt::Display) -> anyhow::Error {
        anyhow::anyhow!("run-config {}: {problem}", self.path.display())
    }

    fn validate(&self, raw: RawConfig) -> Result<RunConfig> {
        if raw.task.prompt.trim().is_empty() {
            return Err(self.err("task.prompt is empty; say what the contestants should do"));
        }
        let run = self.run_limits(raw.run)?;

        let n = raw.contestants.len();
        if !(CONTESTANT_MIN..=CONTESTANT_MAX).contains(&n) {
            return Err(self.err(format!(
                "has {n} [[contestants]] entries; a comparison needs {CONTESTANT_MIN} to \
                 {CONTESTANT_MAX}, add or remove entries"
            )));
        }
        let mut contestants = Vec::new();
        for (i, c) in raw.contestants.into_iter().enumerate() {
            let at = format!("contestants[{i}]");
            let harness = self.harness(&format!("{at}.harness"), &c.harness)?;
            let model = self.model(&format!("{at}.model"), harness, c.model)?;
            contestants.push(Contestant {
                harness,
                model,
                profile: c.profile,
            });
        }

        let eval = self.eval(raw.eval)?;
        let seed = raw.task.seed.map(|seed| {
            self.path
                .parent()
                .map_or_else(|| seed.clone(), |dir| dir.join(&seed))
        });
        Ok(RunConfig {
            path: self.path.to_path_buf(),
            task: Task {
                prompt: raw.task.prompt,
                seed,
            },
            run,
            contestants,
            eval,
        })
    }

    fn run_limits(&self, raw: RawRun) -> Result<RunLimits> {
        let repeat = raw.repeat.unwrap_or(1);
        if repeat < 1 {
            return Err(self.err("run.repeat is 0; it must be at least 1"));
        }
        let timeout = match &raw.timeout {
            Some(text) => self.duration("run.timeout", text)?,
            None => DEFAULT_TIMEOUT,
        };
        if let Some(budget) = raw.budget_usd
            && !(budget.is_finite() && budget > 0.0)
        {
            return Err(self.err(format!(
                "run.budget_usd is {budget}; it must be a positive number of dollars"
            )));
        }
        if raw.cpus == Some(0) {
            return Err(self.err("run.cpus is 0; it must be at least 1"));
        }
        Ok(RunLimits {
            profile: raw.profile,
            timeout,
            budget_usd: raw.budget_usd,
            cpus: raw.cpus,
            memory: raw.memory,
            repeat,
        })
    }

    fn eval(&self, raw: RawEval) -> Result<EvalConfig> {
        let mut seen = HashSet::new();
        let mut checks = Vec::new();
        for (i, c) in raw.checks.into_iter().enumerate() {
            let at = format!("eval.checks[{i}]");
            self.id(&at, &c.id, &mut seen)?;
            if c.command.trim().is_empty() {
                return Err(self.err(format!("{at}.command is empty; give a shell command")));
            }
            let timeout = c
                .timeout
                .as_deref()
                .map(|t| self.duration(&format!("{at}.timeout"), t))
                .transpose()?;
            checks.push(Check {
                id: c.id,
                command: c.command,
                timeout,
            });
        }

        let mut seen = HashSet::new();
        let mut rubric = Vec::new();
        for (i, c) in raw.rubric.into_iter().enumerate() {
            let at = format!("eval.rubric[{i}]");
            self.id(&at, &c.id, &mut seen)?;
            if !(c.weight.is_finite() && c.weight > 0.0) {
                return Err(self.err(format!(
                    "{at}.weight is {}; it must be a finite number above 0",
                    c.weight
                )));
            }
            match c.kind {
                CriterionKind::Scale => {
                    let distinct: HashSet<&String> = c.levels.iter().collect();
                    if distinct.len() < 2 {
                        return Err(self.err(format!(
                            "{at} is a scale with {} distinct levels; give at least 2 distinct \
                             levels, worst to best",
                            distinct.len()
                        )));
                    }
                }
                CriterionKind::PassFail if !c.levels.is_empty() => {
                    return Err(self.err(format!(
                        "{at}.levels is set on a pass_fail criterion; remove levels or use \
                         kind = \"scale\""
                    )));
                }
                CriterionKind::PassFail => {}
            }
            rubric.push(Criterion {
                id: c.id,
                kind: c.kind,
                levels: c.levels,
                weight: c.weight,
                notes: c.notes,
            });
        }

        let judge = match raw.judge {
            Some(j) => {
                let harness = self.harness("eval.judge.harness", &j.harness)?;
                let model = self.model("eval.judge.model", harness, j.model)?;
                if rubric.is_empty() {
                    return Err(self.err(
                        "eval.judge needs at least one [[eval.rubric]] criterion to score; add \
                         one or remove [eval.judge]",
                    ));
                }
                Some(Judge { harness, model })
            }
            None => None,
        };
        let cosine = raw.cosine.map(|c| Cosine {
            model_dir: c.model_dir.map(|dir| {
                self.path
                    .parent()
                    .map_or_else(|| dir.clone(), |base| base.join(&dir))
            }),
        });
        let jev = raw.jev.map(|j| self.jev(j, &rubric)).transpose()?;
        Ok(EvalConfig {
            checks,
            rubric,
            judge,
            cosine,
            jev,
        })
    }

    /// Exactly one token source, named and never read here (decision 241).
    fn jev(&self, raw: RawJev, rubric: &[Criterion]) -> Result<Jev> {
        let token = match (raw.token_command, raw.token_ref) {
            (Some(_), Some(_)) => {
                return Err(self
                    .err("eval.jev sets both token_command and token_ref; keep only one of them"));
            }
            (None, None) => {
                return Err(self.err(
                    "eval.jev has no token source; set token_command (an absolute path to a \
                     helper that prints the token) or token_ref (op://... or an ARN)",
                ));
            }
            (Some(command), None) => {
                if !command.is_absolute() {
                    return Err(self.err(format!(
                        "eval.jev.token_command {} is not an absolute path; sbx needs the \
                         helper's absolute path",
                        command.display()
                    )));
                }
                SecretSource::Command(command)
            }
            (None, Some(reference)) => {
                if !(reference.starts_with("op://") || reference.starts_with("arn:")) {
                    // The text itself is never repeated: it may be a token put there by mistake.
                    return Err(self.err(
                        "eval.jev.token_ref is not a secret reference; give an op://... \
                         reference or an ARN, never the token itself",
                    ));
                }
                SecretSource::Ref(reference)
            }
        };
        let model = raw.model.unwrap_or_else(|| JEV_DEFAULT_MODEL.to_owned());
        if model.trim().is_empty() {
            return Err(self.err(format!(
                "eval.jev.model is empty; name a Jev model or leave it out for {JEV_DEFAULT_MODEL}"
            )));
        }
        if rubric.is_empty() {
            return Err(self.err(
                "eval.jev needs at least one [[eval.rubric]] criterion to score; add one or \
                 remove [eval.jev]",
            ));
        }
        let mut seen = HashSet::new();
        let questions = raw
            .questions
            .into_iter()
            .enumerate()
            .map(|(i, q)| self.jev_question(i, q, rubric, &mut seen))
            .collect::<Result<_>>()?;
        Ok(Jev {
            token,
            model,
            questions,
        })
    }

    /// One raw question (issue 343): `options` only on a choice, `levels` only on a score, each
    /// within Jev's limits; its id unique among the questions and no rubric criterion's, since
    /// both are keys of one request's `questions`.
    fn jev_question(
        &self,
        i: usize,
        raw: RawJevQuestion,
        rubric: &[Criterion],
        seen: &mut HashSet<String>,
    ) -> Result<JevQuestion> {
        let at = format!("eval.jev.questions[{i}]");
        self.id(&at, &raw.id, seen)?;
        let at = format!("{at} (\"{}\")", raw.id);
        if rubric.iter().any(|c| c.id == raw.id) {
            return Err(self.err(format!(
                "{at} has the id of a rubric criterion; Jev answers both in one request, so \
                 give the question another id"
            )));
        }
        if raw.question.trim().is_empty() {
            return Err(self.err(format!(
                "{at}.question is empty; say what Jev should answer"
            )));
        }
        for (key, given, wanted) in [
            (
                "options",
                raw.options.is_some(),
                raw.kind == QuestionKind::Choice,
            ),
            (
                "levels",
                raw.levels.is_some(),
                raw.kind == QuestionKind::Score,
            ),
        ] {
            if given && !wanted {
                return Err(self.err(format!(
                    "{at}.{key} is set on a {} question; remove {key} or change its kind",
                    kind_name(raw.kind)
                )));
            }
            if wanted && !given {
                return Err(self.err(format!(
                    "{at} is a {} question without {key}; add {key}",
                    kind_name(raw.kind)
                )));
            }
        }
        let options = raw.options.unwrap_or_default();
        if raw.kind == QuestionKind::Choice {
            if !CHOICE_OPTIONS.contains(&options.len()) {
                return Err(self.err(format!(
                    "{at} is a choice with {} options; give {} to {}",
                    options.len(),
                    CHOICE_OPTIONS.start(),
                    CHOICE_OPTIONS.end()
                )));
            }
            if options
                .iter()
                .any(|(id, text)| id.trim().is_empty() || text.trim().is_empty())
            {
                return Err(self.err(format!(
                    "{at}.options has an empty option id or description; give every option \
                     both"
                )));
            }
        }
        let levels = raw.levels.unwrap_or_default();
        if raw.kind == QuestionKind::Score {
            let distinct = levels.iter().collect::<HashSet<_>>().len();
            if !SCORE_LEVELS.contains(&distinct) || distinct != levels.len() {
                return Err(self.err(format!(
                    "{at} is a score with {distinct} distinct levels{}; give {} to {} distinct \
                     levels, worst to best",
                    if distinct == levels.len() {
                        ""
                    } else {
                        " and repeats"
                    },
                    SCORE_LEVELS.start(),
                    SCORE_LEVELS.end()
                )));
            }
        }
        Ok(JevQuestion {
            id: raw.id,
            kind: raw.kind,
            question: raw.question,
            options,
            levels,
        })
    }

    /// A non-empty id not seen before in its list.
    fn id(&self, at: &str, id: &str, seen: &mut HashSet<String>) -> Result<()> {
        if id.trim().is_empty() {
            bail!(
                "run-config {}: {at}.id is empty; give it a short unique name",
                self.path.display()
            );
        }
        if !seen.insert(id.to_owned()) {
            bail!(
                "run-config {}: duplicate id \"{id}\" at {at}; ids must be unique within their list",
                self.path.display()
            );
        }
        Ok(())
    }

    /// A contestant's or the judge's `model`: an empty one is refused, and an omitted one is the
    /// harness's [`Harness::default_model`] (issue 314), refused for a harness that has none.
    fn model(&self, at: &str, harness: Harness, model: Option<String>) -> Result<String> {
        match model {
            Some(model) if model.trim().is_empty() => Err(self.err(format!(
                "{at} is empty; name a model for {}",
                harness.as_str()
            ))),
            Some(model) => Ok(model),
            None => harness.model_or_default(None).ok_or_else(|| {
                self.err(format!(
                    "{at} is missing; name a model for {}",
                    harness.as_str()
                ))
            }),
        }
    }

    fn harness(&self, at: &str, name: &str) -> Result<Harness> {
        headless_harness(at, name, "comparisons").map_err(|problem| self.err(problem))
    }

    /// `<n>s`, `<n>m` or `<n>h`, above zero.
    fn duration(&self, at: &str, text: &str) -> Result<Duration> {
        parse_duration(at, text).map_err(|problem| self.err(problem))
    }
}

/// Only claude, codex and antigravity have headless adapters (decision 103).
/// `what` names the feature in the refusal (`comparisons`, `tasks`). The error
/// is the problem text, `<at> ...; <fix>`, for the caller to prefix.
pub(crate) fn headless_harness(
    at: &str,
    name: &str,
    what: &str,
) -> std::result::Result<Harness, String> {
    let allowed = "use claude, codex or antigravity";
    match Harness::from_str(name, false) {
        Ok(h @ (Harness::Claude | Harness::Codex | Harness::Antigravity)) => Ok(h),
        Ok(Harness::Gemini) => Err(format!(
            "{at} \"gemini\" can't run {what}: Gemini CLI is deprecated upstream and \
             blocked by egress on this setup; use antigravity for Google models"
        )),
        Ok(Harness::Pi) => Err(format!(
            "{at} \"pi\" can't run {what} yet: its headless mode is deferred; {allowed}"
        )),
        Err(_) => Err(format!("{at} \"{name}\" is not a harness; {allowed}")),
    }
}

/// A `--worker-model`/`--reviewer-model` flag, checked against the same rule the file's own
/// `model` key uses (decision 48), so a flag can't bypass it. The error is the problem text.
pub(crate) fn check_model_flag(at: &str, model: &str) -> std::result::Result<(), String> {
    if model.trim().is_empty() {
        Err(format!(
            "{at} is empty; name a model or omit the flag for the harness's default"
        ))
    } else {
        Ok(())
    }
}

/// `<n>s`, `<n>m` or `<n>h`, above zero. The error is the problem text.
pub(crate) fn parse_duration(at: &str, text: &str) -> std::result::Result<Duration, String> {
    let parsed = text
        .len()
        .checked_sub(1)
        .filter(|_| text.is_char_boundary(text.len() - 1))
        .and_then(|split| {
            let (digits, unit) = text.split_at(split);
            let n: u64 = digits.parse().ok()?;
            let secs = match unit {
                "s" => n,
                "m" => n.checked_mul(60)?,
                "h" => n.checked_mul(3600)?,
                _ => return None,
            };
            (secs > 0).then_some(Duration::from_secs(secs))
        });
    parsed.ok_or_else(|| {
        format!(
            "{at} \"{text}\" isn't a duration; write a whole number and a unit, e.g. \"90s\", \
             \"10m\" or \"2h\""
        )
    })
}

fn kind_name(kind: QuestionKind) -> &'static str {
    match kind {
        QuestionKind::Noul => "noul",
        QuestionKind::Choice => "choice",
        QuestionKind::Score => "score",
    }
}
