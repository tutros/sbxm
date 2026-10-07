//! The task record, `<base>/.sbxm/tasks/<id>/task.json` (spec §3, decisions 145, 146, 153):
//! the stage machine, atomic writes and the interrupted check. Never holds a secret value.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::run::results::format_timestamp;

pub const SCHEMA: u32 = 1;

/// `[worker] fix_rounds` in `sbxm-task.toml` when the key is absent, and what an older record
/// (written before this field existed) reads as (issue 117, decision 173(a)).
pub const DEFAULT_FIX_ROUNDS: u32 = 3;

fn default_fix_rounds() -> u32 {
    DEFAULT_FIX_ROUNDS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Issue,
    Pr,
    /// A task whose source is a file instead of a GitHub issue (decision 174(d), issue 142): the
    /// id is given explicitly (`spec-<name>-<hash>`, see [`spec_id`]), never derived from
    /// `number`, which is unused (always 0).
    Spec,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pr",
            Self::Spec => "spec",
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

    /// The stages that may follow this one, for a task of `kind`: an issue or a spec task must
    /// pass through its worker, and a PR task (no worker, no fix round, never finished: spec
    /// section 5.2) never reaches `working`, `fixing` or `finished`. A failed gate feeds a fix
    /// round too, for a task with a worker and rounds left (issue 117, decision 173(d)); a PR
    /// task's gates have no such move since its `fix_rounds` is always 0. A spec task reaches
    /// `ready` like an issue task, but not `finished`: `finish` is refused until a sink lands
    /// (decision 174(e), issue 142), so there is no row from `ready` to `finished` for it yet.
    pub(crate) fn next(self, kind: Kind) -> &'static [Stage] {
        match (self, kind) {
            (Self::Prepared, Kind::Issue | Kind::Spec) => &[Self::Working],
            // A PR task has no worker: it goes to its gates (run in the reviewer's sandbox) or
            // straight to review when none are configured (decision 175, finding F3).
            (Self::Prepared, Kind::Pr) => &[Self::Gating, Self::Reviewing],
            (Self::Working | Self::Fixing, Kind::Issue | Kind::Spec) => &[Self::Gating],
            (Self::Working | Self::Fixing, Kind::Pr) => &[],
            (Self::Gating, Kind::Issue | Kind::Spec) => &[Self::Reviewing, Self::Fixing],
            (Self::Gating, Kind::Pr) => &[Self::Reviewing],
            (Self::Reviewing, Kind::Issue | Kind::Spec) => &[Self::Fixing, Self::Ready],
            (Self::Reviewing, Kind::Pr) => &[Self::Ready],
            (Self::Ready, Kind::Issue) => &[Self::Finished],
            (Self::Ready, Kind::Spec | Kind::Pr) | (Self::Finished, _) => &[],
        }
    }

    /// The statuses this stage can be in (first: where it starts).
    pub(crate) fn statuses(self) -> &'static [Status] {
        use Status::*;
        match self {
            Self::Prepared => &[Running, Failed],
            Self::Working | Self::Fixing => &[Running, Completed, TimedOut, Failed],
            Self::Gating => &[Running, Passed, GatesFailed],
            Self::Reviewing => &[Running, Completed, Failed],
            Self::Ready | Self::Finished => &[Ok],
        }
    }

    /// The statuses from which the task may move on to `to`: leaving `Gating` needs `Passed` for
    /// `Reviewing` but `GatesFailed` for `Fixing` (issue 117), so the two moves don't share a
    /// status set the way every other stage's moves do.
    pub(crate) fn done_for(self, to: Stage) -> &'static [Status] {
        use Status::*;
        match (self, to) {
            (Self::Gating, Self::Fixing) => &[GatesFailed],
            (Self::Gating, _) => &[Passed],
            (Self::Prepared, _) => &[Running],
            (Self::Working | Self::Fixing, _) => &[Completed, TimedOut],
            (Self::Reviewing, _) => &[Completed],
            (Self::Ready, _) => &[Ok],
            (Self::Finished, _) => &[],
        }
    }
}

/// Why a task ended `ready` with something left unresolved (spec §5.1, decision 174(c)): recorded
/// once and read by `task finish` and `task resume`, never re-derived from `review.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stopped {
    /// The round budget (`fix_rounds`) was used up with must-fix findings still open.
    RoundsExhausted,
    /// A review validly claimed `Repeat of: <id>` against an earlier must-fix finding: the same
    /// problem, in the same file, came back after a fix round (spec §5.3, decisions 174(b), 177(e),
    /// issue 119).
    RepeatFinding,
}

