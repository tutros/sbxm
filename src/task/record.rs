//! The task record, `<base>/.sbxm/tasks/<id>/task.json` (spec §3, decisions 145, 146, 153):
//! the stage machine, atomic writes and the interrupted check. Never holds a secret value.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run::results::format_timestamp;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Issue,
    Pr,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pr",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Prepared,
    Working,
    Gating,
    Reviewing,
    Fixing,
    Ready,
    Finished,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Working => "working",
            Self::Gating => "gating",
            Self::Reviewing => "reviewing",
            Self::Fixing => "fixing",
            Self::Ready => "ready",
            Self::Finished => "finished",
        }
    }

    /// The stages that may follow this one.
    fn next(self) -> &'static [Stage] {
        match self {
            // A PR task has no worker: it goes straight to review.
            Self::Prepared => &[Self::Working, Self::Reviewing],
            Self::Working | Self::Fixing => &[Self::Gating],
            Self::Gating => &[Self::Reviewing],
            Self::Reviewing => &[Self::Fixing, Self::Ready],
            Self::Ready => &[Self::Finished],
            Self::Finished => &[],
        }
    }

    /// The statuses this stage can be in (first: where it starts).
    fn statuses(self) -> &'static [Status] {
        use Status::*;
        match self {
            Self::Prepared => &[Running, Failed],
            Self::Working | Self::Fixing => &[Running, Completed, TimedOut, Failed],
            Self::Gating => &[Running, Passed, GatesFailed],
            Self::Reviewing => &[Running, Completed, Failed],
            Self::Ready | Self::Finished => &[Ok],
        }
    }

    /// The statuses from which the task may move on.
    fn done(self) -> &'static [Status] {
        use Status::*;
        match self {
            Self::Prepared => &[Running],
            Self::Working | Self::Fixing => &[Completed, TimedOut],
            Self::Gating => &[Passed],
            Self::Reviewing => &[Completed],
            Self::Ready => &[Ok],
            Self::Finished => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Running,
    Completed,
    TimedOut,
    Failed,
    Passed,
    GatesFailed,
    Ok,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::TimedOut => "timed-out",
            Self::Failed => "failed",
            Self::Passed => "passed",
            Self::GatesFailed => "gates-failed",
            Self::Ok => "ok",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub stage: Stage,
    pub at: String,
}

/// The `sbxm` process working on the task: its pid and when it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub started_at: String,
}

impl Process {
    /// `started_secs`: the process's start time in seconds since the epoch.
    pub fn new(pid: u32, started_secs: u64) -> Self {
        Self {
            pid,
            started_at: format_timestamp(started_secs),
        }
    }

    /// This process, as the probe sees it.
    pub fn current(probe: &dyn ProcessProbe) -> Self {
        let pid = std::process::id();
        Self::new(pid, probe.start_time(pid).unwrap_or(0))
    }
}

/// Looks a process up by pid (faked in tests).
pub trait ProcessProbe: Send + Sync {
    /// The start time (seconds since the epoch) of the process with this pid, if it exists.
    fn start_time(&self, pid: u32) -> Option<u64>;
}

/// The real probe, over the operating system's process table.
pub struct SystemProbe;

impl ProcessProbe for SystemProbe {
    fn start_time(&self, pid: u32) -> Option<u64> {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        system.process(pid).map(sysinfo::Process::start_time)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    pub status: String,
    #[serde(default)]
    pub usage: Value,
    pub duration_s: u64,
}

/// A worker or reviewer: names are stored, never re-derived (spec §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub harness: String,
    pub model: Option<String>,
    pub sandbox: String,
    pub workspace: String,
    pub run: Option<RunInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateResult {
    pub phase: String,
    pub tier: String,
    pub command: String,
    pub exit: Option<i32>,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub schema: u32,
    pub id: String,
    pub kind: Kind,
    pub number: u32,
    pub repo: String,
    pub title: String,
    pub base: String,
    pub branch: String,
    pub stage: Stage,
    pub status: Status,
    pub stages: Vec<Stamp>,
    pub process: Option<Process>,
    pub worker: Option<Agent>,
    pub reviewer: Option<Agent>,
    pub fix_round: bool,
    pub gates: Vec<GateResult>,
    /// The PR's URL once `finish` opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    pub related: Vec<u32>,
    /// Things worth telling the user about how the task went (no commits, uncommitted changes,
    /// a missing `result.md`); never a secret.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Free-form; later stages attach here and M2b leaves it alone (decision 139).
    pub hooks: Value,
    pub sbxm_version: String,
    pub config_hash: String,
}

pub struct NewTask<'a> {
    pub kind: Kind,
    pub number: u32,
    pub repo: &'a str,
    pub title: &'a str,
    pub base: &'a str,
    pub branch: &'a str,
    pub config_hash: &'a str,
}

