//! `sbxm-task.toml` at the target repo's root (spec §2, decisions 148, 151,
//! 155, 160): parsed and checked without touching `sbx`, `gh` or the profiles.
//! Every struct denies unknown keys, as in the run-config.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::resolve_inside;
use crate::harness::Harness;
use crate::run::config::{headless_harness, parse_duration};
use crate::task::record::DEFAULT_FIX_ROUNDS;

pub const FILE_NAME: &str = "sbxm-task.toml";

const DEFAULT_WORKER_LIMIT: Duration = Duration::from_secs(2 * 3600);
const DEFAULT_REVIEWER_LIMIT: Duration = Duration::from_secs(45 * 60);
const DEFAULT_GATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
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
    /// Whether the reviewer's harness was chosen (in the file or by a flag) rather than defaulted.
    reviewer_chosen: bool,
    pub sandbox: Sandbox,
    pub gates: Gates,
    pub prompts: Prompts,
    /// `[finish] sink` (decision 174(e)): where a spec task's result lands at `task finish`.
    pub sink: Sink,
    /// Things to say once and carry on (the same harness for both roles).
    pub warnings: Vec<String>,
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
    pub cpus: Option<u32>,
    pub memory: Option<String>,
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

/// Where `task finish` puts a spec task's branch (decision 174(e)); an issue task always opens a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sink {
    /// The branch stays in the task's host-owned `repo.git`; `finish` prints how to fetch it.
    #[default]
    Local,
    /// The branch is pushed to `origin`, and no PR is opened.
    Push,
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
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawFinish {
    #[serde(default)]
    sink: Sink,
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
    cpus: Option<u32>,
    memory: Option<String>,
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
        let raw: RawConfig = toml::from_str(&text).map_err(|e| {
            let line = e
                .span()
                .map(|s| format!(" (line {})", text[..s.start].matches('\n').count() + 1))
                .unwrap_or_default();
            anyhow::anyhow!(
                "task config {}{line}: {}",
                path.display(),
                e.message().replace('\n', " ")
            )
        })?;
        Validator {
            path: &path,
            repo_root,
        }
        .validate(raw)
    }
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
        Ok(TaskConfig {
            worker,
            reviewer,
            fix_rounds,
            reviewer_chosen: chosen_reviewer.is_some(),
            sandbox,
            gates,
            prompts,
            sink: raw.finish.sink,
            warnings,
        })
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
        Ok(Sandbox {
            profile,
            cpus: raw.cpus,
            memory: raw.memory,
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
        for (key, commands) in [("gates.sandbox", &sandbox), ("gates.host", &host)] {
            for (i, command) in commands.iter().enumerate() {
                if command.trim().is_empty() {
                    return Err(
                        self.err(format!("{key}[{i}] is empty; write a command or remove it"))
                    );
                }
                // `sh -c` would run every line, `cmd /C` only the first: refuse instead.
                if command.contains(['\n', '\r']) {
                    return Err(self.err(format!(
                        "{key}[{i}] contains a line break; put one command in each list entry"
                    )));
                }
            }
        }
        Ok(Gates {
            sandbox,
            host,
            timeout: self.duration("gates.timeout", raw.timeout, DEFAULT_GATE_TIMEOUT)?,
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
