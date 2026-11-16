//! The task state machine as data (decisions 173, 174, 175; spec `sdlc/specs/task-state-machine.md`
//! sections 2 and 5.2). `check_can_gate`, `check_can_review`, `check_can_review_pr`,
//! `check_can_finish`, `check_can_retry_summaries`, `check_can_resume` and
//! `finish::plan_removal` all decide through `verdict`/`row_for` on a [`State`] built by
//! [`State::of`], the only way to build one (issue 513), and `Record::advance` enforces the same
//! moves directly (spike `sdlc/spikes/state-table.md`, migration step 2). The recorded-PR rule of
//! `task finish`, the process check of `task resume`, a `ready` task's stop and budget, `task
//! resume --rounds` and `--should-fix` are rows here too (issue 513), so `task states` lists them. `tests/main/task_machine.rs` walks every (state,
//! event) pair and checks that the table and those commands still agree.

use super::record::{Kind, ProcessProbe, Record, Stage, Status, Stopped};

/// Something that can be asked of a task in a given state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `task gates`.
    Gates,
    /// `task review` (issue tasks and PR tasks differ in what they allow).
    Review,
    /// `task finish`.
    Finish,
    /// `task finish` again on a finished task that has a PR (issue 467): only the issue
    /// summaries are retried or edited (`finish::check_can_retry_summaries`).
    FinishSummaries,
    /// `task rm` and `task start --restart`.
    Rm,
    /// `task resume` (decisions 173(c), 175; issue 118): continue a task from its recorded stage,
    /// keeping its clone and commits.
    Resume,
    /// `task resume --rounds N` (decisions 173(c), 177(b)(q)): add N fix rounds to the budget of a
    /// task that stopped `ready`, then resume it (issue 513 moved the rule into the table).
    ResumeRounds,
    /// `task resume --rounds 1 --should-fix` (issue 510, decision 306): one fix round for a clean
    /// `ready` task's should-fix findings sized S. What the review holds is `check_should_fix`'s.
    ResumeShouldFix,
    /// The pipeline moving the task to a stage (`Record::advance`).
    Advance(Stage),
}

/// Everything the guards look at: the stage, the status, whether a `running` task's process is
/// gone, whether the recorded process is alive at all, the kind of task, whether it has a
/// recorded PR and whether it stopped with fix rounds left. Built only by [`State::of`] (and [`states`], which lists them all), so no check can
/// leave a field out:
///
/// ```compile_fail,E0063
/// use sbxm::task::machine::{State, Stop};
/// use sbxm::task::record::{Kind, Stage, Status};
/// let _ = State {
///     stage: Stage::Ready,
///     status: Status::Ok,
///     interrupted: false,
///     alive: false,
///     kind: Kind::Issue,
///     has_pr: false,
///     stop: Stop::Clean,
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::manual_non_exhaustive,
    reason = "`#[non_exhaustive]` would still let the crate's own modules build a State by hand"
)]
pub struct State {
    pub stage: Stage,
    pub status: Status,
    /// A `running` task whose recorded process is gone.
    pub interrupted: bool,
    /// The recorded process is alive, whatever the status.
    pub alive: bool,
    pub kind: Kind,
    /// `task.json` records a PR (`pr`).
    pub has_pr: bool,
    /// Whether the task stopped, and with fix rounds left (`stopped`, `round`, `fix_rounds`).
    pub stop: Stop,
    /// Private, so a `State` can't be built outside this module (issue 513).
    sealed: (),
}

impl State {
    /// The state of `task` now, with `probe` telling whether its recorded process is alive.
    pub fn of(task: &Record, probe: &dyn ProcessProbe) -> Self {
        Self {
            stage: task.stage,
            status: task.status,
            interrupted: task.is_interrupted(probe),
            alive: task.process_alive(probe),
            kind: task.kind,
            has_pr: task.pr.is_some(),
            stop: match task.stopped {
                None => Stop::Clean,
                Some(Stopped::RoundsExhausted) => Stop::RoundsUsed,
                Some(Stopped::RepeatFinding) if task.round >= task.fix_rounds => Stop::RoundsUsed,
                Some(Stopped::RepeatFinding) => Stop::RoundsLeft,
            },
            sealed: (),
        }
    }
}

