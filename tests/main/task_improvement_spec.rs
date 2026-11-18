//! Issue 715: the improvement review's spec, the embedded `prompts/improvement-review.md` filled in
//! with an area, a date and the run data ([`improvement::spec`]), and the run data's 30 KB cap
//! ([`improvement::run_data`]): the newest rows are kept and the spec says how many were left out.

use sbxm::run::results::parse_timestamp;
use sbxm::task::improvement::{self, Area, CAP, LoopLog, RunData};

const FACTS_HEADER: &str = "| task | issue | started | size | review rounds | must-fix per round | \
                            wall to ready (s) | worker (s) | worker cost | reviewer (s) | \
                            reviewer cost | outcome | PR |\n\
                            |---|---|---|---|---|---|---|---|---|---|---|---|---|\n";

fn area() -> Area {
    Area {
        slug: "task-loop".into(),
        paths: "`src/task/`, `src/commands/task_loop.rs`".into(),
    }
}

/// The facts table with `n` tasks, one a minute from 2026-11-17T00:00:00Z, each row padded to
/// `width` bytes with its line end.
fn facts(n: usize, width: usize) -> String {
    let start = parse_timestamp("2026-11-17T00:00:00Z").unwrap();
    let mut text = FACTS_HEADER.to_owned();
    for i in 0..n {
        let stamp = sbxm::run::results::format_timestamp(start + 60 * i as u64);
        let row = format!("| issue-{i:04} | {i} | {stamp} | S | 1 | 0 | - | - | - | - | - | ");
        let pad = width - row.len() - "finished | - |\n".len();
        text.push_str(&format!("{row}{}finished | - |\n", " ".repeat(pad)));
    }
    text
}

fn spec(data: &RunData) -> String {
    improvement::spec(&area(), "2026-11-18", Some("2026-11-16"), data).unwrap()
}

// --- The prompt ---

#[test]
fn task_improvement_spec_says_no_must_fix_and_at_most_10_findings() {
    let text = spec(&improvement::run_data(
        FACTS_HEADER,
        &[],
        Some("2026-11-16"),
    ));
    assert!(text.contains("no must-fix"), "{text}");
    assert!(text.contains("at most 10 findings"), "{text}");
}

#[test]
fn task_improvement_spec_names_kind_7_and_asks_for_run_data_evidence() {
    let text = spec(&improvement::run_data(
        FACTS_HEADER,
        &[],
        Some("2026-11-16"),
    ));
    assert!(
        text.contains("7. Workflow friction the code could remove"),
        "{text}"
    );
    assert!(text.contains("evidence from the run"), "{text}");
    assert!(text.contains("data below"), "{text}");
    assert!(
        text.contains("each finding names the code it would change"),
        "{text}"
    );
}

#[test]
fn task_improvement_spec_fills_in_the_area_the_date_and_the_output() {
    let text = spec(&improvement::run_data(
        FACTS_HEADER,
        &[],
        Some("2026-11-16"),
    ));
    assert!(!text.contains("{{"), "every placeholder filled: {text}");
    assert!(
        text.contains("sdlc/reviews/2026-11-18-improvement-task-loop.md"),
        "{text}"
    );
    assert!(
        text.contains("`src/task/`, `src/commands/task_loop.rs`"),
        "{text}"
    );
    assert!(text.contains("### IMP-"), "{text}");
}

#[test]
fn task_improvement_spec_without_a_review_says_so() {
    let data = improvement::run_data(FACTS_HEADER, &[], None);
    let text = improvement::spec(&area(), "2026-11-18", None, &data).unwrap();
    assert!(text.contains("no improvement review yet"), "{text}");
}

// --- The 30 KB cap ---

#[test]
fn task_improvement_spec_run_data_under_the_cap_is_whole() {
    let data = improvement::run_data(&facts(10, 100), &[], Some("2026-11-16"));
    assert_eq!(data.left_out, 0);
    assert!(data.text.contains("issue-0000"));
    assert!(data.text.contains("issue-0009"));
    assert!(spec(&data).contains("All the run data since 2026-11-16 is below."));
}

/// The run data's size with `n` rows of 100 bytes and no loop log.
fn size_with(n: usize) -> usize {
    improvement::run_data(&facts(n, 100), &[], Some("2026-11-16"))
        .text
        .len()
}

#[test]
fn task_improvement_spec_run_data_at_the_cap_keeps_every_row_and_one_over_leaves_one_out() {
    // The most rows that fit: each is 100 bytes.
    let fits = (CAP - size_with(0)) / 100;
    let at_cap = improvement::run_data(&facts(fits, 100), &[], Some("2026-11-16"));
    assert_eq!(at_cap.left_out, 0);
    assert!(at_cap.text.len() <= CAP);

    let over = improvement::run_data(&facts(fits + 1, 100), &[], Some("2026-11-16"));
    assert_eq!(over.left_out, 1);
    assert!(over.text.len() <= CAP);
    // The oldest is left out, the newest kept.
    assert!(!over.text.contains("| issue-0000 |"), "{}", over.text);
    assert!(over.text.contains(&format!("| issue-{fits:04} |")));
}

#[test]
fn task_improvement_spec_run_data_over_the_cap_keeps_the_newest_of_both_parts_and_names_the_left_out()
 {
    let start = parse_timestamp("2026-11-17T00:00:00Z").unwrap();
    // One loop's log, after every task: its rows are the newest.
    let mut log = "# Loop log\n\nRepository: o/r\n\n| # | Issue | Size | Slot | Started | Ready | \
                   W/R | Rounds | Outcome | Stops | Idle |\n|---|---|---|---|---|---|---|---|---|---|---|\n"
        .to_owned();
    for i in 0..50 {
        log.push_str(&format!(
            "| {} | #9{i:02} t | S | - | 12:{i:02}:00 | - | - | - | merged | none | - |\n",
            i + 1
        ));
    }
    let logs = [LoopLog {
        start: start + 12 * 3600,
        text: log,
    }];
    let data = improvement::run_data(&facts(600, 100), &logs, Some("2026-11-16"));

    assert!(data.text.len() <= CAP, "{}", data.text.len());
    assert!(data.left_out > 0);
    // Every loop-log row is newer than every task, so all are kept.
    assert!(data.text.contains("#900 t"), "{}", data.text);
    assert!(data.text.contains("#949 t"));
    assert!(data.text.contains("| issue-0599 |"));
    assert!(!data.text.contains("| issue-0000 |"));
    let kept = (0..600)
        .filter(|i| data.text.contains(&format!("| issue-{i:04} |")))
        .count();
    assert_eq!(kept + data.left_out, 600);
    let text = spec(&data);
    assert!(
        text.contains(&format!(
            "the {} oldest rows were left out, the newest kept",
            data.left_out
        )),
        "{text}"
    );
}
