//! The loop's job queue (issue 525, decision 309): `<base>/.sbxm/queue/*.json`, one job file each,
//! written by the agent or the user and drained by `sbxm task loop` before each new issue. A job
//! is one of the fixed shapes ([`Job`]; `template` and `spec` from issue 524), parsed with no
//! unknown field allowed, so no free-form flag can be queued. Taking a job moves its file to `queue/running/`; its end writes
//! `queue/done/<job>.json` ([`JobResult`]) and removes the running file. A file still in
//! `running/` when a loop starts was interrupted, and is reported, never run again.
//!
//! The labelled-issue queue is [`super::queue`]; this module is the job files alone.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::finish::{check_own_folder, pr_number};
use super::merged::archive_root;
use super::record::{self, Record};
use crate::config::{GlobalConfig, Profile};
use crate::harness::Harness;

/// One queued job (answers 1 and 2 of issue 525): its kind and its fields, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Job {
    /// `task review --pr N` of a PR sbxm opened.
    Review { pr: u32 },
    /// `task fix --pr N`, or with `check_run` `task fix --pr N --check-run ID`, of a PR sbxm
    /// opened.
    Fix {
        pr: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        check_run: Option<u64>,
    },
    /// `task run --issue N` of an issue labeled `ready-for-worker`, then what the loop does after
    /// a task's run (its finish, auto-merge, the findings filed).
    Run { issue: u32 },
    /// `task rm --merged --yes`.
    RmMerged {},
    /// `task run --spec <specs_dir>/<file>`, then its finish and auto-merge as for a `run` job's
    /// issue (issue 524, answers 1 and 2). `file` is a plain file name in `specs_dir`.
    Spec { file: String },
    /// `template build <profile> --harness <harness>`, with `warm` `--warm <the loop's
    /// checkout>` (issue 524, answer 3). No repo path, `--ignore-disk` or `--no-wait` can be
    /// queued.
    Template {
        profile: String,
        #[serde(default)]
        harness: JobHarness,
        #[serde(default)]
        warm: bool,
    },
}

/// The harnesses a template can be built for (`template build`'s own refusal of the others).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum JobHarness {
    #[default]
    Claude,
    Codex,
    Pi,
}

impl JobHarness {
    pub fn harness(self) -> Harness {
        match self {
            Self::Claude => Harness::Claude,
            Self::Codex => Harness::Codex,
            Self::Pi => Harness::Pi,
        }
    }
}

impl Job {
    /// The job's text in the loop's log and on screen: `job review #465`.
    pub fn label(&self) -> String {
        match self {
            Self::Review { pr } => format!("job review #{pr}"),
            Self::Fix {
                pr,
                check_run: None,
            } => format!("job fix #{pr}"),
            Self::Fix {
                pr,
                check_run: Some(run),
            } => format!("job fix #{pr} --check-run {run}"),
            Self::Run { issue } => format!("job run #{issue}"),
            Self::RmMerged {} => "job rm-merged".to_owned(),
            Self::Spec { file } => format!("job spec {file}"),
            Self::Template {
                profile,
                harness,
                warm,
            } => {
                let harness = match harness {
                    JobHarness::Claude => String::new(),
                    other => format!(" --harness {}", other.harness().as_str()),
                };
                let warm = if *warm { " --warm" } else { "" };
                format!("job template {profile}{harness}{warm}")
            }
        }
    }
}

impl Job {
    /// The job's kind and target in a queued file's name (issue 529): `run-524`, `review-465`,
    /// `fix-465`, `rm-merged`, `spec-<file stem>`, `template-<profile>-<harness>`.
    pub fn file_part(&self) -> String {
        match self {
            Self::Review { pr } => format!("review-{pr}"),
            Self::Fix { pr, .. } => format!("fix-{pr}"),
            Self::Run { issue } => format!("run-{issue}"),
            Self::RmMerged {} => "rm-merged".to_owned(),
            Self::Spec { file } => {
                let stem = Path::new(file)
                    .file_stem()
                    .map_or_else(|| file.clone(), |s| s.to_string_lossy().into_owned());
                format!("spec-{stem}")
            }
            Self::Template {
                profile, harness, ..
            } => format!("template-{profile}-{}", harness.harness().as_str()),
        }
    }

