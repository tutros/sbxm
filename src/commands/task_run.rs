//! `sbxm task run --issue N` (spec §4, decision 158): `start` then `review` for one issue, and
//! then it stops; `finish` publishes, so it stays a deliberate command of its own.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::task_review;
use super::task_start::{self, Restart};
use crate::backend::SandboxBackend;
use crate::github::GitHubBackend;
use crate::harness::Harness;
use crate::run::config::{headless_harness, parse_duration};
use crate::task::gates::HostRunner;
use crate::task::record::ProcessProbe;
use crate::task::repo::Identity;

/// The flags `start` and `review` share, and each one's own.
pub struct Options {
    /// The target repo's root, where `sbxm-task.toml` is.
    pub repo_root: PathBuf,
    pub issue: u32,
    pub worker_harness: Option<Harness>,
    pub worker_model: Option<String>,
    pub reviewer_harness: Option<Harness>,
    pub reviewer_model: Option<String>,
    /// The worker's time limit, also for the fix round.
    pub time_limit: Option<String>,
    pub reviewer_time_limit: Option<String>,
    pub profile: Option<String>,
    pub base: Option<String>,
    pub repo: Option<String>,
    /// For tests: what `repo.git` is cloned from.
    pub clone_source: Option<String>,
    /// For tests: the committer identity.
    pub identity: Option<Identity>,
}

/// Drops the `next:` hint of a step that isn't the last one.
fn without_next(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .lines()
        .filter(|line| !line.trim_start().starts_with("next:"))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Passes each line on once: `start` and `review` both say the config's warnings.
struct Once<'a> {
    inner: &'a mut dyn Write,
    seen: Vec<Vec<u8>>,
    line: Vec<u8>,
}

impl Write for Once<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &byte in buf {
            self.line.push(byte);
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                if !self.seen.contains(&line) {
                    self.inner.write_all(&line)?;
                    self.seen.push(line);
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    config_dir: &Path,
    opts: &Options,
    restart: Option<&Restart>,
    backend: &dyn SandboxBackend,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    host: &dyn HostRunner,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    // The review's flags are checked now, not after the worker has spent its time.
    if let Some(harness) = opts.reviewer_harness {
        headless_harness("--reviewer-harness", harness.as_str(), "tasks")
            .map_err(|e| anyhow!(e))?;
    }
    if let Some(limit) = &opts.reviewer_time_limit {
        parse_duration("--reviewer-time-limit", limit).map_err(|e| anyhow!(e))?;
    }

    let start = task_start::Options {
        repo_root: opts.repo_root.clone(),
        issues: vec![opts.issue],
        workers: None,
        worker_harness: opts.worker_harness,
        worker_model: opts.worker_model.clone(),
        time_limit: opts.time_limit.clone(),
        profile: opts.profile.clone(),
        base: opts.base.clone(),
        repo: opts.repo.clone(),
        clone_source: opts.clone_source.clone(),
        identity: opts.identity.clone(),
    };
    let mut warn = Once {
        inner: warn,
        seen: Vec::new(),
        line: Vec::new(),
    };
    let mut started = Vec::new();
    let result = task_start::run_with(
        config_dir,
        &start,
        restart,
        backend,
        github,
        probe,
        &mut started,
        &mut warn,
    );
    out.write_all(without_next(&started).as_bytes())?;
    result?;

    task_review::run(
        config_dir,
        &task_review::Options {
            repo_root: opts.repo_root.clone(),
            target: task_review::Target::Issue(opts.issue),
            repo: opts.repo.clone(),
            base: opts.base.clone(),
            clone_source: opts.clone_source.clone(),
            reviewer_harness: opts.reviewer_harness,
            reviewer_model: opts.reviewer_model.clone(),
            reviewer_time_limit: opts.reviewer_time_limit.clone(),
            time_limit: opts.time_limit.clone(),
            profile: opts.profile.clone(),
        },
        backend,
        github,
        probe,
        host,
        out,
        &mut warn,
    )
}
