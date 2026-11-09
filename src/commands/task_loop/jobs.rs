//! The loop's queued jobs (issue 525, decision 309): before each new issue, after its conflict
//! resolves and its checks of the runs, the loop runs every job file in `<base>/.sbxm/queue/`,
//! oldest first ([`jobs::pending`]), each ending with one result file. At its start it reports
//! the jobs a stopped loop left in `queue/running/` as `interrupted`, without running them again.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, anyhow};

use super::{End, Halt, Loop, NeedsUser, check, first_line, needs_user, sign_in_stop};
use crate::github::PrState;
use crate::run::results::format_timestamp;
use crate::task::jobs::{self, AsRead, Job, JobResult, Outcome, Running};
use crate::task::next;
use crate::task::queue::{self, Row};
use crate::task::record::{self, Kind, Record, Stage};

/// How one job ended, before its result file.
struct Ended {
    outcome: Outcome,
    reason: String,
    /// The task it started, when it got that far.
    task: Option<String>,
    /// What stops the whole loop after the job's result is written (a run job's sign-in, usage
    /// limit or error, as for a task the loop picked itself).
    halt: Option<anyhow::Error>,
}

impl Ended {
    fn new(outcome: Outcome, reason: impl Into<String>) -> Self {
        Self {
            outcome,
            reason: reason.into(),
            task: None,
            halt: None,
        }
    }

    fn task(mut self, id: String) -> Self {
        self.task = Some(id);
        self
    }
}

/// The next step sbxm names for `rec`: none for a finished task, its `ready` step, or the command
/// that continues it (issue 512).
fn next_of(rec: &Record) -> Option<String> {
    match rec.stage {
        Stage::Finished => None,
        Stage::Ready => next::ready(rec, &next::record_flag(rec)),
        _ => Some(next::resume(rec)),
    }
}

