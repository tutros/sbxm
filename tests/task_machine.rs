//! The state machine migration (decisions 173-175, spec section 5, spike
//! `sdlc/spikes/state-table.md`): the transition table in `task::machine` against what the
//! commands that now decide through it (`check_can_gate`, `check_can_review`,
//! `check_can_review_pr`, `check_can_finish`, `plan_removal`, `Record::advance`) say, for every
//! (state, event) pair. Any difference would mean the table and a command have drifted apart; the
//! list below is pinned empty, so a new difference fails the test instead of going unnoticed.

use std::collections::BTreeSet;

use sbxm::task::finish::{check_can_finish, plan_removal};
use sbxm::task::machine::{Event, STAGES, State, dead_ends, matching, states, verdict};
use sbxm::task::pipeline::{check_can_gate, check_can_review, check_can_review_pr};
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Stopped};
use sbxm::task::resume::check_can_resume;

const T0: u64 = 1_790_000_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn record_in(state: &State) -> Record {
    // A spec id isn't `<prefix>-<number>` (record::spec_id's format), so the default naming
    // would build an id `plan_removal` refuses as invalid before it even reads the state.
    let id = (state.kind == Kind::Spec).then_some("spec-t-abc123");
    let mut record = Record::new(
        &NewTask {
            kind: state.kind,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id,
        },
        T0,
        Process::new(1234, T0),
    );
    record.stage = state.stage;
    record.status = state.status;
    record
}

/// What the commands say: `true` when the operation is allowed. Each of these now decides through
/// `machine::verdict` itself, so this doubles as a regression check that they stay in step with
/// the table as both change.
fn guard(state: &State, event: Event) -> bool {
    let record = record_in(state);
    let probe = Probe(if state.interrupted { None } else { Some(T0) });
    match event {
        Event::Gates => check_can_gate(&record, &probe).is_ok(),
        Event::Review => match state.kind {
            Kind::Issue | Kind::Spec => check_can_review(&record, &probe).is_ok(),
            Kind::Pr => check_can_review_pr(&record).is_ok(),
        },
        Event::Finish => check_can_finish(&record).is_ok(),
        Event::Rm => {
            let base = tempfile::tempdir().unwrap();
            record::write(&record::task_dir(base.path(), &record.id), &record).unwrap();
            plan_removal(base.path(), &record.id, &probe).is_ok()
        }
        // The table doesn't model the recorded process of a task that isn't `running`: a state
        // it calls not in flight has no live process. `ready` is resumable only once the task
        // stopped (`stopped`, not in the table either), so it is asked with a stopped task and
        // `--rounds 1`.
        Event::Resume => {
            let alive = state.status == record::Status::Running && !state.interrupted;
            let probe = Probe(alive.then_some(T0));
            let mut record = record;
            let rounds = if state.stage == Stage::Ready {
                record.stopped = Some(Stopped::RoundsExhausted);
                Some(1)
            } else {
                None
            };
            check_can_resume(&record, &probe, rounds).is_ok()
        }
        Event::Advance(to) => {
            let mut record = record;
            record.advance(to, T0 + 1, Process::new(1234, T0)).is_ok()
        }
    }
}

fn events() -> Vec<Event> {
    let mut all = vec![
        Event::Gates,
        Event::Review,
        Event::Finish,
        Event::Rm,
        Event::Resume,
    ];
    all.extend(STAGES.iter().map(|&stage| Event::Advance(stage)));
    all
}

fn key(state: &State, event: Event) -> String {
    format!(
        "{:?} {:?}/{:?}{} {:?}",
        state.kind,
        state.stage,
        state.status,
        if state.interrupted {
            "+interrupted"
        } else {
            ""
        },
        event
    )
}

/// `task gates` takes only `--issue`, so a PR task is never asked.
fn is_asked(state: &State, event: Event) -> bool {
    !(state.kind == Kind::Pr && event == Event::Gates)
}

/// The differences between the table and the guards, as `<key>: table says X, guard says Y`.
fn differences() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for kind in [Kind::Issue, Kind::Pr, Kind::Spec] {
        for state in states(kind) {
            for event in events() {
                if !is_asked(&state, event) {
                    continue;
                }
                let (table, code) = (verdict(&state, event), guard(&state, event));
                if table != code {
                    found.insert(format!(
                        "{}: table {}, guard {}",
                        key(&state, event),
                        if table { "allows" } else { "refuses" },
                        if code { "allows" } else { "refuses" },
                    ));
                }
            }
        }
    }
    found
}

#[test]
fn the_table_and_the_guards_agree_except_for_the_listed_differences() {
    let found = differences();
    let expected: BTreeSet<String> = EXPECTED.iter().map(|s| (*s).to_owned()).collect();
    let new: Vec<_> = found.difference(&expected).collect();
    let gone: Vec<_> = expected.difference(&found).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "new differences:\n{}\n\nlisted but no longer different:\n{}",
        new.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        gone.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// The table and `Record::advance` (and, below, every other guard) now agree everywhere: `advance`
/// respects the task kind, so findings F1 and F2 of `sdlc/spikes/state-table.md` are closed. An
/// empty list here means exactly that; a future difference fails the build instead of silently
/// reappearing.
const EXPECTED: &[&str] = &[];

/// The rows are checked top to bottom and the first match wins (spec section 5.2); that is only
/// meaningful if at most one row ever matches a given (state, event) pair to begin with, so there
/// is no hidden ambiguity for a row's destination and action to resolve.
#[test]
fn every_state_event_pair_matches_at_most_one_row() {
    for kind in [Kind::Issue, Kind::Pr, Kind::Spec] {
        for state in states(kind) {
            for event in events() {
                let rows = matching(&state, event);
                assert!(
                    rows.len() <= 1,
                    "{} matches {} rows",
                    key(&state, event),
                    rows.len()
                );
            }
        }
    }
}

fn names(kind: Kind) -> Vec<String> {
    dead_ends(kind)
        .iter()
        .map(|s| {
            format!(
                "{:?}/{:?}{}",
                s.stage,
                s.status,
                if s.interrupted { "+interrupted" } else { "" }
            )
        })
        .collect()
}

/// The gaps T3-T6 and T9 of the spec fall out of the table: these are the states a task can be
/// left in (not finished, not in flight) from which no command moves it on, only `rm`.
/// `Prepared/Failed` is one the spec does not list (finding F4). `task resume` (issue 118) closes
/// them one by one; each row goes from this list as its `resume` row is built.
#[test]
fn the_dead_ends_are_the_gaps_the_spec_lists() {
    assert_eq!(
        names(Kind::Issue),
        [
            "Reviewing/Running+interrupted",
            "Reviewing/Completed",
            "Fixing/Running+interrupted",
            "Fixing/Failed",
        ]
    );
    assert_eq!(
        names(Kind::Pr),
        [
            "Prepared/Failed",
            "Gating/Running+interrupted",
            "Reviewing/Running+interrupted",
            "Reviewing/Completed",
        ]
    );
    // A spec task runs the same stages as an issue task (issue 142), and since issue 143 its
    // `ready` moves on through `finish` and the `[finish] sink`, so it has the same dead ends.
    assert_eq!(
        names(Kind::Spec),
        [
            "Reviewing/Running+interrupted",
            "Reviewing/Completed",
            "Fixing/Running+interrupted",
            "Fixing/Failed",
        ]
    );
}
