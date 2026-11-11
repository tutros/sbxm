//! `sbxm-task.toml` at the target repo's root (spec §2, decisions 148, 151,
//! 155, 160): parsed and checked without touching `sbx`, `gh` or the profiles.
//! Every struct denies unknown keys, as in the run-config.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::backend::Location;
use crate::config::resolve_inside;
use crate::harness::Harness;
use crate::run::config::{headless_harness, parse_duration};
use crate::task::record::DEFAULT_FIX_ROUNDS;
use crate::task::risk::{self, Level, PathRule};
use crate::task::shape::{self, Shape};

pub const FILE_NAME: &str = "sbxm-task.toml";

const DEFAULT_WORKER_LIMIT: Duration = Duration::from_secs(2 * 3600);
const DEFAULT_REVIEWER_LIMIT: Duration = Duration::from_secs(45 * 60);
pub(crate) const DEFAULT_GATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// `[loop] usage_wait` and `usage_wait_total` when absent (issue 320, decision 6).
const DEFAULT_USAGE_WAIT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_USAGE_WAIT_TOTAL: Duration = Duration::from_secs(5 * 3600);
/// Applied when the repo has a `Cargo.toml` and `[gates] sandbox` is absent.
const RUST_GATES: [&str; 3] = [
    "cargo fmt --check",
    "cargo clippy --all-targets -- -D warnings",
    "cargo test",
];
/// The reviewer default is the first of these that differs from the worker's [148].
const REVIEWER_ORDER: [Harness; 3] = [Harness::Codex, Harness::Claude, Harness::Antigravity];

/// The first of [`REVIEWER_ORDER`] that differs from the worker's harness [148].
fn default_reviewer(worker: Harness) -> Harness {
    REVIEWER_ORDER
        .into_iter()
        .find(|h| *h != worker)
        .expect("the order has several harnesses")
}

const SAME_HARNESS: &str = "worker and reviewer use the same harness";

fn same_harness_warning(harness: Harness) -> String {
    format!(
        "{SAME_HARNESS} ({}); the review is less independent, set [reviewer] harness to another \
         to avoid it",
        harness.as_str()
    )
}

#[derive(Debug, Clone)]
pub struct TaskConfig {
    pub worker: Role,
    pub reviewer: Role,
    /// `[worker] fix_rounds` (decision 173(a)): the worker's round budget for `task review`; 0
    /// means the first review that finds anything stops the task at once.
    pub fix_rounds: u32,
    /// `[fix] resume` (issue 418, decision 285): a fix round of a Claude worker resumes the
    /// worker's own session instead of starting a fresh one; off when absent.
    pub fix_resume: bool,
    /// Whether the reviewer's harness was chosen (in the file or by a flag) rather than defaulted.
    reviewer_chosen: bool,
    pub sandbox: Sandbox,
    pub gates: Gates,
    pub prompts: Prompts,
    /// `[finish] sink` (decision 174(e)): where a spec task's result lands at `task finish`.
    pub sink: Sink,
    /// The path-rule floor (decision 176(d)): the built-in defaults, plus `[risk]`'s own rules
    /// (or, with `[risk] replace = true`, only those).
    pub risk: Vec<PathRule>,
    /// Things to say once and carry on (the same harness for both roles).
    pub warnings: Vec<String>,
    /// `[warm]`, read by `template build --warm` (decision 207).
    pub warm: Option<Warm>,
    /// `[loop]`, read by `task loop` (issue 320).
    pub task_loop: Loop,
}

/// How `task loop` waits out a harness's usage limit (issue 320, decision 6): `usage_wait` before
/// each `task resume`, and the loop stops once the waits in a row reach `usage_wait_total`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loop {
    pub usage_wait: Duration,
    pub usage_wait_total: Duration,
    /// `should_fix` (issue 510, decision 306): what the loop does with a clean task's should-fix
    /// findings sized S.
    pub should_fix: ShouldFix,
}

