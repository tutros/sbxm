//! `sbxm task queue <kind> ...` (issue 529): adds one job to the loop's queue (decision 309,
//! [`jobs`]) after the checks the loop makes when it takes that job, refusing at once with the
//! reason and writing nothing. The job is serialized from [`Job`], the type the loop parses, to
//! `<base>/.sbxm/queue/<UTC time>-<kind>-<target>.json`, written as `.json.tmp` and renamed. A job
//! of the same kind and target waiting in `queue/` or `queue/running/` is refused, naming its
//! file. The duplicate check, the file name and the write happen under one lock,
//! `<base>/.sbxm/queue-lock/lock.json` (review M-1), so two commands that overlap can neither
//! both queue the same job nor share or overwrite a file name. The loop checks the job again when
//! it takes it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::task_loop::reasons;
use super::task_start::resolve_repo;
use crate::config::GlobalConfig;
use crate::github::{GitHubBackend, PrState};
use crate::run::results::format_timestamp;
use crate::task::finish::check_own_folder;
use crate::task::jobs::{self, Job, Running};
use crate::task::lock::{self, ProcessLock};
use crate::task::record::ProcessProbe;

/// How many seconds `add` waits at most for a file name no other job took, as the loop's log name.
const NAME_TRIES: u32 = 5;

/// How often, and how long apart, `add` looks again at a queue lock another command holds: 10
/// seconds in all, more than a name wait.
const LOCK_TRIES: u32 = 200;
const LOCK_PAUSE: Duration = Duration::from_millis(50);

pub struct Options {
    /// The checkout whose origin names the GitHub repo, as the loop's.
    pub repo_root: PathBuf,
    /// For tests: the GitHub repo, `owner/name`, instead of the checkout's origin.
    pub repo: Option<String>,
}

/// Queues `job`: checks it as the loop would, refuses a duplicate, then writes its file and
/// prints its path. Returns the path.
#[allow(clippy::too_many_arguments)]
pub fn add(
    config_dir: &Path,
    opts: &Options,
    job: &Job,
    github: &dyn GitHubBackend,
    probe: &dyn ProcessProbe,
    now: &dyn Fn() -> u64,
    sleep: &dyn Fn(Duration),
    out: &mut dyn Write,
) -> Result<PathBuf> {
    // Through the loop's parser, so a job it would refuse as a shape is never written.
    let text = serde_json::to_string_pretty(job)?;
    jobs::parse(&text).map_err(|why| anyhow!("{why}"))?;
    let config = GlobalConfig::load(config_dir)?;
    let base_dir = &config.base_dir;
    let sbxm_dir = base_dir.join(".sbxm");
    if fs::symlink_metadata(&sbxm_dir).is_ok() {
        check_own_folder(base_dir, &sbxm_dir)?;
    }
    jobs::check_queue(base_dir)?;
    check(config_dir, &config, opts, job, github)?;

    // Held to the end of the write: the duplicate check and the name stay true until then.
    let _lock = queue_lock(base_dir, probe)?;
    let name = free_name(base_dir, job, now, sleep)?;

    let queue = jobs::queue_dir(base_dir);
    fs::create_dir_all(&queue).with_context(|| format!("cannot create {}", queue.display()))?;
    check_own_folder(base_dir, &queue)?;
    let path = queue.join(&name);
    let tmp = queue.join(format!("{name}.tmp"));
    // Never over a file this command didn't create, and none of its own left behind.
    let mut created = false;
    let written = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        created = true;
        file.write_all(format!("{text}\n").as_bytes())
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    })();
    if let Err(e) = written {
        if created {
            let _ = fs::remove_file(&tmp);
        }
        return Err(e);
    }
    writeln!(out, "{}: queued in {}", job.label(), path.display())?;
    Ok(path)
}