impl Stopped {
    pub fn name(self) -> &'static str {
        match self {
            Self::RoundsExhausted => "rounds-exhausted",
            Self::RepeatFinding => "repeat-finding",
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

/// What the latest *passed* gate run covered: which tiers passed, on which commit of the task
/// branch in `repo.git`. Review skips its own gate run only when this covers every configured tier
/// on the branch's current commit, and a host-only run needs the sandbox tier to have passed on the
/// same commit (decision 160, PR 57 review round 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRun {
    pub sandbox: bool,
    pub host: bool,
    /// The branch tip the gates ran against; `None` when it couldn't be read (never matches).
    pub commit: Option<String>,
}

/// The open PR an issue's task continues (decision 169): the issue names it (`PR: #n`), and the
/// task started from its branch head instead of the base branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrBranch {
    pub pr: u32,
    /// The PR's branch, which is also the task's branch.
    pub branch: String,
    /// The commit the PR's branch was at when the task started.
    pub base: String,
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
    /// Fix rounds used so far (issue 117, decision 173(a)); an older record without this field
    /// reads as 0.
    #[serde(default)]
    pub round: u32,
    /// The budget: `[worker] fix_rounds` as it was when the task started (a later config change
    /// affects new tasks only); 0 for a PR task, which has no worker. An older record without
    /// this field reads as the default.
    #[serde(default = "default_fix_rounds")]
    pub fix_rounds: u32,
    /// Set when the task ends `ready` with something left unresolved (spec §5.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Stopped>,
    /// The branch commit the last review covered; `None` means the next review is full (spec
    /// §5.1, §5.2: a narrow review exists only once this is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reviewed_commit: Option<String>,
    pub gates: Vec<GateResult>,
    /// The latest gate run that passed, and what it covered; cleared when gates start again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_run: Option<GateRun>,
    /// The PR's URL once `finish` opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// Set when the task continues an open PR's branch; absent in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<PrBranch>,
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
    /// The task id, for a kind (`Kind::Spec`) whose id isn't `<prefix>-<number>`; `None` for
    /// `Kind::Issue`/`Kind::Pr`, which derive it from `number`.
    pub id: Option<&'a str>,
}

impl Record {
    /// The task's own commits in `repo_git`: past the PR head it started from when it continues
    /// a PR (decision 169), else past its base branch.
    pub fn commits_ahead(&self, repo_git: &Path) -> Result<u32> {
        match &self.continues {
            Some(c) => super::repo::commits_since(repo_git, &c.base, &self.branch),
            None => super::repo::commits_ahead(repo_git, &self.base, &self.branch),
        }
    }

    /// The paths the task's own commits change: past the PR head it started from when it
    /// continues a PR, else past where it left its base branch.
    pub fn changed_paths(&self, repo_git: &Path) -> Result<Vec<String>> {
        match &self.continues {
            Some(c) => super::repo::changed_paths_since(repo_git, &c.base, &self.branch),
            None => super::repo::changed_paths(repo_git, &self.base, &self.branch),
        }
    }

    /// What `git bundle create` excludes in the worker's clone: the commit a continued PR's
    /// branch started from, else the base branch's remote ref.
    pub fn bundle_exclusion(&self) -> String {
        match &self.continues {
            Some(c) => format!("^{}", c.base),
            None => format!("^origin/{}", self.base),
        }
    }

    /// Where the reviewer's scope starts, as a git revision: the commit a continued PR's branch
    /// started from, else the base branch's remote ref.
    pub fn scope_base(&self) -> String {
        match &self.continues {
            Some(c) => c.base.clone(),
            None => format!("origin/{}", self.base),
        }
    }