impl Record {
    /// A task that has just been created: stage `prepared`, running.
    pub fn new(task: &NewTask, now: u64, process: Process) -> Self {
        Self {
            schema: SCHEMA,
            id: format!("{}-{}", task.kind.prefix(), task.number),
            kind: task.kind,
            number: task.number,
            repo: task.repo.to_owned(),
            title: task.title.to_owned(),
            base: task.base.to_owned(),
            branch: task.branch.to_owned(),
            stage: Stage::Prepared,
            status: Status::Running,
            stages: vec![Stamp {
                stage: Stage::Prepared,
                at: format_timestamp(now),
            }],
            process: Some(process),
            worker: None,
            reviewer: None,
            fix_round: false,
            gates: Vec::new(),
            pr: None,
            related: Vec::new(),
            notes: Vec::new(),
            hooks: Value::Object(serde_json::Map::new()),
            sbxm_version: env!("CARGO_PKG_VERSION").to_owned(),
            config_hash: task.config_hash.to_owned(),
        }
    }

    /// Enters the next stage (running, or `ok` for the last two). Refused, with nothing
    /// changed, unless the current stage is finished and the move is a legal one.
    pub fn advance(&mut self, stage: Stage, now: u64, process: Process) -> Result<()> {
        if !self.stage.next().contains(&stage) {
            bail!(
                "task {} cannot go from {} to {}; see `sbxm task status`",
                self.id,
                self.stage.name(),
                stage.name()
            );
        }
        if !self.stage.done().contains(&self.status) {
            bail!(
                "task {} is {} in stage {}, so it cannot move on; see `sbxm task status`",
                self.id,
                self.status.name(),
                self.stage.name()
            );
        }
        self.stage = stage;
        self.status = stage.statuses()[0];
        self.stages.push(Stamp {
            stage,
            at: format_timestamp(now),
        });
        self.process = Some(process);
        Ok(())
    }

    /// Starts the gates: after the worker or the fix round, or again after earlier gates passed or
    /// failed (the user changed something and wants them re-run). Refused while gates are running.
    pub fn begin_gating(&mut self, now: u64, process: Process) -> Result<()> {
        if self.stage != Stage::Gating {
            return self.advance(Stage::Gating, now, process);
        }
        if self.status == Status::Running {
            bail!(
                "task {} is already running its gates; wait for them, or see `sbxm task status`",
                self.id
            );
        }
        self.status = Status::Running;
        self.stages.push(Stamp {
            stage: Stage::Gating,
            at: format_timestamp(now),
        });
        self.process = Some(process);
        Ok(())
    }

    /// Ends the current stage with `status`, which must belong to the stage.
    pub fn finish(&mut self, status: Status) -> Result<()> {
        if !self.stage.statuses().contains(&status) {
            bail!(
                "status {} doesn't belong to stage {} of task {}",
                status.name(),
                self.stage.name(),
                self.id
            );
        }
        self.status = status;
        Ok(())
    }

    /// `running` whose process is gone (no pid, or the pid now belongs to another process).
    pub fn is_interrupted(&self, probe: &dyn ProcessProbe) -> bool {
        if self.status != Status::Running {
            return false;
        }
        match &self.process {
            None => true,
            Some(process) => probe
                .start_time(process.pid)
                .is_none_or(|started| format_timestamp(started) != process.started_at),
        }
    }
}

/// `issue-<n>` or `pr-<n>`, with a plain positive number: the only ids that may become a path.
pub fn is_valid_id(id: &str) -> bool {
    let Some(digits) = id.strip_prefix("issue-").or_else(|| id.strip_prefix("pr-")) else {
        return false;
    };
    !digits.starts_with('0') && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

pub fn tasks_root(base: &Path) -> PathBuf {
    base.join(".sbxm").join("tasks")
}

pub fn task_dir(base: &Path, id: &str) -> PathBuf {
    tasks_root(base).join(id)
}

/// Writes `<dir>/task.json` through a temp file and a rename, creating `dir`.
pub fn write(dir: &Path, record: &Record) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join("task.json");
    let tmp = dir.join("task.json.tmp");
    let mut text = serde_json::to_string_pretty(record)?;
    text.push('\n');
    fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

pub fn read(path: &Path) -> Result<Record> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} isn't valid JSON; remove the task with `sbxm task rm`",
            path.display()
        )
    })?;
    let schema = value["schema"].as_u64().unwrap_or(0);
    if schema > u64::from(SCHEMA) {
        bail!(
            "{} has schema {schema}, newer than this sbxm understands ({SCHEMA}); upgrade sbxm",
            path.display()
        );
    }
    serde_json::from_value(value).with_context(|| {
        format!(
            "{} isn't a task record; remove the task with `sbxm task rm`",
            path.display()
        )
    })
}

/// Every task under `<base>/.sbxm/tasks/`, ordered by kind then number; a folder without a
/// `task.json` (a task that never got written) is skipped.
pub fn load_all(base: &Path) -> Result<Vec<Record>> {
    let root = tasks_root(base);
    let Ok(entries) = fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry?.path().join("task.json");
        if path.is_file() {
            records.push(read(&path)?);
        }
    }
    records.sort_by_key(|r| (r.kind.prefix(), r.number));
    Ok(records)
}
