//! The loop's queued jobs (issue 525, decision 309): before each new issue, after its conflict
//! resolves and its checks of the runs, the loop runs every job file in `<base>/.sbxm/queue/`,
//! oldest first ([`jobs::pending`]), each ending with one result file. At its start it reports
//! the jobs a stopped loop left in `queue/running/` as `interrupted`, without running them again.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, anyhow};

use super::{
    Capture, Cause, Claimed, End, Halt, Loop, NeedsUser, Steps, check, first_line, needs_user,
    says_sign_in, sign_in_stop,
};
use crate::commands::task_start::spec_flag;
use crate::config::GlobalConfig;
use crate::github::PrState;
use crate::notify::Notification;
use crate::run::results::format_timestamp;
use crate::task::jobs::{self, AsRead, Job, JobHarness, JobResult, Outcome, Running};
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
        let config = match GlobalConfig::load(self.config_dir) {
            Ok(config) => config,
            Err(e) => return Ok(Ended::new(Outcome::Failed, format!("{e:#}"))),
        };
        let (spec, id) = match jobs::spec_file(self.config_dir, &config, file) {
            Ok(found) => found,
            Err(why) => return Ok(Ended::new(Outcome::Refused, why)),
        };
        self.mark(taken, read, started, Some(id.clone()))?;
        let ended = self.spec_task(&spec, &id, out, warn)?;
        Ok(Ended {
            flag: Some(spec_flag(&spec)),
            ..ended.task(id)
        })
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
        let config = match GlobalConfig::load(self.config_dir) {
            Ok(config) => config,
            Err(e) => return Ok(Ended::new(Outcome::Failed, format!("{e:#}"))),
        };
        if let Err(why) = jobs::template_profile(&config, profile) {
            return Ok(Ended::new(Outcome::Refused, why));
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

/// A job file's text in the log when it isn't a job: `job <file name>`.
fn file_label(taken: &Path) -> String {
    let name = taken
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("job {name}")
}