/// The loop's own checks of `job` (issue 529, answer 3), each refusing with its reason.
fn check(
    config_dir: &Path,
    config: &GlobalConfig,
    opts: &Options,
    job: &Job,
    github: &dyn GitHubBackend,
) -> Result<()> {
    match job {
        Job::Run { issue } => {
            let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
            let open = github.issues_open(&repo)?;
            let Some(found) = open.iter().find(|i| i.number == *issue) else {
                bail!("#{issue} is not an open issue of {repo}; not queued");
            };
            let reasons = reasons(config_dir, &repo, github, &open, found)?;
            if !reasons.is_empty() {
                bail!(
                    "the loop would skip #{issue}: {}; not queued",
                    reasons.join("; also: ")
                );
            }
        }
        Job::Review { pr } | Job::Fix { pr, .. } => {
            let repo = resolve_repo(opts.repo.as_deref(), &opts.repo_root)?;
            if jobs::opener(&config.base_dir, &repo, *pr).is_none() {
                bail!(
                    "no task record of {repo} names PR #{pr} as its PR, live or archived, so \
                     sbxm didn't open it and the loop would refuse the job; not queued"
                );
            }
            match github.pr(&repo, *pr)?.state {
                PrState::Open => {}
                PrState::Merged => bail!("PR #{pr} is merged; not queued"),
                PrState::Closed => bail!("PR #{pr} is closed; not queued"),
            }
        }
        Job::Spec { file } => {
            jobs::spec_file(config_dir, config, file)
                .map_err(|why| anyhow!("{why}; not queued"))?;
        }
        Job::Template { profile, .. } => {
            jobs::template_profile(config, profile).map_err(|why| anyhow!("{why}; not queued"))?;
        }
        Job::RmMerged {} => {}
    }
    Ok(())
}

/// Refuses `job` when a job of the same kind and target waits in `queue/` or runs in
/// `queue/running/`, naming that file.
fn refuse_duplicate(base_dir: &Path, job: &Job) -> Result<()> {
    let files = jobs::pending(base_dir)?
        .into_iter()
        .chain(jobs::left_running(base_dir)?);
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let queued = Running::read(&text)
            .read
            .job
            .and_then(|value| jobs::parse(&value.to_string()).ok());
        if queued.is_some_and(|queued| queued.same_target(job)) {
            bail!(
                "{} is already queued as {}; not queued again",
                job.label(),
                path.display()
            );
        }
    }
    Ok(())
}

/// Takes the lock every `task queue` holds from its duplicate check to its file's rename,
/// waiting while another command holds it (each holds it for a moment, at most the
/// [`NAME_TRIES`] seconds of a name wait).
fn queue_lock(base_dir: &Path, probe: &dyn ProcessProbe) -> Result<ProcessLock> {
    let dir = base_dir.join(".sbxm").join("queue-lock");
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    check_own_folder(base_dir, &dir)?;
    let mut tries = 0;
    loop {
        match ProcessLock::acquire(&dir, "the loop's job queue", probe) {
            Err(e) if e.is::<lock::InUse>() && tries < LOCK_TRIES => {
                tries += 1;
                std::thread::sleep(LOCK_PAUSE);
            }
            result => return result,
        }
    }
}

/// `<UTC time>-<kind>-<target>.json`, a name no job in `queue/` or `queue/running/` has: one
/// taken waits for the next second, at most [`NAME_TRIES`] times. Each try first refuses a
/// duplicate of `job`, so one queued during a wait is found too.
fn free_name(
    base_dir: &Path,
    job: &Job,
    now: &dyn Fn() -> u64,
    sleep: &dyn Fn(Duration),
) -> Result<String> {
    for _ in 0..NAME_TRIES {
        refuse_duplicate(base_dir, job)?;
        let stamp: String = format_timestamp(now())
            .chars()
            .filter(|c| !matches!(c, '-' | ':'))
            .collect();
        let name = format!("{stamp}-{}.json", job.file_part());
        let taken = [jobs::queue_dir(base_dir), jobs::running_dir(base_dir)]
            .iter()
            .any(|dir| fs::symlink_metadata(dir.join(&name)).is_ok());
        if !taken {
            return Ok(name);
        }
        sleep(Duration::from_secs(1));
    }
    bail!(
        "every job file name of the last {NAME_TRIES} seconds for {} is taken in {}; try again",
        job.label(),
        jobs::queue_dir(base_dir).display()
    )
}