/// Whether a task stopped on something unresolved (`stopped`, set when it stops `ready`), and
/// whether its budget has a fix round left for a plain `task resume` (review M-1 of issue 513).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Not stopped.
    Clean,
    /// Stopped (`repeat-finding`) with rounds left in the budget.
    RoundsLeft,
    /// Stopped with its rounds used: `rounds-exhausted`, or the budget is spent.
    RoundsUsed,
}

pub const STOPS: [Stop; 3] = [Stop::Clean, Stop::RoundsLeft, Stop::RoundsUsed];
const STOPPED: &[Stop] = &[Stop::RoundsLeft, Stop::RoundsUsed];

/// Which tasks a row covers by their recorded process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Any,
    /// A `running` task whose recorded process is gone; irrelevant for any other status.
    Interrupted,
    /// No recorded process is alive, whatever the status (`task resume`'s process check).
    Gone,
}

/// Which tasks a row covers by their recorded PR (`task finish`'s rule, issues 430, 467, 480).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedPr {
    Any,
    Yes,
    No,
}

/// One allowed move: in any of `stages` with any of `statuses` (and `live`), a task of one of the
/// `kinds` may take `event`. Whatever no row allows is refused.
pub struct Row {
    pub stages: &'static [Stage],
    pub statuses: &'static [Status],
    pub live: Live,
    pub pr: RecordedPr,
    /// The stops a row covers (`STOPS` for a row that doesn't look at them).
    pub stops: &'static [Stop],
    pub kinds: &'static [Kind],
    pub event: Event,
    /// Where the move leaves the task; `None` for a row that does not change the stage by itself
    /// (`rm`, which removes the task instead).
    pub to: Option<Stage>,
    /// What taking this row does, in words (descriptive only; no driver reads this yet).
    pub action: &'static str,
}

impl Row {
    fn allows(&self, state: &State, event: Event) -> bool {
        self.event == event
            && self.stages.contains(&state.stage)
            && self.statuses.contains(&state.status)
            && self.kinds.contains(&state.kind)
            && match self.live {
                Live::Any => true,
                Live::Interrupted => state.interrupted,
                Live::Gone => !state.alive,
            }
            && match self.pr {
                RecordedPr::Any => true,
                RecordedPr::Yes => state.has_pr,
                RecordedPr::No => !state.has_pr,
            }
            && self.stops.contains(&state.stop)
    }
}

pub const STAGES: [Stage; 7] = [
    Stage::Prepared,
    Stage::Working,
    Stage::Gating,
    Stage::Reviewing,
    Stage::Fixing,
    Stage::Ready,
    Stage::Finished,
];

const ISSUE: &[Kind] = &[Kind::Issue];
const PR: &[Kind] = &[Kind::Pr];
const SPEC: &[Kind] = &[Kind::Spec];
const FIX: &[Kind] = &[Kind::Fix];
/// Issue and spec tasks both have a worker and go through gates, review and fix rounds the same
/// way (issue 142, decision 174(d)): a spec source adds this one answer set, no new rows.
/// A fix task of a PR (issue 313) runs them the same way too.
const HAS_WORKER: &[Kind] = &[Kind::Issue, Kind::Spec, Kind::Fix];
/// Every kind: the rows that don't care which source a task has.
const ANY: &[Kind] = &[Kind::Issue, Kind::Pr, Kind::Spec, Kind::Fix];

const WORKER_DONE: &[Status] = &[Status::Completed, Status::TimedOut];
const GATES_ENDED: &[Status] = &[Status::Passed, Status::GatesFailed];
const NOT_RUNNING: &[Status] = &[
    Status::Completed,
    Status::TimedOut,
    Status::Failed,
    Status::Passed,
    Status::GatesFailed,
    Status::Ok,
];

