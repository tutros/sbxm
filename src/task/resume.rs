//! `sbxm task resume` (issue 118; decisions 173(c), 175; spec `sdlc/specs/task-state-machine.md`
//! section 5.2): continues an issue or spec task from its recorded stage, keeping its clone and
//! commits, and carries it on to `ready` the way `task review` would. Refused while the recorded
//! `sbxm` process is alive, and for a PR task (no worker; `ready` ends it). Decided through
//! `machine::TABLE`'s `Resume` rows; the refusal text for each state lives here.

use anyhow::{Result, bail};

use super::machine::{self, Event};
use super::pipeline::{self, Prepared, ReviewReport, TaskEnv, Worked};
use super::record::{self, Kind, Process, ProcessProbe, Record, Stage, Status};
use crate::headless::RunStatus;
use crate::run::results::now;

/// What `resume` did.
#[derive(Debug)]
pub struct Resumed {
    /// The stage and status the task was resumed from.
    pub from: (Stage, Status),
    /// Whether the worker's sandbox was gone and had to be made again.
    pub sandbox_remade: bool,
    /// The worker's run, when the worker ran again.
    pub worked: Option<Worked>,
    /// The review that carried the task on to `ready` (or stopped it on failed gates); `None`
    /// when an earlier step failed.
    pub review: Option<ReviewReport>,
}

/// The status command to point at: a spec task has no number to filter by.
fn status_hint(task: &Record) -> String {
    match task.kind {
        Kind::Spec => "sbxm task status".to_owned(),
        _ => format!("sbxm task status --issue {}", task.number),
    }
}

/// Whether `task resume` (with `--rounds`, when given) may continue this task now. Every refusal
/// says why and what to do instead.
pub fn check_can_resume(
    task: &Record,
    probe: &dyn ProcessProbe,
    rounds: Option<u32>,
) -> Result<()> {
    let id = &task.id;
    if task.kind == Kind::Pr {
        bail!(
            "task {id} is a PR task, which has no worker or fix rounds to resume; to review the PR \
             again, run `sbxm task rm --pr {n}`, then `sbxm task review --pr {n}`",
            n = task.number
        );
    }
    if task.process_alive(probe)
        && let Some(process) = &task.process
    {
        bail!(
            "task {id} is still running in sbxm process pid {} (started {}); wait for it to \
             finish, or stop that process first",
            process.pid,
            process.started_at
        );
    }
    if rounds == Some(0) {
        bail!("--rounds adds fix rounds to the task's budget; give 1 or more");
    }
    if let Some(n) = rounds
        && task.stage != Stage::Ready
    {
        bail!(
            "--rounds {n} is for a task that stopped ready with its rounds used; task {id} is at \
             {} ({}), so resume it without --rounds",
            task.stage.name(),
            task.status.name()
        );
    }
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        interrupted: task.is_interrupted(probe),
        kind: task.kind,
    };
    if machine::verdict(&state, Event::Resume) {
        return Ok(());
    }
    match task.stage {
        Stage::Finished => bail!(
            "task {id} is already finished; to work on what its review found, file the findings \
             (`sbxm task file-findings`) and start tasks for them"
        ),
        stage => bail!(
            "task {id} is {} ({}), which `task resume` cannot continue; see `{}`",
            stage.name(),
            task.status.name(),
            status_hint(task)
        ),
    }
}

/// Continues the task from its recorded stage (issue 118). Checks first: nothing runs or is
/// written when [`check_can_resume`] or the reviewer's checks refuse.
pub fn resume(env: &TaskEnv, prepared: &mut Prepared, rounds: Option<u32>) -> Result<Resumed> {
    check_can_resume(&prepared.record, env.probe, rounds)?;
    pipeline::check_reviewer(env, prepared)?;
    let mut resumed = Resumed {
        from: (prepared.record.stage, prepared.record.status),
        sandbox_remade: false,
        worked: None,
        review: None,
    };
    let probe = env.probe;
    // Whether the worker runs again (its stage is set up below, running).
    let mut run_worker = false;
    match (prepared.record.stage, prepared.record.status) {
        // F4, decision 175: a cut-off `sbx create` may have left a half-made sandbox, so it is
        // made afresh. A failure leaves the task `prepared/failed` with its folders, for another
        // resume.
        (Stage::Prepared, _) => {
            if let Err(e) = pipeline::ensure_worker_sandbox(env, prepared, true) {
                prepared.record.status = Status::Failed;
                prepared
                    .record
                    .notes
                    .push(format!("resume: the preparation failed again: {e:#}"));
                record::write(&prepared.meta, &prepared.record)?;
                return Err(e);
            }
            resumed.sandbox_remade = true;
            prepared.record.rerun(now(), Process::current(probe))?;
            prepared
                .record
                .advance(Stage::Working, now(), Process::current(probe))?;
            run_worker = true;
        }
        // T6, T9: the worker runs again in its own clone, which keeps what the last run did.
        (Stage::Working, Status::Running | Status::Failed) => {
            resumed.sandbox_remade = pipeline::ensure_worker_sandbox(env, prepared, false)?;
            prepared.record.rerun(now(), Process::current(probe))?;
            run_worker = true;
        }
        // A worker or fix round that ended, but whose process was gone before the review: it may
        // have been cut off while its commits were collected, so they are collected again (a
        // fetch of what `repo.git` already has changes nothing). A fix round is counted once its
        // run is over (`review_issue` adds it then), so this one was never counted.
        (Stage::Working | Stage::Fixing, Status::Completed | Status::TimedOut) => {
            resumed.sandbox_remade = pipeline::ensure_worker_sandbox(env, prepared, false)?;
            pipeline::recollect_commits(env, prepared)?;
            if prepared.record.stage == Stage::Fixing {
                prepared.record.round += 1;
            }
        }
        // T3: its process is gone, so the review it was running never finished: the same as one
        // that failed, which `review_issue` runs again (its gates passed for these commits).
        (Stage::Reviewing, Status::Running) => {
            prepared.record.status = Status::Failed;
            prepared
                .record
                .notes
                .push("resume: the review was interrupted; it runs again".to_owned());
        }
        // Gates (ended or cut off) and a failed review: what `review_issue` already continues.
        (Stage::Gating, _) | (Stage::Reviewing, Status::Failed) => {}
        (stage, status) => bail!(
            "task {} is {} ({}), which `task resume` cannot continue yet",
            prepared.record.id,
            stage.name(),
            status.name()
        ),
    }
    if resumed.sandbox_remade && resumed.from.0 != Stage::Prepared {
        prepared
            .record
            .notes
            .push("resume: the worker's sandbox was gone and was made again".to_owned());
    }
    if run_worker {
        record::write(&prepared.meta, &prepared.record)?;
        let worked = pipeline::work(env, prepared)?;
        let failed = matches!(worked.status, RunStatus::Failed(_));
        resumed.worked = Some(worked);
        if failed {
            return Ok(resumed);
        }
    }
    resumed.review = Some(pipeline::review_issue(env, prepared)?);
    Ok(resumed)
}
