//! Jobs as children (issue 633, milestone 5 slice 5; decisions 317(7), (8), 321): with
//! `[resources] cloud_slots` set, the loop's own resolves (decision 298) and log-fixes (decisions
//! 299, 300) and its queued jobs (decision 309) run as child `sbxm` processes, supervised next to
//! its task children, instead of in the loop's process one after another. `rm-merged` stays in
//! the loop's process (it takes seconds). With `cloud_slots` unset nothing here runs: the loop
//! runs them in its process, one at a time, as before (decision 321(5)).
//!
//! - **Slots:** a job child holds its slot, local or cloud, by [`Loop::job_slot`]'s rules (a
//!   template build is local), as a task child does.
//! - **Jobs first** (317(7)): the loop's own steps start their children before any new issue in
//!   each pass, so a job that waits for its slot takes it as soon as it is free; issues for the
//!   other place may start meanwhile.
//! - **One child per PR** (317(7)): a job on a PR that already has a job child running waits,
//!   keeping its place; the jobs behind it that fit may start. Once the first ends, the waiting
//!   one runs against the PR's new head; a resolve no longer needed is dropped as before.
//! - **Ends:** what each job's end does (re-arming, its log row, its result file) runs in the
//!   loop's process as the child ends, one at a time.

use std::io::Write;

use anyhow::Result;

use super::{JobStep, Loop, PhaseEnd, Started};
use crate::backend::Location;
use crate::github::WorkflowRun;

/// A job child the loop started and supervises, until it ends.
pub(super) struct JobChild<'a> {
    /// What it is, and what its end does.
    work: Work,
    /// The PR it works on: no other job child starts on it meanwhile.
    pub pr: Option<u32>,
    /// The slot it holds.
    pub slot: Location,
    /// Its running command.
    phase: Box<dyn Started + 'a>,
}

/// What a job child is, with what its end needs.
enum Work {
    /// A resolve of PR `number` (decision 298), its head `head` when it started.
    Resolve {
        number: u32,
        task: String,
        armed: bool,
        branch: String,
        head: String,
        started: u64,
    },
    /// The one log-fix of PR `number` (decision 300), for its `run` of `head`.
    LogFix {
        number: u32,
        task: String,
        armed: bool,
        head: String,
        run: WorkflowRun,
        started: u64,
    },
}

impl<'a> Loop<'a> {
    /// Whether a job child runs on PR `pr` (317(7)).
    pub(super) fn pr_busy(&self, pr: u32) -> bool {
        self.jobs.iter().any(|j| j.pr == Some(pr))
    }

    /// How many job children hold a slot in `at`.
    pub(super) fn jobs_in(&self, at: Location) -> usize {
        self.jobs.iter().filter(|j| j.slot == at).count()
    }

    /// Starts PR `number`'s resolve as a child (issue 633): its steps before `task fix` here, then
    /// `task fix --pr N` as a child; its end is [`Self::job_ended`]'s.
    pub(super) fn start_resolve(
        &mut self,
        number: u32,
        task: &str,
        armed: bool,
        branch: &str,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let started = self.now();
        let Some(head) = self.resolve_begin(number, armed, branch, out, warn)? else {
            return Ok(());
        };
        let slot = self.fix_slot(number).unwrap_or(self.default_location);
        let job = JobStep::Fix {
            pr: number,
            check_run: None,
        };
        let phase = self.steps.start_job(&job, slot, out, warn);
        self.jobs.push(JobChild {
            work: Work::Resolve {
                number,
                task: task.to_owned(),
                armed,
                branch: branch.to_owned(),
                head,
                started,
            },
            pr: Some(number),
            slot,
            phase,
        });
        Ok(())
    }

    /// Starts PR `number`'s log-fix as a child (issue 633): its steps before `task fix` here,
    /// then `task fix --pr N --check-run <id>` as a child; its end is [`Self::job_ended`]'s.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_log_fix(
        &mut self,
        number: u32,
        task: &str,
        armed: bool,
        head: &str,
        run: &WorkflowRun,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<()> {
        let started = self.now();
        self.log_fix_begin(number, armed, run, out, warn)?;
        let slot = self.fix_slot(number).unwrap_or(self.default_location);
        let job = JobStep::Fix {
            pr: number,
            check_run: Some(run.id),
        };
        let phase = self.steps.start_job(&job, slot, out, warn);
        self.jobs.push(JobChild {
            work: Work::LogFix {
                number,
                task: task.to_owned(),
                armed,
                head: head.to_owned(),
                run: run.clone(),
                started,
            },
            pr: Some(number),
            slot,
            phase,
        });
        Ok(())
    }

    /// Looks at each job child once, in the order they started, and acts on each end at once.
    /// Returns whether one ended, and the first stop that needs the user one of them asked for.
    pub(super) fn supervise_jobs(
        &mut self,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<(bool, Option<anyhow::Error>)> {
        let mut ended_any = false;
        let mut halt = None;
        let mut i = 0;
        while i < self.jobs.len() {
            let Some(end) = self.jobs[i].phase.poll(out) else {
                i += 1;
                continue;
            };
            let job = self.jobs.remove(i);
            ended_any = true;
            if let Some(stop) = self.job_ended(job.work, end, out, warn)? {
                halt.get_or_insert(stop);
            }
        }
        Ok((ended_any, halt))
    }

    /// What a job child's end does; `Some` is a stop that needs the user.
    fn job_ended(
        &mut self,
        work: Work,
        end: PhaseEnd,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<anyhow::Error>> {
        let end = end.masked();
        match work {
            Work::Resolve {
                number,
                task,
                armed,
                branch,
                head,
                started,
            } => {
                let result = end.result();
                self.resolve_end(
                    number, &task, armed, &branch, &head, started, result, out, warn,
                )?;
                Ok(None)
            }
            Work::LogFix {
                number,
                task,
                armed,
                head,
                run,
                started,
            } => {
                let result = end.result();
                self.log_fix_end(
                    number, &task, armed, &head, &run, started, result, out, warn,
                )?;
                Ok(None)
            }
        }
    }
}
