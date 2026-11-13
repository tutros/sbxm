//! The loop's queued jobs (issue 525, decision 309): before each new issue, after its conflict
//! resolves and its checks of the runs, the loop runs every job file in `<base>/.sbxm/queue/`,
//! oldest first ([`jobs::pending`]), each ending with one result file. At its start it reports
//! the jobs a stopped loop left in `queue/running/` as `interrupted`, without running them again.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::job_children::{JobChild, Work};
use super::{
    Capture, Cause, Claimed, End, Halt, JobStep, Loop, NeedsUser, PhaseEnd, Steps, check,
    first_line, needs_user, says_sign_in, sign_in_stop,
};
use crate::backend::Location;
use crate::commands::task_start::spec_flag;
use crate::config::GlobalConfig;
use crate::github::PrState;
use crate::harness::Harness;
use crate::notify::Notification;
use crate::run::results::format_timestamp;
use crate::task::jobs::{self, AsRead, Job, JobHarness, JobResult, Outcome, Running};
use crate::task::outcome::Kind as ChildKind;
use crate::task::queue::{self, Row};
use crate::task::record::{self, Kind, Record, Stage};
use crate::task::{location, lock, next, pipeline};

/// How one job ended, before its result file.
struct Ended {
    outcome: Outcome,
    reason: String,
    /// The task it started, when it got that far.
    task: Option<String>,
    /// What stops the whole loop after the job's result is written (a run job's sign-in, usage
    /// limit or error, as for a task the loop picked itself).
    halt: Option<anyhow::Error>,
    /// The flag naming the task in its `next` step, when its record's own can't (a spec task's
    /// real path, issue 524).
    flag: Option<String>,
}

impl Ended {
    fn new(outcome: Outcome, reason: impl Into<String>) -> Self {
        Self {
            outcome,
            reason: reason.into(),
            task: None,
            halt: None,
            flag: None,
        }
    }

    fn task(mut self, id: String) -> Self {
        self.task = Some(id);
        self
    }
}

/// The next step sbxm names for `rec`: none for a finished task, its `ready` step, or the command
/// that continues it (issue 512). `flag` names the task instead of its record's own flag (a spec
/// task's real path).
fn next_of(rec: &Record, flag: Option<&str>) -> Option<String> {
    match (rec.stage, flag) {
        (Stage::Finished, _) => None,
        (Stage::Ready, Some(flag)) => next::ready(rec, flag),
        (Stage::Ready, None) => next::ready(rec, &next::record_flag(rec)),
        (_, Some(flag)) => Some(format!("sbxm task resume {flag}")),
        (_, None) => Some(next::resume(rec)),
    }
}

