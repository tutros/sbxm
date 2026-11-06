//! The comment `task finish` posts on the issue a task's PR closes (issue 430): the in-task
//! review and fix rounds, which otherwise live only in the task's folder (`review-<n>.md`), told
//! from `task.json` and those files. No review text is copied: only counts, sbxm's own links,
//! and each must-fix finding's id and title made inert ([`inert`]).

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::record::{Record, RunInfo};
use super::review::{self, COMMENT_CAP};

/// The longest finding id or title the comment shows in full, in characters.
pub const TITLE_CAP: usize = 100;

/// `text` as an inline code span GitHub renders as it is: backticks removed (so the span can't
/// be closed early), each line break made a space, and cut to [`TITLE_CAP`] characters with `…`.
/// An `@name`, a `Fixes #12`, a link or markup inside it does nothing.
pub fn inert(text: &str) -> String {
    let flat: String = text
        .chars()
        .filter(|&c| c != '`')
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let shown = match flat.char_indices().nth(TITLE_CAP) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    };
    format!("`{shown}`")
}

/// One saved review round, as the comment tells it.
struct Round {
    number: u32,
    /// `None` for a review that wasn't used (no readable must-fix count).
    must_fix: Option<u32>,
    repeats: usize,
    /// Each must-fix finding's id and title.
    findings: Vec<(String, String)>,
}

/// The review rounds saved in the task's folder `meta`, in order.
fn rounds(meta: &Path) -> Vec<Round> {
    let saved: Vec<(u32, String)> = review::saved_rounds(meta)
        .into_iter()
        .filter_map(|r| {
            fs::read_to_string(meta.join(format!("review-{r}.md")))
                .ok()
                .map(|text| (r, text))
        })
        .collect();
    saved
        .iter()
        .enumerate()
        .map(|(i, (number, text))| {
            let earlier: Vec<&str> = saved[..i].iter().map(|(_, t)| t.as_str()).collect();
            Round {
                number: *number,
                must_fix: review::saved_must_fix_count(text),
                repeats: review::repeat_count(text, &earlier),
                findings: review::must_fix_findings(text, *number)
                    .into_iter()
                    .map(|f| (f.id, f.title))
                    .collect(),
            }
        })
        .collect()
}

/// A run's time and cost, as `task.json` recorded them.
fn run_line(run: Option<&RunInfo>) -> String {
    let Some(run) = run else {
        return "not recorded".to_owned();
    };
    let cost = match run.usage.get("cost_usd").and_then(Value::as_f64) {
        Some(usd) => format!("${usd:.2}"),
        None => "cost not reported".to_owned(),
    };
    format!("{} s, {cost}", run.duration_s)
}

/// The comment on an issue the PR at `pr_url` closes, from `task` and the review files in its
/// folder `meta`. It stays below what GitHub accepts: findings that don't fit are counted.
pub fn issue_comment(task: &Record, meta: &Path, pr_url: &str) -> String {
    let mut head = format!(
        "**sbxm task finish**: the in-task review and fix rounds of task `{}`.\n\n",
        task.id
    );
    let rounds = rounds(meta);
    if rounds.is_empty() {
        head.push_str("- No review round is saved in the task's folder.\n");
    }
    let mut tail = String::new();
    tail.push_str(&format!(
        "- Fix rounds run: {} (budget {})\n",
        task.round, task.fix_rounds
    ));
    for n in 1..=task.round {
        let how = match task.fix_runs.iter().find(|r| r.round == n) {
            Some(r) if r.resumed => "resumed the worker's session",
            Some(_) => "ran fresh (the worker's session wasn't resumed)",
            None => "not recorded",
        };
        tail.push_str(&format!("  - Fix round {n}: {how}\n"));
    }
    if let Some(stopped) = task.stopped {
        tail.push_str(&format!("- Stopped: {}\n", stopped.name()));
    }
    tail.push_str(&format!(
        "- Worker: {}\n",
        run_line(task.worker.as_ref().and_then(|w| w.run.as_ref()))
    ));
    tail.push_str(&format!(
        "- Latest review: {}\n",
        run_line(task.reviewer.as_ref().and_then(|r| r.run.as_ref()))
    ));
    tail.push_str(&format!("- PR: {}\n", pr_url.trim()));

    let mut body = head;
    let mut left = 0;
    for round in &rounds {
        let line = match round.must_fix {
            Some(count) => format!(
                "- Review round {}: {count} must-fix, {} `Repeat of:`\n",
                round.number, round.repeats
            ),
            None => format!(
                "- Review round {}: not used (no must-fix count)\n",
                round.number
            ),
        };
        body.push_str(&line);
        for (id, title) in &round.findings {
            let item = format!("  - {} {}\n", inert(id), inert(title));
            if left > 0 || body.len() + item.len() + tail.len() + 200 > COMMENT_CAP {
                left += 1;
                continue;
            }
            body.push_str(&item);
        }
    }
    if left > 0 {
        body.push_str(&format!(
            "- … and {left} more must-fix finding(s), not listed here\n"
        ));
    }
    body.push_str(&tail);
    body
}