    /// Whether `other` is a job of the same kind and target (issue 529, answer 2): a fix's check
    /// run and a template's `warm` are not part of the target, and two `rm-merged` are the same.
    pub fn same_target(&self, other: &Job) -> bool {
        match (self, other) {
            (Self::Review { pr: a }, Self::Review { pr: b })
            | (Self::Fix { pr: a, .. }, Self::Fix { pr: b, .. })
            | (Self::Run { issue: a }, Self::Run { issue: b }) => a == b,
            (Self::RmMerged {}, Self::RmMerged {}) => true,
            (Self::Spec { file: a }, Self::Spec { file: b }) => a == b,
            (
                Self::Template {
                    profile: a,
                    harness: ha,
                    ..
                },
                Self::Template {
                    profile: b,
                    harness: hb,
                    ..
                },
            ) => a == b && ha == hb,
            _ => false,
        }
    }
}

/// Parses a job file's text: one of [`Job`]'s shapes, numbers from 1. The error says why it is
/// refused.
pub fn parse(text: &str) -> std::result::Result<Job, String> {
    let job: Job = serde_json::from_str(text).map_err(|e| {
        format!(
            "not a job: {e}; a job is one of {{\"kind\": \"review\", \"pr\": N}}, {{\"kind\": \
             \"fix\", \"pr\": N}} (optionally with \"check_run\": ID), {{\"kind\": \"run\", \
             \"issue\": N}}, {{\"kind\": \"rm-merged\"}}, {{\"kind\": \"spec\", \"file\": \
             \"<a file name in specs_dir>\"}} or {{\"kind\": \"template\", \
             \"profile\": P}} (optionally with \"harness\": \"claude\", \"codex\" or \"pi\", \
             and \"warm\": true)"
        )
    })?;
    match job {
        Job::Review { pr: 0 } | Job::Fix { pr: 0, .. } => {
            Err("not a job: PR numbers start at 1".to_owned())
        }
        Job::Run { issue: 0 } => Err("not a job: issue numbers start at 1".to_owned()),
        Job::Fix {
            check_run: Some(0), ..
        } => Err("not a job: check run ids start at 1".to_owned()),
        Job::Spec { file } => check_spec_name(&file).map(|()| Job::Spec { file }),
        job => Ok(job),
    }
}

/// Refuses a spec job's `file` that isn't a plain file name (issue 524, answer 1): empty, or
/// with a path separator, `..`, a drive letter or a colon, or a control character.
fn check_spec_name(file: &str) -> std::result::Result<(), String> {
    let why = if file.is_empty() {
        "its file name is empty"
    } else if file.contains(['/', '\\']) {
        "its file name has a path separator"
    } else if file.contains("..") {
        "its file name has `..`"
    } else if file.contains(':') {
        "its file name has a drive letter or a colon"
    } else if file.chars().any(char::is_control) {
        "its file name has a control character"
    } else {
        return Ok(());
    };
    Err(format!(
        "not a job: {why}; a spec job names a plain file name in specs_dir, like \
         {{\"kind\": \"spec\", \"file\": \"retro.md\"}}"
    ))
}

