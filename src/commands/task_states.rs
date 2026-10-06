//! `sbxm task states` (issue 116, spec `sdlc/specs/task-state-machine.md` section 5.2, migration
//! step 3 of 5.5): prints `task::machine::TABLE` as the same markdown table the spec's "Today
//! (generated)" block carries verbatim, so the two can be compared by a test instead of drifting.
//! Reads nothing and calls no backend.

use std::fmt::Write as _;

use crate::task::machine::{self, Event, Row};
use crate::task::record::Kind;

fn stage_cell(stages: &[crate::task::record::Stage]) -> String {
    if stages.len() == machine::STAGES.len() {
        "any".to_owned()
    } else {
        stages
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn status_cell(statuses: &[crate::task::record::Status]) -> String {
    statuses
        .iter()
        .map(|s| s.name())
        .collect::<Vec<_>>()
        .join("/")
}

fn kind_suffix(kinds: &[Kind]) -> &'static str {
    match kinds {
        [Kind::Issue] => " (issue)",
        [Kind::Pr] => " (pr)",
        _ => "",
    }
}

fn from_cell(row: &Row) -> String {
    format!(
        "{} {}{}{}",
        stage_cell(row.stages),
        status_cell(row.statuses),
        kind_suffix(row.kinds),
        if row.live == machine::Live::Interrupted {
            ", interrupted"
        } else {
            ""
        },
    )
}

fn event_cell(event: Event) -> &'static str {
    match event {
        Event::Gates => "task gates",
        Event::Review => "task review",
        Event::Finish => "task finish",
        Event::Rm => "task rm",
        Event::Advance(_) => "advance",
    }
}

fn to_cell(row: &Row) -> &'static str {
    row.to.map_or("removed", |stage| stage.name())
}

/// The transition table (`task::machine::TABLE`) as markdown: one row per table entry, in table
/// order. Pure and total: every row in `TABLE` renders, so this never fails.
pub fn render() -> String {
    let mut out = String::from("| From | Event | To | Action |\n|---|---|---|---|\n");
    for row in machine::TABLE {
        writeln!(
            out,
            "| {} | {} | {} | {} |",
            from_cell(row),
            event_cell(row.event),
            to_cell(row),
            row.action,
        )
        .expect("writing to a String never fails");
    }
    out
}