impl Loop<'_> {
    /// Runs every queued job, oldest first, a job written meanwhile too. `Ok(false)` when a stop
    /// was asked between jobs: the rest stay queued.
    pub(super) fn drain_jobs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<bool> {
        if self.slots.is_some() {
            return self.start_jobs(out, warn);
        }
        loop {
            let pending = jobs::pending(&self.base_dir)?;
            let Some(path) = pending.first() else {
                return Ok(true);
            };
            if (self.env.stop_requested)() {
                return Ok(false);
            }
            // A job whose slot a child holds, or that a child's usage limit holds back, waits for
            // a later pass, the jobs after it too, so they still run oldest first (issue 615).
            if let Some((label, at, task)) = self.job_slot(path)
                && let Some(waits) = self.wait_reason(at, task)
            {
                let line = format!("{label}: waits for {waits}; tried again at a later pass");
                self.say_once(out, line)?;
                return Ok(true);
            }
            // A stop asked during the reads that found its slot: it stays queued.
            if (self.env.stop_requested)() {
                return Ok(false);
            }
            self.job(path, out, warn)?;
        }
    }

    /// [`Self::drain_jobs`] for a loop with several children (issue 633): each queued job,
    /// oldest first, started as a child while its slot is free, ahead of new issues (317(7)). A
    /// job whose slot is taken waits, and the jobs after it in the same place wait too, so each
    /// place runs its jobs oldest first; a job on a PR a job child runs on waits, keeping its
    /// place, and the jobs after it that fit start. `rm-merged`, and a file that isn't a job,
    /// run here at once. `Ok(false)` when a stop was asked: the rest stay queued.
    fn start_jobs(&mut self, out: &mut dyn Write, warn: &mut dyn Write) -> Result<bool> {
        let mut held: Vec<Location> = Vec::new();
        for path in jobs::pending(&self.base_dir)? {
            if self.stopping() {
                return Ok(false);
            }
            let job = peek(&path);
            if let Some(Job::Review { pr } | Job::Fix { pr, .. }) = job
                && self.pr_busy(pr)
            {
                let line = format!(
                    "{}: waits for the job running on PR #{pr}; tried again at a later pass",
                    job.as_ref().map(Job::label).unwrap_or_default()
                );
                self.say_once(out, line)?;
                continue;
            }
            let slot = match self.job_slot(&path) {
                Some((label, at, task)) => {
                    let place = at.unwrap_or(self.default_location);
                    if held.contains(&place) {
                        continue;
                    }
                    if let Some(waits) = self.wait_reason(at, task) {
                        let line =
                            format!("{label}: waits for {waits}; tried again at a later pass");
                        self.say_once(out, line)?;
                        held.push(place);
                        continue;
                    }
                    Some(place)
                }
                None => None,
            };
            // A stop asked during the reads that found its slot: it stays queued.
            if self.stopping() {
                return Ok(false);
            }
            match (job, slot) {
                (
                    Some(
                        Job::Review { .. }
                        | Job::Fix { .. }
                        | Job::Template { .. }
                        | Job::Spec { .. },
                    ),
                    Some(slot),
                ) => {
                    self.start_queued(&path, slot, out, warn)?;
                }
                _ => self.job(&path, out, warn)?,
            }
        }
        Ok(true)
    }

    /// Starts queued job `path` as a child in `slot` (issue 633), after the checks its run makes
    /// first; one that ends at those checks, or that isn't a child's, ends here.
    fn start_queued(
        &mut self,
        path: &Path,
        slot: Location,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let (taken, text) = jobs::take(&self.base_dir, path)?;
        let started = self.now();
        let read = AsRead::new(text.as_deref().unwrap_or_default());
        let job = match text.and_then(|text| jobs::parse(&text)) {
            Ok(job) => job,
            Err(why) => {
                let label = file_label(&taken);
                let ended = Ended::new(Outcome::Refused, why);
                return self.halt_after(&taken, &label, read, started, ended, out, warn);
            }
        };
        let label = job.label();
        writeln!(out, "{label}: started")?;
        let (step, what) = match &job {
            Job::Review { pr } | Job::Fix { pr, .. } => {
                let (pr, fix) = (*pr, matches!(job, Job::Fix { .. }));
                let task = match self.pr_job_ready(pr, fix) {
                    Ok(task) => task,
                    Err(ended) => {
                        return self.halt_after(&taken, &label, read, started, ended, out, warn);
                    }
                };
                self.mark(&taken, &read, started, task.clone())?;
                let step = match job {
                    Job::Fix { check_run, .. } => JobStep::Fix { pr, check_run },
                    _ => JobStep::Review { pr },
                };
                (step, QueuedWhat::Pr { pr, fix, task })
            }
            Job::Template {
                profile,
                harness,
                warm,
            } => {
                if let Err(ended) = self.template_ready(profile) {
                    return self.halt_after(&taken, &label, read, started, ended, out, warn);
                }
                self.mark(&taken, &read, started, None)?;
                let step = JobStep::Template {
                    profile: profile.clone(),
                    harness: harness.harness(),
                    warm: warm.then(|| self.repo_root.clone()),
                };
                let what = QueuedWhat::Template {
                    profile: profile.clone(),
                    harness: harness.harness(),
                };
                (step, what)
            }
            Job::Spec { file } => {
                let (spec, id) = match self.spec_ready(file) {
                    Ok(found) => found,
                    Err(ended) => {
                        return self.halt_after(&taken, &label, read, started, ended, out, warn);
                    }
                };
                self.mark(&taken, &read, started, Some(id.clone()))?;
                let step = JobStep::RunSpec(spec.clone());
                let what = QueuedWhat::Spec {
                    spec,
                    id,
                    should_fix: false,
                };
                (step, what)
            }
            _ => {
                let ended = self.run_job(&job, &taken, &read, started, out, warn)?;
                return self.halt_after(&taken, &label, read, started, ended, out, warn);
            }
        };
        let queued = Queued {
            taken,
            label,
            read,
            started,
            slot,
            what,
        };
        self.start_queued_step(queued, &step, out, warn);
        Ok(())
    }

    /// Starts `step` of `queued` as its child.
    fn start_queued_step(
        &mut self,
        queued: Queued,
        step: &JobStep,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) {
        let pr = match queued.what {
            QueuedWhat::Pr { pr, .. } => Some(pr),
            QueuedWhat::Template { .. } | QueuedWhat::Spec { .. } => None,
        };
        let slot = queued.slot;
        let phase = self.steps.start_job(step, slot, out, warn);
        self.jobs.push(JobChild {
            work: Work::Queued(queued),
            pr,
            slot,
            phase,
        });
    }

    /// [`Self::job_done`], its stop kept for the loop: draining, the running children first.
    #[allow(clippy::too_many_arguments)]
    fn halt_after(
        &mut self,
        taken: &Path,
        label: &str,
        read: AsRead,
        started: u64,
        ended: Ended,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        match self.job_done(taken, label, read, started, ended, out, warn)? {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// What a queued job's child's end does (issue 633): its result file, its log row and its
    /// line, as for one run here. `Some` is the stop of the loop it asks for.
    pub(super) fn queued_ended(
        &mut self,
        queued: Queued,
        end: &PhaseEnd,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<anyhow::Error>> {
        let ended = match &queued.what {
            QueuedWhat::Spec {
                spec,
                id,
                should_fix,
            } => {
                let (spec, id, should_fix) = (spec.clone(), id.clone(), *should_fix);
                let flag = spec_flag(&spec);
                let resume = Some((id.as_str(), format!("sbxm task resume {flag}")));
                let what = if should_fix {
                    "its should-fix round failed: "
                } else {
                    ""
                };
                let ended = match self.child_failed(what, resume, end) {
                    Some(ended) => ended,
                    // Its should-fix round next, as another child (decision 306).
                    None if !should_fix && self.should_fix_due_of(&id) => {
                        let step = JobStep::ShouldFixSpec(spec.clone());
                        let queued = Queued {
                            what: QueuedWhat::Spec {
                                spec,
                                id,
                                should_fix: true,
                            },
                            ..queued
                        };
                        self.start_queued_step(queued, &step, out, warn);
                        return Ok(None);
                    }
                    None => self.spec_finish(&spec, &id, out, warn)?,
                };
                Ended {
                    flag: Some(flag),
                    ..ended.task(id)
                }
            }
            QueuedWhat::Pr { pr, fix, task } => {
                let (pr, fix, task) = (*pr, *fix, task.clone());
                let command = if fix { "fix" } else { "review" };
                let ended = match end.kind() {
                    ChildKind::Ready => {
                        Ended::new(Outcome::Done, format!("`task {command} --pr {pr}` ended"))
                    }
                    _ => Ended::new(Outcome::Failed, format!("{:#}", end.error())),
                };
                match task {
                    Some(id) => ended.task(id),
                    None => ended,
                }
            }
            QueuedWhat::Template { profile, harness } => match self.child_failed("", None, end) {
                Some(ended) => ended,
                None => {
                    let key = crate::template::key(profile, *harness);
                    let built = match crate::template::Record::load(&self.base_dir, &key) {
                        Ok(Some(rec)) => format!("built template {}", rec.tag),
                        _ => format!("built the template of {profile}"),
                    };
                    Ended::new(Outcome::Done, built)
                }
            },
        };
        self.job_done(
            &queued.taken,
            &queued.label,
            queued.read,
            queued.started,
            ended,
            out,
            warn,
        )
    }

    /// How a job's child ended, as [`Self::phase`] reads a phase run here: `None` when ready; a
    /// lapsed sign-in is `stopped` and stops the loop; a missing secret is `failed` and stops it;
    /// anything else is `failed`, with `what` before the error. `resume`: the task the child
    /// works on and the command that continues it, named in a sign-in stop once its record
    /// exists.
    fn child_failed(
        &self,
        what: &str,
        resume: Option<(&str, String)>,
        end: &PhaseEnd,
    ) -> Option<Ended> {
        let e = end.error();
        match end.kind() {
            ChildKind::Ready => None,
            ChildKind::SignedOut => {
                let resume = resume.and_then(|(id, command)| self.record_of(id).map(|_| command));
                let stop = sign_in_stop(e, resume);
                Some(Ended {
                    halt: Some(needs_user(Cause::SignIn, anyhow!("{stop:#}"))),
                    ..Ended::new(Outcome::Stopped, format!("{stop:#}"))
                })
            }
            ChildKind::MissingSecret => Some(Ended {
                halt: Some(needs_user(Cause::Secret, anyhow!("{e:#}"))),
                ..Ended::new(Outcome::Failed, format!("{what}{e:#}"))
            }),
            _ => Some(Ended::new(Outcome::Failed, format!("{what}{e:#}"))),
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
            let job = running
                .read
                .job
                .as_ref()
                .and_then(|v| jobs::parse(&v.to_string()).ok());
            let label = job.as_ref().map_or_else(|| file_label(&taken), Job::label);
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
                next: rec.as_ref().and_then(|rec| {
                    let flag = match &job {
                        Some(Job::Spec { file }) => self.spec_flag_of(file, rec),
                        _ => None,
                    };
                    next_of(rec, flag.as_deref())
                }),
            };
            self.ended(&taken, &label, &result, self.now(), out, warn)?;
        }
        Ok(())
    }

    /// The `--spec` flag of a spec job's `file` in `specs_dir`, when it is still `rec`'s spec
    /// (`specs_dir` may have changed since the job started).
    fn spec_flag_of(&self, file: &str, rec: &Record) -> Option<String> {
        let spec = GlobalConfig::load(self.config_dir)
            .ok()?
            .specs_dir?
            .join(file);
        let (id, _) = record::spec_id(&spec).ok()?;
        (id == rec.id).then(|| spec_flag(&spec))
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
        match self.job_done(&taken, &label, read, started, ended, out, warn)? {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// A job's end, in-process or as a child: its result file, its log row and its line; `Some`
    /// is the stop of the loop it asked for, after its result is written.
    #[allow(clippy::too_many_arguments)]
    fn job_done(
        &mut self,
        taken: &Path,
        label: &str,
        read: AsRead,
        started: u64,
        ended: Ended,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<anyhow::Error>> {
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
            next: rec
                .as_ref()
                .and_then(|rec| next_of(rec, ended.flag.as_deref())),
        };
        self.ended(taken, label, &result, started, out, warn)?;
        Ok(ended.halt)
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
            slot: None,
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
        self.notify_job(taken, label, result, out, warn)?;
        // The task a job ran may hand a question back whatever its outcome (review M-2).
        if let Some(id) = result.task.as_deref() {
            self.notify_question(id, out, warn)?;
        }
        if let Err(e) = queue::append_job_row(&self.log, &self.repo, label, &row) {
            self.warn_once(
                warn,
                format!("{e:#}; the loop goes on without that log row"),
            )?;
        } else {
            self.logged = true;
        }
        Ok(())
    }

    /// (b) of issue 530: a notification for a job that ended `failed`, `stopped`, `refused` or
    /// `interrupted`, keyed by its file name. The click opens the PR or issue the job names.
    fn notify_job(
        &mut self,
        taken: &Path,
        label: &str,
        result: &JobResult,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        if matches!(result.outcome, Outcome::Done | Outcome::Skipped) {
            return Ok(());
        }
        let file = taken
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let named = |field: &str| {
            result
                .read
                .job
                .as_ref()
                .and_then(|job| job.get(field))
                .and_then(serde_json::Value::as_u64)
        };
        let click = result
            .url
            .clone()
            .or_else(|| named("pr").map(|n| format!("https://github.com/{}/pull/{n}", self.repo)))
            .or_else(|| {
                named("issue").map(|n| format!("https://github.com/{}/issues/{n}", self.repo))
            })
            .unwrap_or_else(|| self.log_click());
        let mut title = label.to_owned();
        if let Some(first) = title.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        let next = match &result.next {
            Some(next) => format!("; next: `{next}`"),
            None => String::new(),
        };
        let note = Notification {
            key: format!("job:{file}"),
            title: format!("{title} {}", result.outcome.name()),
            body: format!("{}{next}", first_line(&result.reason)),
            click,
        };
        self.notify(note, out, warn)
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
            Job::Template {
                ref profile,
                harness,
                warm,
            } => self.template_job(profile, harness, warm, taken, read, started, out, warn),
            Job::Spec { ref file } => self.spec_job(file, taken, read, started, out, warn),
        }
    }

    /// A `spec` job (issue 524, answers 1 and 2): a plain existing file in `specs_dir`, run with
    /// `task run --spec`; then, as for the loop's issues, a task ready with 0 must-fix gets its
    /// should-fix round (decision 306), its finish and its auto-merge (decision 293). A task
    /// that stops short of ready, or with must-fix left, is not finished: `stopped`.
    fn spec_job(
        &mut self,
        file: &str,
        taken: &Path,
        read: &AsRead,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        let (spec, id) = match self.spec_ready(file) {
            Ok(found) => found,
            Err(ended) => return Ok(ended),
        };
        self.mark(taken, read, started, Some(id.clone()))?;
        let ended = self.spec_task(&spec, &id, out, warn)?;
        Ok(Ended {
            flag: Some(spec_flag(&spec)),
            ..ended.task(id)
        })
    }

    /// A `spec` job's file `file` in `specs_dir` and its task's id; `Err`: how the job ends
    /// without running.
    fn spec_ready(&self, file: &str) -> std::result::Result<(PathBuf, String), Ended> {
        let config = GlobalConfig::load(self.config_dir)
            .map_err(|e| Ended::new(Outcome::Failed, format!("{e:#}")))?;
        jobs::spec_file(self.config_dir, &config, file)
            .map_err(|why| Ended::new(Outcome::Refused, why))
    }

    /// A spec job's task `id` of file `spec`, from its run to its PR or its stop.
    fn spec_task(
        &mut self,
        spec: &Path,
        id: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        let flag = spec_flag(spec);
        let resume = Some((id, format!("sbxm task resume {flag}")));
        if let Err(ended) = self.phase("", resume.clone(), out, warn, |steps, out, warn| {
            steps.run_spec(spec, out, warn)
        }) {
            return Ok(ended);
        }
        if self.should_fix_due_of(id)
            && let Err(ended) = self.phase(
                "its should-fix round failed: ",
                resume,
                out,
                warn,
                |steps, out, warn| steps.should_fix_spec(spec, out, warn),
            )
        {
            return Ok(ended);
        }
        self.spec_finish(spec, id, out, warn)
    }

    /// A spec job's task `id` of file `spec` after its run and its should-fix round: finished,
    /// with its auto-merge, when it is ready and clean; `stopped` otherwise.
    fn spec_finish(
        &mut self,
        spec: &Path,
        id: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        let flag = spec_flag(spec);
        let Some(rec) = self.record_of(id) else {
            return Ok(Ended::new(
                Outcome::Failed,
                "`task run` ended without a task record",
            ));
        };
        if rec.stage != Stage::Ready {
            return Ok(Ended::new(
                Outcome::Stopped,
                format!(
                    "it stopped at {} ({}) short of ready; not finished",
                    rec.stage.name(),
                    rec.status.name()
                ),
            ));
        }
        let must_fix = rec.review.as_ref().map_or(0, |r| r.must_fix);
        if must_fix > 0 || rec.stopped.is_some() {
            let stop = rec
                .stopped
                .map_or_else(String::new, |s| format!(", work stop: {}", s.name()));
            return Ok(Ended::new(
                Outcome::Stopped,
                format!("ready with {must_fix} must-fix left{stop}; not finished"),
            ));
        }
        match self.claim(id) {
            Ok(Claimed::Marked) => {}
            Ok(Claimed::Finished(pr)) => {
                let into = pr.map_or_else(String::new, |url| format!(" into {url}"));
                return Ok(Ended::new(
                    Outcome::Skipped,
                    format!("finished by a command run by hand{into}"),
                ));
            }
            Err(e) if e.is::<lock::InUse>() => {
                return Ok(Ended::new(
                    Outcome::Failed,
                    "its lock is held by a command run by hand; finish it once that ends",
                ));
            }
            Err(e) => self.warn_once(
                warn,
                format!(
                    "task {id} could not be marked as the loop's ({e:#}); its PR won't count \
                     among the PRs awaiting you"
                ),
            )?,
        }
        let finish = Some((id, format!("sbxm task finish {flag}")));
        if let Err(ended) = self.phase(
            "its finish failed: ",
            finish,
            out,
            warn,
            |steps, out, warn| steps.finish_spec(spec, out, warn),
        ) {
            return Ok(ended);
        }
        let armed = self.arm(id, out, warn)?;
        self.notify_question(id, out, warn)?;
        let pr = self
            .record_of(id)
            .and_then(|r| r.pr)
            .and_then(|url| queue::pr_number(&url));
        let what = match (pr, armed) {
            (None, _) => "finished; its [finish] sink opened no PR".to_owned(),
            (Some(n), Some(note)) => format!("PR #{n}, clean, {note}"),
            (Some(n), None) => format!("PR #{n}, clean"),
        };
        Ok(Ended::new(Outcome::Done, what))
    }

    /// A `template` job (issue 524, answer 3): only of an existing profile with a `[template]`
    /// table, warmed from the loop's own checkout when `warm`. A low disk or a busy sandbox slot
    /// is `template build`'s own: it refuses, or waits for the slot.
    #[allow(clippy::too_many_arguments)]
    fn template_job(
        &mut self,
        profile: &str,
        harness: JobHarness,
        warm: bool,
        taken: &Path,
        read: &AsRead,
        started: u64,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Ended> {
        if let Err(ended) = self.template_ready(profile) {
            return Ok(ended);
        }
        self.mark(taken, read, started, None)?;
        let repo_root = self.repo_root.clone();
        let warm = warm.then_some(repo_root.as_path());
        Ok(
            match self.phase("", None, out, warn, |steps, out, warn| {
                steps.template_build(profile, harness.harness(), warm, out, warn)
            }) {
                Ok(tag) => Ended::new(Outcome::Done, format!("built template {tag}")),
                Err(ended) => ended,
            },
        )
    }

    /// Whether a `template` job of `profile` may run: `Err` is how it ends without running.
    fn template_ready(&self, profile: &str) -> std::result::Result<(), Ended> {
        let config = GlobalConfig::load(self.config_dir)
            .map_err(|e| Ended::new(Outcome::Failed, format!("{e:#}")))?;
        jobs::template_profile(&config, profile).map_err(|why| Ended::new(Outcome::Refused, why))
    }

    /// Runs one phase of a queued `spec` or `template` job (issue 524) with its output captured,
    /// and ends the job on its error as the loop ends a task's: a lapsed sign-in, `sbx`'s or a
    /// harness's, typed or only in the output (issue 445), is `stopped` and stops the loop; a
    /// missing secret (decision 193(a)) is `failed` and stops it; anything else is `failed`, with
    /// `what` before the error, and the loop goes on. `resume`: the task the phase works on and
    /// the command that continues it, named in a sign-in stop once that task's record exists.
    fn phase<T>(
        &self,
        what: &str,
        resume: Option<(&str, String)>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
        step: impl FnOnce(&dyn Steps, &mut dyn Write, &mut dyn Write) -> Result<T>,
    ) -> std::result::Result<T, Ended> {
        let mut captured = Vec::new();
        let result = {
            let mut out = Capture {
                inner: out,
                text: Vec::new(),
            };
            let mut warn = Capture {
                inner: warn,
                text: Vec::new(),
            };
            let result = step(self.steps, &mut out, &mut warn);
            captured.extend(out.text);
            captured.extend(warn.text);
            result
        };
        let e = match result {
            Ok(value) => return Ok(value),
            Err(e) => e,
        };
        if says_sign_in(&e, &String::from_utf8_lossy(&captured)) {
            let resume = resume.and_then(|(id, command)| self.record_of(id).map(|_| command));
            let stop = sign_in_stop(e, resume);
            return Err(Ended {
                halt: Some(needs_user(Cause::SignIn, anyhow!("{stop:#}"))),
                ..Ended::new(Outcome::Stopped, format!("{stop:#}"))
            });
        }
        if e.is::<pipeline::MissingSecret>() {
            return Err(Ended {
                halt: Some(needs_user(Cause::Secret, anyhow!("{e:#}"))),
                ..Ended::new(Outcome::Failed, format!("{what}{e:#}"))
            });
        }
        Err(Ended::new(Outcome::Failed, format!("{what}{e:#}")))
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
        let task = match self.pr_job_ready(pr, fix) {
            Ok(task) => task,
            Err(ended) => return Ok(ended),
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

    /// Whether a `review` or (`fix`) `fix` job of PR `pr` may run: only of a PR sbxm opened
    /// (answer 4), skipped when it is merged or closed. `Ok`: the task it works on, when known;
    /// `Err`: how it ends without running.
    fn pr_job_ready(&self, pr: u32, fix: bool) -> std::result::Result<Option<String>, Ended> {
        let command = if fix { "fix" } else { "review" };
        if jobs::opener(&self.base_dir, &self.repo, pr).is_none() {
            return Err(Ended::new(
                Outcome::Refused,
                format!(
                    "no task record of {} names PR #{pr} as its PR, live or archived, so sbxm \
                     didn't open it; run `sbxm task {command} --pr {pr}` by hand instead",
                    self.repo
                ),
            ));
        }
        let info = self.github.pr(&self.repo, pr).map_err(|e| {
            Ended::new(
                Outcome::Failed,
                format!("PR #{pr} could not be read: {e:#}"),
            )
        })?;
        match info.state {
            PrState::Merged => Err(Ended::new(Outcome::Skipped, format!("PR #{pr} is merged"))),
            PrState::Closed => Err(Ended::new(Outcome::Skipped, format!("PR #{pr} is closed"))),
            PrState::Open if fix => Ok(record::fix_id(pr, &info.head_oid)),
            PrState::Open => Ok(Some(format!("pr-{pr}"))),
        }
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
        // Where its labels pin it (issue 600), refused as the loop's own pick refuses it: the
        // reasons above read `cloud_slots` now, the loop when it started.
        let location = match location::for_loop(&found.labels, self.cloud_slots) {
            Ok(location) => location,
            Err(reason) => return Ok(Ended::new(Outcome::Refused, reason)),
        };
        self.locations.insert(issue, location);
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
            Ok(End::Stopped(_)) | Err(Halt::Stopped) => {
                Ended::new(Outcome::Stopped, "stopped on request")
            }
            Err(Halt::Error(e)) => Ended {
                // A missing secret stays a stop that needs the user (decision 310).
                halt: Some(
                    if let Some(NeedsUser(cause, _)) = e.downcast_ref::<NeedsUser>() {
                        needs_user(*cause, anyhow!("{e:#}"))
                    } else {
                        anyhow!("{e:#}")
                    },
                ),
                ..Ended::new(Outcome::Failed, format!("{e:#}"))
            },
            Err(Halt::SignIn(e)) => {
                let exists = self.record(issue).is_some();
                let stop = sign_in_stop(
                    e,
                    exists.then(|| format!("sbxm task resume --issue {issue}")),
                );
                Ended {
                    halt: Some(needs_user(Cause::SignIn, anyhow!("{stop:#}"))),
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
                    halt: Some(needs_user(Cause::UsageLimit, anyhow!("{stop}"))),
                    ..Ended::new(Outcome::Stopped, stop)
                }
            }
        };
        Ok(ended.task(id))
    }
}

/// A queued job running as a child (issue 633), with what its end needs.
pub(super) struct Queued {
    /// Its file in `queue/running/`.
    taken: PathBuf,
    label: String,
    read: AsRead,
    started: u64,
    /// The slot its children hold.
    slot: Location,
    what: QueuedWhat,
}

/// Which queued job a child runs.
enum QueuedWhat {
    /// `task review --pr N`, or (`fix`) `task fix --pr N`; `task`: the task it works on.
    Pr {
        pr: u32,
        fix: bool,
        task: Option<String>,
    },
    /// `template build`.
    Template { profile: String, harness: Harness },
    /// `task run --spec`, then (`should_fix`) its should-fix round; then its finish here.
    Spec {
        spec: PathBuf,
        id: String,
        should_fix: bool,
    },
}

/// Queued job `path` as its file says now, without taking it; `None` for one that can't be read
/// or isn't a job (its run says why).
fn peek(path: &Path) -> Option<Job> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    jobs::parse(&std::fs::read_to_string(path).ok()?).ok()
}

/// A job file's text in the log when it isn't a job: `job <file name>`.
fn file_label(taken: &Path) -> String {
    let name = taken
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("job {name}")
}