/// Where a spec job's `file` is and its task's id (issue 524): a plain file in `specs_dir`, the
/// loop's check when it takes the job and `task queue spec`'s before it writes one. The error says
/// why the job is refused.
pub fn spec_file(
    config_dir: &Path,
    config: &GlobalConfig,
    file: &str,
) -> std::result::Result<(PathBuf, String), String> {
    let Some(dir) = &config.specs_dir else {
        return Err(format!(
            "no specs_dir is set in {}, so no spec job runs; set specs_dir to the folder of the \
             spec files",
            config_dir.join("config.toml").display()
        ));
    };
    let spec = dir.join(file);
    match fs::symlink_metadata(&spec) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(format!("{} isn't a plain file", spec.display())),
        Err(e) => {
            return Err(format!(
                "{} cannot be read ({e}); a spec job names a file in specs_dir {}",
                spec.display(),
                dir.display()
            ));
        }
    }
    let (id, _) = record::spec_id(&spec).map_err(|e| format!("{e:#}"))?;
    Ok((spec, id))
}

/// Refuses a template job of a profile that doesn't load or has no `[template]` table (issue
/// 524, answer 3): the loop's check when it takes the job and `task queue template`'s.
pub fn template_profile(config: &GlobalConfig, profile: &str) -> std::result::Result<(), String> {
    match Profile::load(config.profiles_dir(), profile) {
        Err(e) => Err(format!("{e:#}")),
        Ok(found) if found.template.is_none() => Err(format!(
            "profile '{profile}' has no [template] table, so it has no template to build"
        )),
        Ok(_) => Ok(()),
    }
}

/// `<base>/.sbxm/queue`.
pub fn queue_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".sbxm").join("queue")
}

/// `<base>/.sbxm/queue/running`: the jobs taken and not ended.
pub fn running_dir(base_dir: &Path) -> PathBuf {
    queue_dir(base_dir).join("running")
}

/// `<base>/.sbxm/queue/done`: one result file per job.
pub fn done_dir(base_dir: &Path) -> PathBuf {
    queue_dir(base_dir).join("done")
}

/// Refuses a queue folder that is a link or isn't really under the base dir, as the task folders
/// are refused (decision 195); a missing one is fine.
pub fn check_queue(base_dir: &Path) -> Result<()> {
    for dir in [
        queue_dir(base_dir),
        running_dir(base_dir),
        done_dir(base_dir),
    ] {
        if let Ok(meta) = fs::symlink_metadata(&dir) {
            check_own_folder(base_dir, &dir)?;
            if !meta.is_dir() {
                bail!(
                    "{} isn't a folder; the loop's job queue is kept there, so move it away",
                    dir.display()
                );
            }
        }
    }
    Ok(())
}

/// The `*.json` plain files in `dir`, oldest first by modified time, ties by file name (answer
/// 3). Anything else (`x.json.tmp` while it is written, a folder, a link) is left alone. A
/// missing folder has none.
fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut files: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((modified, name, path));
    }
    files.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(files.into_iter().map(|(_, _, path)| path).collect())
}

/// The jobs waiting in the queue folder, in the order they run.
pub fn pending(base_dir: &Path) -> Result<Vec<PathBuf>> {
    json_files(&queue_dir(base_dir))
}

/// The jobs left in `running/` by a loop that stopped while they ran.
pub fn left_running(base_dir: &Path) -> Result<Vec<PathBuf>> {
    json_files(&running_dir(base_dir))
}

/// `dir`, a folder under `base_dir`, checked if it exists, created if it doesn't.
fn own_folder(base_dir: &Path, dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(_) => check_own_folder(base_dir, dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(dir).with_context(|| format!("cannot create {}", dir.display()))
        }
        Err(e) => Err(e).with_context(|| format!("cannot read {}", dir.display())),
    }
}

/// Takes the job at `path`: moves it to `running/` (created when needed) and returns its new
/// place and its text, as read, or why it can't be read (the job is then refused, not left
/// there).
pub fn take(
    base_dir: &Path,
    path: &Path,
) -> Result<(PathBuf, std::result::Result<String, String>)> {
    let running = running_dir(base_dir);
    own_folder(base_dir, &running)?;
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    let taken = running.join(name);
    fs::rename(path, &taken)
        .with_context(|| format!("cannot move {} to {}", path.display(), running.display()))?;
    let text = fs::read(&taken)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|e| format!("cannot read {}: {e}", taken.display()));
    Ok((taken, text))
}

