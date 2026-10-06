//! State-machine spike (decisions 173 and 174, spec section 5): the transition table in
//! `task::machine` against what the existing guards say, for every (state, event) pair.
//! Every difference is listed below with its reason, so a new one (or a fixed one) fails the test.

use std::collections::BTreeSet;

use sbxm::task::finish::{check_can_finish, plan_removal};
use sbxm::task::machine::{Event, STAGES, State, dead_ends, matching, states, verdict};
use sbxm::task::pipeline::{check_can_gate, check_can_review, check_can_review_pr};
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record};

const T0: u64 = 1_790_000_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn record_in(state: &State) -> Record {
    let mut record = Record::new(
        &NewTask {
            kind: state.kind,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
        },
        T0,
        Process::new(1234, T0),
    );
    record.stage = state.stage;
    record.status = state.status;
    record
}

/// What the existing code says: `true` when the operation is allowed.
fn guard(state: &State, event: Event) -> bool {
    let record = record_in(state);
    let probe = Probe(if state.interrupted { None } else { Some(T0) });
    match event {
        Event::Gates => check_can_gate(&record, &probe).is_ok(),
        Event::Review => match state.kind {
            Kind::Issue => check_can_review(&record, &probe).is_ok(),
            Kind::Pr => check_can_review_pr(&record).is_ok(),
        },
        Event::Finish => check_can_finish(&record).is_ok(),
        Event::Rm => {
            let base = tempfile::tempdir().unwrap();
            record::write(&record::task_dir(base.path(), &record.id), &record).unwrap();
            plan_removal(base.path(), &record.id, &probe).is_ok()
        }
        Event::Advance(to) => {
            let mut record = record;
            record.advance(to, T0 + 1, Process::new(1234, T0)).is_ok()
        }
    }
}

fn events() -> Vec<Event> {
    let mut all = vec![Event::Gates, Event::Review, Event::Finish, Event::Rm];
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
    for kind in [Kind::Issue, Kind::Pr] {
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

/// Where the table (spec sections 2 and 5.2) and `Record::advance` differ; every one is about
/// `advance` not knowing the kind of task (see `sdlc/spikes/state-table.md`, findings F1-F2).
const EXPECTED: &[&str] = &[
    // F1: `advance` lets an issue task skip its worker.
    "Issue Prepared/Running Advance(Gating): table refuses, guard allows",
    "Issue Prepared/Running Advance(Reviewing): table refuses, guard allows",
    "Issue Prepared/Running+interrupted Advance(Gating): table refuses, guard allows",
    "Issue Prepared/Running+interrupted Advance(Reviewing): table refuses, guard allows",
    // F2: `advance` lets a PR task have a worker, a fix round or a finish.
    "Pr Prepared/Running Advance(Working): table refuses, guard allows",
    "Pr Prepared/Running+interrupted Advance(Working): table refuses, guard allows",
    "Pr Reviewing/Completed Advance(Fixing): table refuses, guard allows",
    "Pr Ready/Ok Advance(Finished): table refuses, guard allows",
];

/// The rows are checked top to bottom and the first match wins (spec section 5.2); that is only
/// meaningful if at most one row ever matches a given (state, event) pair to begin with, so there
/// is no hidden ambiguity for a row's destination and action to resolve.
#[test]
fn every_state_event_pair_matches_at_most_one_row() {
    for kind in [Kind::Issue, Kind::Pr] {
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
/// `Prepared/Failed` is one the spec does not list (finding F4).
#[test]
fn the_dead_ends_are_the_gaps_the_spec_lists() {
    assert_eq!(
        names(Kind::Issue),
        [
            "Prepared/Running+interrupted",
            "Prepared/Failed",
            "Working/Running+interrupted",
            "Working/Failed",
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
}