/// Spec section 5.2 (today's rows) and section 2 (the operations per state). Checked top to
/// bottom; the first row that matches wins ([`row_for`]), though a well-formed table never has
/// two rows match the same (state, event) pair to begin with (`tests/main/task_machine.rs` checks
/// it).
pub const TABLE: &[Row] = &[
    // The pipeline's moves (`Record::advance`).
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Working),
        to: Some(Stage::Working),
        action: "run the worker",
    },
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    // F3 (decision 175): a PR task with no gates configured goes straight to review.
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Advance(Stage::Reviewing),
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Working],
        statuses: WORKER_DONE,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Passed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: ANY,
        event: Event::Advance(Stage::Reviewing),
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    // A gate failure feeds a fix round when the round budget allows it (issue 117, decision
    // 173(d)); `Record::advance` enforces the budget itself through `done_for`, so this row only
    // says the move exists for an issue task.
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::GatesFailed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Fixing),
        to: Some(Stage::Fixing),
        action: "run the fix round for the gate's own output",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Fixing),
        to: Some(Stage::Fixing),
        action: "run the fix round",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: ANY,
        event: Event::Advance(Stage::Ready),
        to: Some(Stage::Ready),
        action: "record the task as ready",
    },
    Row {
        stages: &[Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: ISSUE,
        event: Event::Advance(Stage::Finished),
        to: Some(Stage::Finished),
        action: "push the branch and open the PR",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: SPEC,
        event: Event::Advance(Stage::Finished),
        to: Some(Stage::Finished),
        action: "deliver the branch through the [finish] sink",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: FIX,
        event: Event::Advance(Stage::Finished),
        to: Some(Stage::Finished),
        action: "push the branch onto the PR's own branch, a fast-forward or nothing",
    },
    // `task resume` on a task that stopped `ready` (issue 118); `resume` checks `stopped`
    // and the budget, which this table does not model.
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Fixing),
        to: Some(Stage::Fixing),
        action: "run a fix round from the recorded review",
    },
    // `task review --pr` on a `ready` PR task (issue 195, gap G34): the refusal for an unchanged
    // head lives in `review_pr` itself, which needs a GitHub call this table does not model.
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "fetch the PR's new head, then run the gates",
    },
    // `task gates` (a task with a worker: issue and spec).
    Row {
        stages: &[Stage::Working, Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Gates,
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Gates,
        to: Some(Stage::Gating),
        action: "re-run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Gates,
        to: Some(Stage::Gating),
        action: "re-run the abandoned gates",
    },
    // `task review`, a task with a worker: after the worker or fix round, after gates, or
    // retrying a failed review.
    Row {
        stages: &[Stage::Working, Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "re-run the abandoned gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Failed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "retry the reviewer",
    },
    // `task review --pr`: a PR task has no worker; it starts prepared, or after gates, or retries a failed review.
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Gating),
        action: "run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Failed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "retry the reviewer",
    },
    // A PR task stuck where only `rm` used to move it on (issue 379, decision 282): a preparation
    // that failed, or gates or a review whose process is gone, start over from the PR's head
    // fetched again, the earlier `review-<n>.md` kept and any leftover reviewer removed first.
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Failed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Gating),
        action: "fetch the PR's head again, run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating, Stage::Reviewing],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Gating),
        action: "remove the leftover reviewer, fetch the PR's head again, run the gates, then the reviewer",
    },
    // A completed review never recorded as `ready` (a PR task never saves this state itself:
    // `review_pr` records `ready` in the same write): the reviewer runs again, as after a failure.
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the reviewer again",
    },
    // A `ready` PR task is reviewed again (issue 195, gap G34): refused while the PR's head is
    // still the commit the last review covered, unless `--force` (`task review --pr`'s own
    // check, which this table does not model since it needs a GitHub call).
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Gating),
        action: "fetch the PR's new head, keep the review as review-<n>.md, run the gates, then the reviewer",
    },
    // `task finish`, on a task with no recorded PR yet (a recorded one means it was published).
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::No,
        stops: &STOPS,
        kinds: ISSUE,
        event: Event::Finish,
        to: Some(Stage::Finished),
        action: "push the branch and open the PR",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::No,
        stops: &STOPS,
        kinds: SPEC,
        event: Event::Finish,
        to: Some(Stage::Finished),
        action: "deliver the branch through the [finish] sink",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::No,
        stops: &STOPS,
        kinds: FIX,
        event: Event::Finish,
        to: Some(Stage::Finished),
        action: "push the branch onto the PR's own branch, a fast-forward or nothing",
    },
    // `task finish` on a finished issue or spec task whose PR is recorded (issues 430, 467, 480;
    // a `local` or `push` spec task has none).
    Row {
        stages: &[Stage::Finished],
        statuses: &[Status::Ok],
        live: Live::Any,
        pr: RecordedPr::Yes,
        stops: &STOPS,
        kinds: &[Kind::Issue, Kind::Spec],
        event: Event::FinishSummaries,
        to: Some(Stage::Finished),
        action: "post or edit the issue summaries only (no push, no new PR, no PR review)",
    },
    // `task resume` (decisions 173(c), 175; issue 118), a task with a worker: a failed or
    // interrupted stage runs again in the task's own clone, and the task goes on to `ready`.
    // Refused while the recorded process is alive, whatever the status (`Live::Gone`).
    // F4 (decision 175): a preparation that failed or was cut off keeps its folders and clone.
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Working),
        action: "make the worker's sandbox again, then run the worker and review",
    },
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Failed],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Working),
        action: "make the worker's sandbox again, then run the worker and review",
    },
    Row {
        stages: &[Stage::Working],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Working),
        action: "run the worker again in its clone, then review",
    },
    Row {
        stages: &[Stage::Working],
        statuses: &[Status::Failed],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Working),
        action: "run the worker again in its clone, then review",
    },
    // A worker or fix round that ended, or gates that ended or were cut off: on to the review,
    // as `task review` would.
    Row {
        stages: &[Stage::Working, Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Reviewing),
        action: "run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Reviewing),
        action: "run the reviewer (after the gates again, when they failed)",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Reviewing),
        action: "re-run the abandoned gates, then the reviewer",
    },
    // T3: a review that failed or was cut off runs again; its gates passed for the same commits.
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Reviewing),
        action: "run the reviewer again",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Failed],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Reviewing),
        action: "run the reviewer again",
    },
    // T5: a fix round that failed or was cut off runs again with the same input.
    Row {
        stages: &[Stage::Fixing],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Fixing),
        action: "run the fix round again in its clone, then the gates and the review",
    },
    Row {
        stages: &[Stage::Fixing],
        statuses: &[Status::Failed],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Fixing),
        action: "run the fix round again in its clone, then the gates and the review",
    },
    // T4: a review that completed, but whose process was gone before it acted on it.
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Fixing),
        action: "replay the recorded review: a fix round for its findings, else ready",
    },
    // A task that stopped `ready` with findings left (`stopped`): another fix round, after
    // `--rounds N` added to the budget when it was used (decisions 173(c), 177(b)(q)); `--rounds`
    // is for these states only (issue 513).
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &[Stop::RoundsLeft],
        kinds: HAS_WORKER,
        event: Event::Resume,
        to: Some(Stage::Fixing),
        action: "a fix round from the recorded review",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: STOPPED,
        kinds: HAS_WORKER,
        event: Event::ResumeRounds,
        to: Some(Stage::Fixing),
        action: "add N fix rounds to the budget, then a fix round from the recorded review",
    },
    // A clean `ready` fix task has only its push onto the PR's branch left (issue 485), which
    // `task resume --pr` makes.
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &[Stop::Clean],
        kinds: FIX,
        event: Event::Resume,
        to: Some(Stage::Finished),
        action: "push the branch onto the PR's own branch, a fast-forward or nothing",
    },
    // A clean `ready` task's should-fix round (issue 510, decision 306): at most one per task,
    // for the review's should-fix findings sized S (`check_should_fix` reads the review).
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Gone,
        pr: RecordedPr::Any,
        stops: &[Stop::Clean],
        kinds: HAS_WORKER,
        event: Event::ResumeShouldFix,
        to: Some(Stage::Fixing),
        action: "one fix round for the review's should-fix findings sized S, once per task",
    },
    // `task rm`: anything that is not running, or running with its process gone.
    Row {
        stages: &STAGES,
        statuses: NOT_RUNNING,
        live: Live::Any,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: ANY,
        event: Event::Rm,
        to: None,
        action: "remove the task's folders and sandboxes",
    },
    Row {
        stages: &STAGES,
        statuses: &[Status::Running],
        live: Live::Interrupted,
        pr: RecordedPr::Any,
        stops: &STOPS,
        kinds: ANY,
        event: Event::Rm,
        to: None,
        action: "remove the task's folders and sandboxes",
    },
];

