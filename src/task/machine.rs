//! The task state machine as data (decisions 173, 174, 175; spec `sdlc/specs/task-state-machine.md`
//! sections 2 and 5.2). `check_can_gate`, `check_can_review`, `check_can_review_pr`,
//! `check_can_finish` and `finish::plan_removal` all decide through `verdict`/`row_for`, and
//! `Record::advance` enforces the same moves directly (spike `sdlc/spikes/state-table.md`,
//! migration step 2). `tests/task_machine.rs` walks every (state, event) pair and checks that the
//! table and those commands still agree.

use super::record::{Kind, Stage, Status};

/// Something that can be asked of a task in a given state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `task gates`.
    Gates,
    /// `task review` (issue tasks and PR tasks differ in what they allow).
    Review,
    /// `task finish`.
    Finish,
    /// `task rm` and `task start --restart`.
    Rm,
    /// The pipeline moving the task to a stage (`Record::advance`).
    Advance(Stage),
}

/// Everything the guards look at: the stage, the status, whether a `running` task's process is gone
/// and the kind of task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub stage: Stage,
    pub status: Status,
    pub interrupted: bool,
    pub kind: Kind,
}

/// Which `running` tasks a row covers; irrelevant for any other status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Any,
    /// The recorded process is gone.
    Interrupted,
}

/// One allowed move: in any of `stages` with any of `statuses` (and `live`), a task of one of the
/// `kinds` may take `event`. Whatever no row allows is refused.
pub struct Row {
    pub stages: &'static [Stage],
    pub statuses: &'static [Status],
    pub live: Live,
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
            && (self.live == Live::Any || state.interrupted)
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
/// Issue and spec tasks both have a worker and go through gates, review and fix rounds the same
/// way (issue 142, decision 174(d)): a spec source adds this one answer set, no new rows.
const HAS_WORKER: &[Kind] = &[Kind::Issue, Kind::Spec];
/// Every kind: the rows that don't care which source a task has.
const ANY: &[Kind] = &[Kind::Issue, Kind::Pr, Kind::Spec];

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
/// two rows match the same (state, event) pair to begin with (`tests/task_machine.rs` checks it).
pub const TABLE: &[Row] = &[
    // The pipeline's moves (`Record::advance`).
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Working),
        to: Some(Stage::Working),
        action: "run the worker",
    },
    Row {
        stages: &[Stage::Prepared],
        statuses: &[Status::Running],
        live: Live::Any,
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
        kinds: PR,
        event: Event::Advance(Stage::Reviewing),
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Working],
        statuses: WORKER_DONE,
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Passed],
        live: Live::Any,
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
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Fixing),
        to: Some(Stage::Fixing),
        action: "run the fix round for the gate's own output",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Fixing),
        to: Some(Stage::Fixing),
        action: "run the fix round",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Completed],
        live: Live::Any,
        kinds: ANY,
        event: Event::Advance(Stage::Ready),
        to: Some(Stage::Ready),
        action: "record the task as ready",
    },
    Row {
        stages: &[Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Advance(Stage::Gating),
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        kinds: ISSUE,
        event: Event::Advance(Stage::Finished),
        to: Some(Stage::Finished),
        action: "push the branch and open the PR",
    },
    // `task gates` (a task with a worker: issue and spec).
    Row {
        stages: &[Stage::Working, Stage::Fixing],
        statuses: WORKER_DONE,
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Gates,
        to: Some(Stage::Gating),
        action: "run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Gates,
        to: Some(Stage::Gating),
        action: "re-run the gates",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Running],
        live: Live::Interrupted,
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
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: &[Status::Running],
        live: Live::Interrupted,
        kinds: HAS_WORKER,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "re-run the abandoned gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Failed],
        live: Live::Any,
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
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Gating),
        action: "run the gates, then the reviewer",
    },
    Row {
        stages: &[Stage::Gating],
        statuses: GATES_ENDED,
        live: Live::Any,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "run the reviewer",
    },
    Row {
        stages: &[Stage::Reviewing],
        statuses: &[Status::Failed],
        live: Live::Any,
        kinds: PR,
        event: Event::Review,
        to: Some(Stage::Reviewing),
        action: "retry the reviewer",
    },
    // `task finish`.
    Row {
        stages: &[Stage::Ready],
        statuses: &[Status::Ok],
        live: Live::Any,
        kinds: ISSUE,
        event: Event::Finish,
        to: Some(Stage::Finished),
        action: "push the branch and open the PR",
    },
    // `task rm`: anything that is not running, or running with its process gone.
    Row {
        stages: &STAGES,
        statuses: NOT_RUNNING,
        live: Live::Any,
        kinds: ANY,
        event: Event::Rm,
        to: None,
        action: "remove the task's folders and sandboxes",
    },
    Row {
        stages: &STAGES,
        statuses: &[Status::Running],
        live: Live::Interrupted,
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

/// Every state a task of `kind` can be recorded in: each stage with each of its statuses, and a
/// `running` one both alive and interrupted. A PR task has no worker, so it is never `working` or
/// `fixing`.
pub fn states(kind: Kind) -> Vec<State> {
    let mut all = Vec::new();
    for stage in STAGES {
        if kind == Kind::Pr && matches!(stage, Stage::Working | Stage::Fixing) {
            continue;
        }
        for &status in stage.statuses() {
            for interrupted in [false, true] {
                if interrupted && status != Status::Running {
                    continue;
                }
                all.push(State {
                    stage,
                    status,
                    interrupted,
                    kind,
                });
            }
        }
    }
    all
}

/// The states of a `kind` task that are not the end of its life (`finished`; a PR task's `ready`),
/// not in flight (a `running` task whose process is alive) and from which no command moves it on:
/// only `rm` is allowed. Each is a gap in the operations. A spec task's `ready` is one of these
/// too (not excluded like a PR task's): `finish` is refused for it until a sink lands (decision
/// 174(e), issue 142), so it is a dead end by design, not yet a missing operation.
pub fn dead_ends(kind: Kind) -> Vec<State> {
    states(kind)
        .into_iter()
        .filter(|s| s.stage != Stage::Finished && !(kind == Kind::Pr && s.stage == Stage::Ready))
        .filter(|s| s.status != Status::Running || s.interrupted)
        .filter(|s| {
            [Event::Gates, Event::Review, Event::Finish]
                .iter()
                .all(|&e| !verdict(s, e))
        })
        .collect()
}