impl Loop<'_> {
    /// Runs every queued job, oldest first, a job written meanwhile too. `Ok(false)` when a stop
    /// was asked between jobs: the rest stay queued.
    pub(super) fn drain_jobs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<bool> {
        loop {
            let pending = jobs::pending(&self.base_dir)?;
            let Some(path) = pending.first() else {
                return Ok(true);
            };
            if (self.env.stop_requested)() {
                return Ok(false);
            }
            self.job(path, out, warn)?;
        }
    }

    /// Reports each job a stopped loop left in `running/` as `interrupted` (answer 6), naming the
    /// task it started, that task's state and the command that continues it; never runs it
    /// again. Called under the loop's lock, so no other loop is running them.
    pub(super) fn interrupted_jobs(
        &mut self,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        for taken in jobs::left_running(&self.base_dir)? {
            let text = std::fs::read(&taken)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            let running = Running::read(&text);
            let label = running
                .read
                .job
                .as_ref()
                .and_then(|v| serde_json::from_value::<Job>(v.clone()).ok())
                .map_or_else(|| file_label(&taken), |job| job.label());
            // A task id only, never a path, whatever the file says.
            let rec = running
                .task
                .as_deref()
                .filter(|id| record::is_valid_id(id))
                .and_then(|id| {
                    record::read(&record::task_dir(&self.base_dir, id).join("task.json")).ok()
                });
            let reason = match &rec {
                Some(rec) => format!(
                    "the loop stopped while it ran; it started task {} ({} {}), not run again",
                    rec.id,
                    rec.stage.name(),
                    rec.status.name()
                ),
                None => "the loop stopped while it ran, before it started a task; not run again"
                    .to_owned(),
            };
            let now = format_timestamp(self.now());
            let started_at = if running.started_at.is_empty() {
                now.clone()
            } else {
                running.started_at.clone()
            };
            let result = JobResult {
                read: running.read.clone(),
                outcome: Outcome::Interrupted,
                reason,
                started_at,
                ended_at: now,
                task: rec.as_ref().map(|r| r.id.clone()),
                url: rec.as_ref().and_then(|r| self.url_of(r)),
                next: rec.as_ref().and_then(next_of),
            };
            self.ended(&taken, &label, &result, self.now(), out, warn)?;
        }
        Ok(())
    }

    /// One job, from its file in the queue folder to its result file.
    fn job(&mut self, path: &Path, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        let (taken, text) = jobs::take(&self.base_dir, path)?;
        let started = self.now();
        let read = AsRead::new(text.as_deref().unwrap_or_default());
        let (label, ended) = match text.and_then(|text| jobs::parse(&text)) {
            Err(why) => (file_label(&taken), Ended::new(Outcome::Refused, why)),
            Ok(job) => {
                writeln!(out, "{}: started", job.label())?;
                let ended = self.run_job(&job, &taken, &read, started, out, warn)?;
                (job.label(), ended)
            }
        };
        let rec = ended.task.as_deref().and_then(|id| {
            record::read(&record::task_dir(&self.base_dir, id).join("task.json")).ok()
        });
        let result = JobResult {
            read,
            outcome: ended.outcome,
            reason: ended.reason,
            started_at: format_timestamp(started),
            ended_at: format_timestamp(self.now()),
            task: rec.as_ref().map(|r| r.id.clone()),
            url: rec.as_ref().and_then(|r| self.url_of(r)),
            next: rec.as_ref().and_then(next_of),
        };
        self.ended(&taken, &label, &result, started, out, warn)?;
        match ended.halt {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The PR a task produced or works on: the PR its finish opened, or a PR or fix task's own.
    fn url_of(&self, rec: &Record) -> Option<String> {
        match rec.kind {
            Kind::Pr | Kind::Fix => Some(format!(
                "https://github.com/{}/pull/{}",
                rec.repo, rec.number
            )),
            Kind::Issue | Kind::Spec => rec.pr.clone(),
        }
    }

    /// Writes a job's result file and its log row, and says how it ended.
    fn ended(
        &mut self,
        taken: &Path,
        label: &str,
        result: &JobResult,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let path = jobs::end(&self.base_dir, taken, result)?;
        writeln!(
            out,
            "{label}: {}: {}; result in {}",
            result.outcome.name(),
            result.reason,
            path.display()
        )?;
        let row = Row {
            issue: 0,
            title: String::new(),
            size: None,
            started,
            ready: None,
            worker_s: None,
            reviewer_s: None,
            rounds: None,
            must_fix: None,
            outcome: format!("{}: {}", result.outcome.name(), first_line(&result.reason)),
            stops: Vec::new(),
            idle_before: None,
        };
        if let Err(e) = queue::append_job_row(&self.log, label, &row) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that log row"),
            )?;
        } else {
            self.logged = true;
        }
        Ok(())
    }

    /// Marks the running file with the task the job starts, so a stop meanwhile names it.
    fn mark(&self, taken: &Path, read: &AsRead, started: u64, task: Option<String>) -> Result<()> {
        jobs::mark(
            taken,
            &Running {
                read: read.clone(),
                started_at: format_timestamp(started),
                task,
            },
        )
    }

    /// Runs a parsed job; an error is the loop's own (a file it can't write), the job's failures
    /// are in [`Ended`].
    fn run_job(
        &mut self,
        job: &Job,
        taken: &Path,
        read: &AsRead,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        match *job {
            Job::Review { pr } => self.pr_job(pr, None, false, taken, read, started, out, warn),
            Job::Fix { pr, check_run } => {
                self.pr_job(pr, check_run, true, taken, read, started, out, warn)
            }
            Job::Run { issue } => self.run_issue(issue, taken, read, started, out, warn),
            Job::RmMerged {} => {
                self.mark(taken, read, started, None)?;
                Ok(match self.steps.rm_merged(out, warn) {
                    Ok(()) => Ended::new(Outcome::Done, "merged tasks removed"),
                    Err(e) => Ended::new(Outcome::Failed, format!("{e:#}")),
                })
            }
        }
    }

    /// A `review` or (`fix`) `fix` job of PR `pr`: only of a PR sbxm opened (answer 4), skipped
    /// when it is merged or closed.
    #[allow(clippy::too_many_arguments)]
    fn pr_job(
        &mut self,
        pr: u32,
        check_run: Option<u64>,
        fix: bool,
        taken: &Path,
        read: &AsRead,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        let command = if fix { "fix" } else { "review" };
        if jobs::opener(&self.base_dir, &self.repo, pr).is_none() {
            return Ok(Ended::new(
                Outcome::Refused,
                format!(
                    "no task record of {} names PR #{pr} as its PR, live or archived, so sbxm \
                     didn't open it; run `sbxm task {command} --pr {pr}` by hand instead",
                    self.repo
                ),
            ));
        }
        let info = match self.github.pr(&self.repo, pr) {
            Ok(info) => info,
            Err(e) => {
                return Ok(Ended::new(
                    Outcome::Failed,
                    format!("PR #{pr} could not be read: {e:#}"),
                ));
            }
        };
        match info.state {
            PrState::Merged => {
                return Ok(Ended::new(Outcome::Skipped, format!("PR #{pr} is merged")));
            }
            PrState::Closed => {
                return Ok(Ended::new(Outcome::Skipped, format!("PR #{pr} is closed")));
            }
            PrState::Open => {}
        }
        let task = if fix {
            record::fix_id(pr, &info.head_oid)
        } else {
            Some(format!("pr-{pr}"))
        };
        self.mark(taken, read, started, task.clone())?;
        let result = match (fix, check_run) {
            (false, _) => self.steps.review_pr(pr, out, warn),
            (true, None) => self.steps.fix_pr(pr, out, warn),
            (true, Some(run)) => self.steps.fix_check(pr, run, out, warn),
        };
        let ended = match result {
            Ok(()) => Ended::new(Outcome::Done, format!("`task {command} --pr {pr}` ended")),
            Err(e) => Ended::new(Outcome::Failed, format!("{e:#}")),
        };
        Ok(match task {
            Some(id) => ended.task(id),
            None => ended,
        })
    }

    /// A `run` job: an open issue the loop would take (`task loop --check`'s reasons), then the
    /// task as the loop runs one it picked itself, to its PR or its parking.
    fn run_issue(
        &mut self,
        issue: u32,
        taken: &Path,
        read: &AsRead,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        let open = match self.github.issues_open(&self.repo) {
            Ok(open) => open,
            Err(e) => {
                return Ok(Ended::new(
                    Outcome::Failed,
                    format!("the open issues could not be read: {e:#}"),
                ));
            }
        };
        let Some(found) = open.iter().find(|i| i.number == issue) else {
            return Ok(Ended::new(
                Outcome::Refused,
                format!("#{issue} is not an open issue of {}", self.repo),
            ));
        };
        let reasons = match check::reasons(self.config_dir, &self.repo, self.github, &open, found) {
            Ok(reasons) => reasons,
            Err(e) => {
                return Ok(Ended::new(
                    Outcome::Failed,
                    format!("#{issue} could not be checked: {e:#}"),
                ));
            }
        };
        if !reasons.is_empty() {
            return Ok(Ended::new(Outcome::Refused, reasons.join("; ")));
        }
        let id = format!("issue-{issue}");
        self.mark(taken, read, started, Some(id.clone()))?;
        let mut stops = Vec::new();
        let ended = match self.task(issue, true, 0, &mut stops, out, warn) {
            Ok(End::Done { outcome, .. }) => Ended::new(Outcome::Done, outcome),
            Ok(End::Parked { .. }) if (self.env.stop_requested)() => {
                Ended::new(Outcome::Stopped, "stopped on request")
            }
            Ok(End::Parked { outcome, .. }) => Ended::new(Outcome::Failed, outcome),
            Ok(End::Refused(reason)) => Ended::new(Outcome::Refused, reason),
            Ok(End::Held(_)) => Ended::new(
                Outcome::Failed,
                "its lock is held by a command run by hand; queue it again once that ends",
            ),
            Ok(End::Taken(line)) => Ended::new(Outcome::Skipped, line),
            Err(Halt::Stopped) => Ended::new(Outcome::Stopped, "stopped on request"),
            Err(Halt::Error(e)) => Ended {
                // A missing secret stays a stop that needs the user (decision 310).
                halt: Some(if e.is::<NeedsUser>() {
                    needs_user(anyhow!("{e:#}"))
                } else {
                    anyhow!("{e:#}")
                }),
                ..Ended::new(Outcome::Failed, format!("{e:#}"))
            },
            Err(Halt::SignIn(e)) => {
                let exists = self.record(issue).is_some();
                let stop = sign_in_stop(e, issue, exists);
                Ended {
                    halt: Some(needs_user(anyhow!("{stop:#}"))),
                    ..Ended::new(Outcome::Stopped, format!("{stop:#}"))
                }
            }
            Err(Halt::UsageLimit(total)) => {
                let stop = format!(
                    "the usage limit held through {} of waits in a row; the loop stops; continue \
                     with `sbxm task resume --issue {issue}` once it resets",
                    queue::span(total.as_secs())
                );
                Ended {
                    halt: Some(needs_user(anyhow!("{stop}"))),
                    ..Ended::new(Outcome::Stopped, stop)
                }
            }
        };
        Ok(ended.task(id))
    }
}

/// A job file's text in the log when it isn't a job: `job <file name>`.
fn file_label(taken: &Path) -> String {
    let name = taken
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("job {name}")
}