/// `[loop] should_fix` (issue 510, decision 306).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShouldFix {
    /// Every should-fix finding of a clean task is filed as an issue after `task finish`.
    #[default]
    File,
    /// A clean task with should-fix findings sized S gets one more fix round on them (`task
    /// resume --rounds 1 --should-fix`) before `task finish`; the rest are filed.
    Fix,
}

/// How `template build --warm` fills a template's caches from this repo (decision 207).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warm {
    /// Run in order with `sh -c` in the copied repo, each under the gate timeout.
    pub commands: Vec<String>,
    /// Paths from the repo's root, hashed into the template's record.
    pub lock_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Role {
    pub harness: Harness,
    /// The harness's own default when absent.
    pub model: Option<String>,
    pub time_limit: Duration,
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    pub profile: String,
    /// `[sandbox] location` (decision 262(a)): where a new task's sandboxes run unless a flag
    /// says otherwise; local when absent.
    pub location: Location,
    pub cpus: Option<u32>,
    pub memory: Option<String>,
    /// `[sandbox] cloud_shape` (issue 559): the shape of every cloud sandbox of a task, in place
    /// of the local cpus/memory; see [`super::shape`].
    pub cloud_shape: Option<Shape>,
}

#[derive(Debug, Clone)]
pub struct Gates {
    /// Run inside the sandbox; empty means none.
    pub sandbox: Vec<String>,
    /// The optional Windows tier; empty means off (decision 160).
    pub host: Vec<String>,
    /// Per command.
    pub timeout: Duration,
}

/// Where `task finish` puts a spec task's branch (decisions 174(e), 301); an issue task always opens
/// a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sink {
    /// The branch stays in the task's host-owned `repo.git`; `finish` prints how to fetch it.
    #[default]
    Local,
    /// The branch is pushed to `origin`, and no PR is opened.
    Push,
    /// The branch is pushed and a PR opened, with the in-task review posted as its review, as
    /// for an issue task (issue 438, decision 301); the spec's `Fixes:` line names the issues it
    /// closes.
    Pr,
}