    /// A task that has just been created: stage `prepared`, running.
    pub fn new(task: &NewTask, now: u64, process: Process) -> Self {
        Self {
            schema: SCHEMA,
            id: task
                .id
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{}-{}", task.kind.prefix(), task.number)),
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
            round: 0,
            fix_rounds: DEFAULT_FIX_ROUNDS,
            stopped: None,
            last_reviewed_commit: None,
            gates: Vec::new(),
            gate_run: None,
            pr: None,
            continues: None,
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
        if !self.stage.next(self.kind).contains(&stage) {
            bail!(
                "task {} cannot go from {} to {}; see `sbxm task status`",
                self.id,
                self.stage.name(),
                stage.name()
            );
        }
        if !self.stage.done_for(stage).contains(&self.status) {
            bail!(
                "task {} is {} in stage {}, so it cannot move on; see `sbxm task status`",
                self.id,
                self.status.name(),
                self.stage.name()
            );
        }
        if stage == Stage::Gating {
            self.gate_run = None;
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
        self.gate_run = None;
        self.stages.push(Stamp {
            stage: Stage::Gating,
            at: format_timestamp(now),
        });
        self.process = Some(process);
        Ok(())
    }

    /// Starts a review round: after the gates, after a review that failed, or again after a
    /// review that completed (a retry, or the confirmatory full review that follows a clean
    /// narrow one, spec §5.2, issue 117). Refused only while one is already running.
    pub fn begin_review(&mut self, now: u64, process: Process) -> Result<()> {
        if self.stage != Stage::Reviewing {
            return self.advance(Stage::Reviewing, now, process);
        }
        if self.status == Status::Running {
            bail!(
                "task {} is already running its review; wait for it, or see `sbxm task status`",
                self.id
            );
        }
        self.status = Status::Running;
        self.stages.push(Stamp {
            stage: Stage::Reviewing,
            at: format_timestamp(now),
        });
        self.process = Some(process);
        Ok(())
    }

    /// Gates that were running but whose `sbxm` process is gone (a crash, Ctrl-C) never finished:
    /// restarts them in place, owned by `process`, so they stay `running` (never recorded as
    /// `gates-failed`, which would consume a fix round once #117 lands) and are re-runnable at
    /// once. Returns whether that happened.
    pub fn abandon_interrupted_gates(
        &mut self,
        now: u64,
        process: Process,
        probe: &dyn ProcessProbe,
    ) -> bool {
        if self.stage == Stage::Gating && self.is_interrupted(probe) {
            self.gate_run = None;
            self.stages.push(Stamp {
                stage: Stage::Gating,
                at: format_timestamp(now),
            });
            self.process = Some(process);
            return true;
        }
        false
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

/// `issue-<n>` or `pr-<n>`, with a plain positive number, or a `spec-<name>-<hash>` id built by
/// [`spec_id`]: the only ids that may become a path.
pub fn is_valid_id(id: &str) -> bool {
    if let Some(digits) = id.strip_prefix("issue-").or_else(|| id.strip_prefix("pr-")) {
        return !digits.starts_with('0')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit());
    }
    is_valid_spec_id(id)
}

/// `spec-<name>-<hash>`: `name` passes [`crate::project::validate_name`] and `hash` is exactly 6
/// lowercase hex digits. Checked structurally, not by re-deriving it from a file (there is none
/// to read here), so it rejects anything [`spec_id`] could not have built, including a `name`
/// that smuggles a path separator.
fn is_valid_spec_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("spec-") else {
        return false;
    };
    let Some((name, hash)) = rest.rsplit_once('-') else {
        return false;
    };
    hash.len() == 6
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        && crate::project::validate_name(name).is_ok()
}

/// The sanitized file name for a spec task's id: lowercased, every run of characters that are not
/// `[a-z0-9]` collapsed to one `-`, and leading/trailing `-` trimmed (so `"My Spec v2.md"`'s stem
/// `"My Spec v2"` becomes `"my-spec-v2"`).
fn sanitize_spec_name(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len());
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.trim_start_matches('-').to_owned()
}

/// `spec-<name>-<hash>` for `task start --spec <file>` (decision 174(d), issue 142, spec section
/// 5.1 "source"): `name` is the sanitized file name (its extension dropped, then validated like a
/// project name) and `hash` is the first 6 hex digits of the SHA-256 of the file's canonical path,
/// so two files named alike never collide. Returns the id and the file's name (for the record's
/// title); the file must exist, so the path is canonicalized before anything else reads it.
pub fn spec_id(path: &Path) -> Result<(String, String)> {
    let canonical =
        fs::canonicalize(path).with_context(|| format!("cannot read {}", path.display()))?;
    spec_id_for_canonical(&canonical)
}

/// The pure part of [`spec_id`]: no filesystem access, so the same canonical path always gives the
/// same id whether or not it exists. The hash covers the path's raw encoded bytes, not a lossy text
/// conversion, so two paths that differ only in bytes that aren't valid text never share an id.
pub fn spec_id_for_canonical(canonical: &Path) -> Result<(String, String)> {
    let file_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", canonical.display()))?
        .to_owned();
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file_name);
    let name = sanitize_spec_name(stem);
    crate::project::validate_name(&name).map_err(|e| {
        anyhow::anyhow!(
            "{}: its name sanitizes to {name:?}, which isn't a usable task id ({e:#})",
            canonical.display()
        )
    })?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let id = format!(
        "spec-{name}-{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2]
    );
    Ok((id, file_name))
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
    records.sort_by(|a, b| {
        (a.kind.prefix(), a.number, &a.id).cmp(&(b.kind.prefix(), b.number, &b.id))
    });
    Ok(records)
}
