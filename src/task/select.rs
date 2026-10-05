//! Issue selection (spec §8, decision 154): which open issues get a task, and why the
//! others don't. Pure; the rules are those of `Select-Issues` in the PowerShell script removed in decision 165,
//! plus decision 169's: a `must-fix` issue of an open PR comes first.

use std::collections::HashMap;
use std::fmt;

use crate::github::{Issue, PrState};
use crate::task::findings::pr_of;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    HasTask,
    Question,
    /// The open issues it depends on, ascending.
    Blocked(Vec<u32>),
    /// The issues with a task (or picked in this run) it is related to.
    RelatedToTask(Vec<u32>),
    /// A `must-fix` issue whose PR (`PR: #n`) is closed or merged (decision 169).
    PrNotOpen(u32, PrState),
    /// A `must-fix` issue whose PR's state isn't known (the lookup failed), so it can't be shown
    /// to belong to an open PR; only an explicit list starts it.
    PrUnreadable(u32),
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
            Self::PrNotOpen(pr, state) => {
                let state = match state {
                    PrState::Open => "open",
                    PrState::Closed => "closed",
                    PrState::Merged => "merged",
                };
                write!(f, "its PR #{pr} is {state}")
            }
            Self::PrUnreadable(pr) => write!(f, "its PR #{pr} couldn't be read"),
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
    /// Warnings from before selection (the caller prints them), e.g. a PR that couldn't be read.
    pub warnings: Vec<String>,
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

/// The PR a `must-fix` issue names on its first line (`PR: #n`), with that PR's state when known.
fn must_fix_pr(issue: &Issue, prs: &HashMap<u32, PrState>) -> Option<(u32, Option<PrState>)> {
    if !issue.labels.iter().any(|l| l == "must-fix") {
        return None;
    }
    let pr = pr_of(&issue.body)?;
    Some((pr, prs.get(&pr).copied()))
}

/// The rules that come before any PR is looked at, in the order the spec gives them: already has
/// a task, a question, blocked by an open issue. `select` applies them first, and
/// `needs_pr_state` uses the same function to leave every issue they skip out of the PR lookups,
/// so the order lives in one place.
fn early_skip(issue: &Issue, taken: &[u32], open_numbers: &[u32]) -> Option<Reason> {
    if taken.contains(&issue.number) {
        return Some(Reason::HasTask);
    }
    if issue.labels.iter().any(|l| l == "question") {
        return Some(Reason::Question);
    }
    let mut blockers = field_numbers(&issue.body, "Depends on");
    blockers.sort_unstable();
    blockers.dedup();
    blockers.retain(|b| open_numbers.contains(b));
    (!blockers.is_empty()).then_some(Reason::Blocked(blockers))
}

/// The `(pr, issue)` pairs whose PR state an automatic selection needs: `must-fix` issues naming
/// a PR that no earlier rule skips. Nothing else is worth a lookup (or a warning about one).
pub fn needs_pr_state(open: &[Issue], in_progress: &[u32]) -> Vec<(u32, u32)> {
    let open_numbers: Vec<u32> = open.iter().map(|i| i.number).collect();
    open.iter()
        .filter(|i| early_skip(i, in_progress, &open_numbers).is_none())
        .filter_map(|i| must_fix_pr(i, &HashMap::new()).map(|(pr, _)| (pr, i.number)))
        .collect()
}

/// Picks up to `workers` issues (all of `explicit` when given): `must-fix` issues of an open PR
/// first, then by label rank, then by number. `prs` holds the known state of the PRs that
/// `must-fix` issues name; without an explicit list, one whose PR is closed or merged is skipped,
/// and so is one whose PR is missing from `prs` (it couldn't be read, so it isn't known to be open).
/// An explicit list names any issue, whatever its PR.
pub fn select(
    open: &[Issue],
    in_progress: &[u32],
    explicit: Option<&[u32]>,
    workers: usize,
    prs: &HashMap<u32, PrState>,
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
    let of_open_pr = |i: &Issue| matches!(must_fix_pr(i, prs), Some((_, Some(PrState::Open))));
    candidates.sort_by_key(|i| (!of_open_pr(i), rank(&i.labels), i.number));

    let mut taken: Vec<u32> = in_progress.to_vec();
    for candidate in candidates {
        if explicit.is_none() && selection.picks.len() >= workers {
            break;
        }
        let number = candidate.number;
        let own = related_of.get(&number).map_or(&[][..], Vec::as_slice);
        let clashes: Vec<u32> = taken
            .iter()
            .copied()
            .filter(|t| own.contains(t) || related_of.get(t).is_some_and(|r| r.contains(&number)))
            .collect();

        if let Some(reason) = early_skip(candidate, &taken, &open_numbers) {
            selection.skips.push((number, reason));
        } else if let Some((pr, Some(state @ (PrState::Closed | PrState::Merged)))) =
            must_fix_pr(candidate, prs).filter(|_| explicit.is_none())
        {
            selection.skips.push((number, Reason::PrNotOpen(pr, state)));
        } else if let Some((pr, None)) = must_fix_pr(candidate, prs).filter(|_| explicit.is_none())
        {
            selection.skips.push((number, Reason::PrUnreadable(pr)));
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