/// Override files, as absolute paths inside the repo, checked to exist.
#[derive(Debug, Clone, Default)]
pub struct Prompts {
    pub worker: Option<PathBuf>,
    pub reviewer: Option<PathBuf>,
    pub fix: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    worker: RawWorkerRole,
    #[serde(default)]
    reviewer: RawRole,
    sandbox: Option<RawSandbox>,
    #[serde(default)]
    gates: RawGates,
    #[serde(default)]
    prompts: RawPrompts,
    #[serde(default)]
    finish: RawFinish,
    #[serde(default)]
    fix: RawFix,
    risk: Option<RawRisk>,
    warm: Option<RawWarm>,
    #[serde(default, rename = "loop")]
    task_loop: RawLoop,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLoop {
    usage_wait: Option<String>,
    usage_wait_total: Option<String>,
    #[serde(default)]
    should_fix: ShouldFix,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWarm {
    commands: Option<Vec<String>>,
    lock_files: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawFinish {
    #[serde(default)]
    sink: Sink,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawFix {
    #[serde(default)]
    resume: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRole {
    harness: Option<String>,
    model: Option<String>,
    time_limit: Option<String>,
}

/// `[worker]`: like `RawRole`, plus `fix_rounds`, which only the worker has (a reviewer never
/// fixes its own findings, decision 148).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawWorkerRole {
    harness: Option<String>,
    model: Option<String>,
    time_limit: Option<String>,
    fix_rounds: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSandbox {
    profile: Option<String>,
    #[serde(default)]
    location: Location,
    cpus: Option<u32>,
    memory: Option<String>,
    cloud_shape: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawGates {
    sandbox: Option<Vec<String>>,
    host: Option<Vec<String>>,
    timeout: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPrompts {
    worker: Option<PathBuf>,
    reviewer: Option<PathBuf>,
    fix: Option<PathBuf>,
}

/// `[risk]` (decision 176(d)): path-rule entries that raise the floor at least to `medium` or
/// `high`; `replace` drops the built-in defaults instead of adding to them.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRisk {
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    medium: Vec<String>,
    #[serde(default)]
    high: Vec<String>,
}

impl TaskConfig {
    /// A flag changed the worker's harness: a reviewer nobody chose is chosen again to differ from
    /// it, and the same-harness warning follows.
    pub fn set_worker_harness(&mut self, harness: Harness) {
        self.worker.harness = harness;
        if !self.reviewer_chosen {
            self.reviewer.harness = default_reviewer(harness);
        }
        self.refresh_warning();
    }

    /// A flag chose the reviewer's harness (it is kept from now on).
    pub fn set_reviewer_harness(&mut self, harness: Harness) {
        self.reviewer.harness = harness;
        self.reviewer_chosen = true;
        self.refresh_warning();
    }

    /// Exactly one same-harness warning while the two roles match, none otherwise.
    fn refresh_warning(&mut self) {
        self.warnings.retain(|w| !w.starts_with(SAME_HARNESS));
        if self.worker.harness == self.reviewer.harness {
            self.warnings
                .push(same_harness_warning(self.worker.harness));
        }
    }

    /// Reads and validates `<repo_root>/sbxm-task.toml`.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {}; create one with `sbxm task init` or run from the repo's root",
                path.display()
            )
        })?;
        let raw: RawConfig = parse(&path, &text)?;
        Validator {
            path: &path,
            repo_root,
        }
        .validate(raw)
    }
}

/// `[warm]` of `<repo_root>/sbxm-task.toml`, or `None` when there is no such file or table
/// (decision 207). Only that table is checked, so a repo that is not a task target can hold just
/// `[warm]`.
pub fn load_warm(repo_root: &Path) -> Result<Option<Warm>> {
    #[derive(Deserialize)]
    struct WarmOnly {
        warm: Option<RawWarm>,
    }
    let path = repo_root.join(FILE_NAME);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let raw: WarmOnly = parse(&path, &text)?;
    raw.warm
        .map(|warm| {
            Validator {
                path: &path,
                repo_root,
            }
            .warm(warm)
        })
        .transpose()
}

/// `text` as `T`, or `task config <file> (line n): <problem>`.
fn parse<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    toml::from_str(text).map_err(|e| {
        let line = e
            .span()
            .map(|s| format!(" (line {})", text[..s.start].matches('\n').count() + 1))
            .unwrap_or_default();
        anyhow::anyhow!(
            "task config {}{line}: {}",
            path.display(),
            e.message().replace('\n', " ")
        )
    })
}

struct Validator<'a> {
    path: &'a Path,
    repo_root: &'a Path,
}