/// The first row that allows `event` in `state` (rows are checked top to bottom).
pub fn row_for(state: &State, event: Event) -> Option<&'static Row> {
    TABLE.iter().find(|row| row.allows(state, event))
}

/// Every row that allows `event` in `state`; at most one in a well-formed table.
pub fn matching(state: &State, event: Event) -> Vec<&'static Row> {
    TABLE
        .iter()
        .filter(|row| row.allows(state, event))
        .collect()
}

/// Whether the table allows `event` in `state`.
pub fn verdict(state: &State, event: Event) -> bool {
    row_for(state, event).is_some()
}

/// Every state a task of `kind` can be recorded in: each stage with each of its statuses, its
/// recorded process alive or gone (a `running` one with its process gone is interrupted), with a
/// recorded PR or without when `finished` (no other stage has one), and with each [`Stop`]. A PR task has no worker, so it is never `working` or `fixing`.
pub fn states(kind: Kind) -> Vec<State> {
    let mut all = Vec::new();
    for stage in STAGES {
        if kind == Kind::Pr && matches!(stage, Stage::Working | Stage::Fixing) {
            continue;
        }
        for &status in stage.statuses() {
            for (interrupted, alive) in [(false, true), (false, false), (true, false)] {
                // A `running` task is interrupted exactly when its process is gone.
                if status == Status::Running && interrupted == alive {
                    continue;
                }
                if status != Status::Running && interrupted {
                    continue;
                }
                for has_pr in [false, true] {
                    // `finish` records the PR in the same write that makes the task `finished`.
                    if has_pr && stage != Stage::Finished {
                        continue;
                    }
                    for stop in STOPS {
                        all.push(State {
                            stage,
                            status,
                            interrupted,
                            alive,
                            kind,
                            has_pr,
                            stop,
                            sealed: (),
                        });
                    }
                }
            }
        }
    }
    all
}

/// The states of a `kind` task that are not where it settles once there is nowhere further to go
/// on its own (`finished`; a PR task's `ready`, which `task review --pr` can still move on from,
/// issue 195, but nothing drives it there by itself), not in flight (a task whose recorded process
/// is alive) and from which no command moves it on: only `rm` is allowed. Each is a gap in
/// the operations.
pub fn dead_ends(kind: Kind) -> Vec<State> {
    states(kind)
        .into_iter()
        .filter(|s| s.stage != Stage::Finished && !(kind == Kind::Pr && s.stage == Stage::Ready))
        .filter(|s| !s.alive)
        .filter(|s| {
            [
                Event::Gates,
                Event::Review,
                Event::Finish,
                Event::Resume,
                Event::ResumeRounds,
                Event::ResumeShouldFix,
            ]
            .iter()
            .all(|&e| !verdict(s, e))
        })
        .collect()
}
