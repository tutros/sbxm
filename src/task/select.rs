//! Issue selection (spec §8, decision 154): which open issues get a task, and why the
//! others don't. Pure; the rules are those of `Select-Issues` in `scripts/issue-workers.psm1`.

use std::collections::HashMap;
use std::fmt;

use crate::github::Issue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    HasTask,
    Question,
    /// The open issues it depends on, ascending.
    Blocked(Vec<u32>),
    /// The issues with a task (or picked in this run) it is related to.
    RelatedToTask(Vec<u32>),
}

fn numbers(list: &[u32]) -> String {
    list.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HasTask => write!(f, "already has a task"),
            Self::Question => write!(f, "a question, needs your answer first"),
            Self::Blocked(open) => write!(f, "blocked by open {}", numbers(open)),
            Self::RelatedToTask(tasks) => {
                write!(f, "related to {}, which has a task", numbers(tasks))
            }
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub picks: Vec<u32>,
    /// Every candidate that was passed over, in the order it was considered.
    pub skips: Vec<(u32, Reason)>,
    /// Explicitly named numbers that are not open issues (the caller warns).
    pub not_open: Vec<u32>,
}

/// The `#<digits>` numbers on the first line of `body` that starts with `**<field>:**`.
fn field_numbers(body: &str, field: &str) -> Vec<u32> {
    let prefix = format!("**{field}:**");
    let Some(line) = body.lines().find_map(|line| line.strip_prefix(&prefix)) else {
        return Vec::new();
    };
    line.split('#')
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect()
}

fn rank(labels: &[String]) -> u8 {
    labels
        .iter()
        .filter_map(|label| match label.as_str() {
            "must-fix" => Some(0),
            "should-fix" => Some(1),
            _ => None,
        })
        .min()
        .unwrap_or(9)
}

/// Picks up to `workers` issues (all of `explicit` when given), by label rank then number.
pub fn select(
    open: &[Issue],
    in_progress: &[u32],
    explicit: Option<&[u32]>,
    workers: usize,
) -> Selection {
    let open_numbers: Vec<u32> = open.iter().map(|i| i.number).collect();
    // "Related" is often written on one of the two issues only, so look both ways.
    let related_of: HashMap<u32, Vec<u32>> = open
        .iter()
        .map(|i| (i.number, field_numbers(&i.body, "Related")))
        .collect();

    let mut selection = Selection::default();
    if let Some(wanted) = explicit {
        selection.not_open = wanted
            .iter()
            .copied()
            .filter(|n| !open_numbers.contains(n))
            .collect();
    }

    let mut candidates: Vec<&Issue> = open
        .iter()
        .filter(|i| explicit.is_none_or(|wanted| wanted.contains(&i.number)))
        .collect();
    candidates.sort_by_key(|i| (rank(&i.labels), i.number));

    let mut taken: Vec<u32> = in_progress.to_vec();
    for candidate in candidates {
        if explicit.is_none() && selection.picks.len() >= workers {
            break;
        }
        let number = candidate.number;
        let mut blockers = field_numbers(&candidate.body, "Depends on");
        blockers.sort_unstable();
        blockers.dedup();
        blockers.retain(|b| open_numbers.contains(b));
        let own = related_of.get(&number).map_or(&[][..], Vec::as_slice);
        let clashes: Vec<u32> = taken
            .iter()
            .copied()
            .filter(|t| own.contains(t) || related_of.get(t).is_some_and(|r| r.contains(&number)))
            .collect();

        if taken.contains(&number) {
            selection.skips.push((number, Reason::HasTask));
        } else if candidate.labels.iter().any(|l| l == "question") {
            selection.skips.push((number, Reason::Question));
        } else if !blockers.is_empty() {
            selection.skips.push((number, Reason::Blocked(blockers)));
        } else if !clashes.is_empty() {
            selection
                .skips
                .push((number, Reason::RelatedToTask(clashes)));
        } else {
            selection.picks.push(number);
            taken.push(number);
        }
    }
    selection
}