/// What a job file held: its JSON when it is JSON, else its raw text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsRead {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_text: Option<String>,
}

impl AsRead {
    pub fn new(text: &str) -> Self {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => Self {
                job: Some(value),
                job_text: None,
            },
            Err(_) => Self {
                job: None,
                job_text: Some(text.to_owned()),
            },
        }
    }
}

/// What a running file holds once its job started: the job as read, when it started and the task
/// it starts, so a loop that finds it there after a stop can name that task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Running {
    #[serde(flatten)]
    pub read: AsRead,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Running {
    /// Reads a running file: what [`mark`] wrote, or else the job file as it was moved there.
    pub fn read(text: &str) -> Self {
        serde_json::from_str::<Running>(text).unwrap_or_else(|_| Self {
            read: AsRead::new(text),
            started_at: String::new(),
            task: None,
        })
    }
}

/// Rewrites the running file at `taken` with what [`Running`] holds, through a temporary file.
pub fn mark(taken: &Path, running: &Running) -> Result<()> {
    let tmp = taken.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(running)?;
    fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, taken).with_context(|| format!("cannot write {}", taken.display()))
}

/// How a job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Done,
    Failed,
    Stopped,
    Refused,
    Skipped,
    Interrupted,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Refused => "refused",
            Self::Skipped => "skipped",
            Self::Interrupted => "interrupted",
        }
    }
}

/// `queue/done/<job>.json`: the job as read, how it ended and why, when, the task it ran, the URL
/// it produced and the next step sbxm names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    #[serde(flatten)]
    pub read: AsRead,
    pub outcome: Outcome,
    pub reason: String,
    pub started_at: String,
    pub ended_at: String,
    pub task: Option<String>,
    pub url: Option<String>,
    pub next: Option<String>,
}

/// Writes `result` as `done/<name>`, or `done/<stem>-2.json` and on when that name is taken: a
/// result file is never overwritten. Then removes the running file `taken`. Returns the result's
/// path.
pub fn end(base_dir: &Path, taken: &Path, result: &JobResult) -> Result<PathBuf> {
    let done = done_dir(base_dir);
    own_folder(base_dir, &queue_dir(base_dir))?;
    own_folder(base_dir, &done)?;
    let name = taken
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("job.json");
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let mut text = serde_json::to_string_pretty(result)?;
    text.push('\n');
    let mut n = 1u32;
    let path = loop {
        let candidate = if n == 1 {
            done.join(format!("{stem}.json"))
        } else {
            done.join(format!("{stem}-{n}.json"))
        };
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .with_context(|| format!("cannot write {}", candidate.display()))?;
                break candidate;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => {
                return Err(e).with_context(|| format!("cannot create {}", candidate.display()));
            }
        }
    };
    match fs::remove_file(taken) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot remove {}", taken.display())),
    }
    Ok(path)
}

/// The task record that names PR `pr` of `repo` as its `pr` (answer 4): a live one under the
/// task folders, else one archived under `.sbxm/task-archive/`. A folder that isn't sbxm's own,
/// or a record that can't be read, doesn't count.
pub fn opener(base_dir: &Path, repo: &str, pr: u32) -> Option<Record> {
    let opened = |dir: &Path| -> Option<Record> {
        check_own_folder(base_dir, dir).ok()?;
        let rec = record::read(&dir.join("task.json")).ok()?;
        (rec.repo == repo && rec.pr.as_deref().and_then(pr_number) == Some(pr)).then_some(rec)
    };
    let live = record::task_folders(base_dir).unwrap_or_default();
    if let Some(rec) = live.iter().find_map(|dir| opened(dir)) {
        return Some(rec);
    }
    let archived: Vec<PathBuf> = fs::read_dir(archive_root(base_dir))
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    archived.iter().find_map(|dir| opened(dir))
}