impl Validator<'_> {
    /// `task config <file>: <problem>; <fix>`, on one line (decision 48).
    fn err(&self, problem: impl std::fmt::Display) -> anyhow::Error {
        anyhow::anyhow!("task config {}: {problem}", self.path.display())
    }

    fn validate(&self, raw: RawConfig) -> Result<TaskConfig> {
        let mut warnings = Vec::new();

        let worker_harness = self.harness("worker.harness", raw.worker.harness.as_deref())?;
        let worker_harness = worker_harness.unwrap_or(Harness::Claude);
        let chosen_reviewer = self.harness("reviewer.harness", raw.reviewer.harness.as_deref())?;
        let reviewer_harness = match chosen_reviewer {
            Some(h) => {
                if h == worker_harness {
                    warnings.push(same_harness_warning(h));
                }
                h
            }
            None => default_reviewer(worker_harness),
        };
        let fix_rounds = raw.worker.fix_rounds.unwrap_or(DEFAULT_FIX_ROUNDS);
        let worker = self.role(
            "worker",
            worker_harness,
            raw.worker.model,
            raw.worker.time_limit,
            DEFAULT_WORKER_LIMIT,
        )?;
        let reviewer = self.role(
            "reviewer",
            reviewer_harness,
            raw.reviewer.model,
            raw.reviewer.time_limit,
            DEFAULT_REVIEWER_LIMIT,
        )?;

        let sandbox = self.sandbox(raw.sandbox)?;
        let gates = self.gates(raw.gates)?;
        let prompts = Prompts {
            worker: self.prompt("worker", raw.prompts.worker)?,
            reviewer: self.prompt("reviewer", raw.prompts.reviewer)?,
            fix: self.prompt("fix", raw.prompts.fix)?,
        };
        let risk = self.risk(raw.risk)?;
        let warm = raw.warm.map(|w| self.warm(w)).transpose()?;
        let task_loop = self.task_loop(raw.task_loop)?;
        Ok(TaskConfig {
            worker,
            reviewer,
            fix_rounds,
            fix_resume: raw.fix.resume,
            reviewer_chosen: chosen_reviewer.is_some(),
            sandbox,
            gates,
            prompts,
            sink: raw.finish.sink,
            risk,
            warnings,
            warm,
            task_loop,
        })
    }

    /// `[loop]`: both waits are durations, and one wait can't be longer than the waits in a row
    /// the loop allows.
    fn task_loop(&self, raw: RawLoop) -> Result<Loop> {
        let usage_wait = self.duration("loop.usage_wait", raw.usage_wait, DEFAULT_USAGE_WAIT)?;
        let usage_wait_total = self.duration(
            "loop.usage_wait_total",
            raw.usage_wait_total,
            DEFAULT_USAGE_WAIT_TOTAL,
        )?;
        if usage_wait > usage_wait_total {
            return Err(self.err(
                "loop.usage_wait is longer than loop.usage_wait_total; shorten the one or \
                 lengthen the other",
            ));
        }
        Ok(Loop {
            usage_wait,
            usage_wait_total,
            should_fix: raw.should_fix,
        })
    }

    /// The path-rule floor: the built-in defaults plus `[risk]`'s own patterns at `medium` and
    /// `high`, or, with `replace = true`, only those. An empty pattern is refused, naming the key.
    fn risk(&self, raw: Option<RawRisk>) -> Result<Vec<PathRule>> {
        let raw = raw.unwrap_or_default();
        let mut rules = if raw.replace {
            Vec::new()
        } else {
            risk::built_in_rules()
        };
        for (key, level, patterns) in [
            ("risk.medium", Level::Medium, &raw.medium),
            ("risk.high", Level::High, &raw.high),
        ] {
            for (i, pattern) in patterns.iter().enumerate() {
                if pattern.trim().is_empty() {
                    return Err(self.err(format!(
                        "{key}[{i}] is empty; write a path pattern or remove it"
                    )));
                }
                rules.push(PathRule {
                    level,
                    pattern: pattern.to_lowercase(),
                    category: format!("named in [risk] of {}", FILE_NAME),
                });
            }
        }
        Ok(rules)
    }

    fn harness(&self, at: &str, name: Option<&str>) -> Result<Option<Harness>> {
        name.map(|n| headless_harness(at, n, "tasks"))
            .transpose()
            .map_err(|problem| self.err(problem))
    }

    fn role(
        &self,
        table: &str,
        harness: Harness,
        model: Option<String>,
        time_limit: Option<String>,
        default_limit: Duration,
    ) -> Result<Role> {
        if model.as_deref().is_some_and(|m| m.trim().is_empty()) {
            return Err(self.err(format!(
                "{table}.model is empty; name a model or remove the key for {}'s default",
                harness.as_str()
            )));
        }
        Ok(Role {
            harness,
            model,
            time_limit: self.duration(&format!("{table}.time_limit"), time_limit, default_limit)?,
        })
    }

    fn sandbox(&self, raw: Option<RawSandbox>) -> Result<Sandbox> {
        let missing = || {
            self.err(
                "sandbox.profile is missing; name a profile from your profiles dir in \
                 [sandbox] profile (see `sbxm config profiles-dir`)",
            )
        };
        let raw = raw.ok_or_else(missing)?;
        let profile = raw
            .profile
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(missing)?;
        if raw.cpus == Some(0) {
            return Err(self.err("sandbox.cpus is 0; it must be at least 1"));
        }
        let cloud_shape = match raw.cloud_shape {
            None => None,
            Some(name) => Some(Shape::parse(&name).ok_or_else(|| {
                self.err(format!(
                    "sandbox.cloud_shape {name:?} isn't a cloud shape; use {}",
                    shape::NAMES
                ))
            })?),
        };
        Ok(Sandbox {
            profile,
            location: raw.location,
            cpus: raw.cpus,
            memory: raw.memory,
            cloud_shape,
        })
    }

    fn gates(&self, raw: RawGates) -> Result<Gates> {
        let sandbox = match raw.sandbox {
            Some(commands) => commands,
            None if self.repo_root.join("Cargo.toml").is_file() => {
                RUST_GATES.map(str::to_owned).into()
            }
            None => {
                return Err(self.err(
                    "no gates configured for this repo; list the commands in [gates] sandbox \
                     in sbxm-task.toml",
                ));
            }
        };
        let host = raw.host.unwrap_or_default();
        self.commands("gates.sandbox", &sandbox)?;
        self.commands("gates.host", &host)?;
        Ok(Gates {
            sandbox,
            host,
            timeout: self.duration("gates.timeout", raw.timeout, DEFAULT_GATE_TIMEOUT)?,
        })
    }

    /// Refuses an empty command or one with a line break in `key`'s list.
    fn commands(&self, key: &str, commands: &[String]) -> Result<()> {
        for (i, command) in commands.iter().enumerate() {
            if command.trim().is_empty() {
                return Err(self.err(format!("{key}[{i}] is empty; write a command or remove it")));
            }
            // `sh -c` would run every line, `cmd /C` only the first: refuse instead.
            if command.contains(['\n', '\r']) {
                return Err(self.err(format!(
                    "{key}[{i}] contains a line break; put one command in each list entry"
                )));
            }
        }
        Ok(())
    }

    /// `[warm]` (decision 207): commands to run and lock files to hash, both required; each lock
    /// file a relative path inside the repo that exists.
    fn warm(&self, raw: RawWarm) -> Result<Warm> {
        let commands = raw.commands.filter(|c| !c.is_empty()).ok_or_else(|| {
            self.err("warm.commands is missing or empty; list the commands that fill the caches")
        })?;
        self.commands("warm.commands", &commands)?;
        let lock_files = raw.lock_files.ok_or_else(|| {
            self.err(
                "warm.lock_files is missing; list the files whose change needs a new template, \
                 or write lock_files = [] for none",
            )
        })?;
        for file in &lock_files {
            let inside = !file.is_empty()
                && Path::new(file)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)));
            if !inside {
                return Err(self.err(format!(
                    "warm.lock_files entry `{file}` is not a relative path inside the repo; name \
                     it from the repo's root, without `..`"
                )));
            }
            let path = self.repo_root.join(file);
            if !path.is_file() {
                return Err(self.err(format!(
                    "lock file {} does not exist; create it or remove it from warm.lock_files",
                    path.display()
                )));
            }
        }
        Ok(Warm {
            commands,
            lock_files,
        })
    }

    /// An override file: inside the repo, no links, and it must exist.
    fn prompt(&self, name: &str, relative: Option<PathBuf>) -> Result<Option<PathBuf>> {
        let Some(relative) = relative else {
            return Ok(None);
        };
        let key = format!("prompts.{name}");
        let path = resolve_inside(&key, &relative, self.repo_root, FILE_NAME, self.path)?;
        if !path.is_file() {
            bail!(
                "{key} file {} is missing or not a file; create it or fix {}",
                path.display(),
                self.path.display()
            );
        }
        Ok(Some(path))
    }

    fn duration(&self, at: &str, text: Option<String>, default: Duration) -> Result<Duration> {
        match text {
            Some(text) => parse_duration(at, &text).map_err(|problem| self.err(problem)),
            None => Ok(default),
        }
    }
}
