//! Issue 715: the improvement review's spec, the embedded `prompts/improvement-review.md` filled in
//! with an area, a date and the run data ([`improvement::spec`]), and the run data's 30 KB cap
//! ([`improvement::run_data`]): the newest rows are kept and the spec says how many were left out.

use sbxm::run::results::parse_timestamp;
use sbxm::task::improvement::{self, Anchor, Area, CAP, LoopLog, RunData};

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

/// The run data of `facts` and `logs`, with nothing skipped and no anchors.
fn data(facts: &str, logs: &[LoopLog], since: Option<&str>) -> RunData {
    improvement::run_data(facts, &[], logs, &[], since)
}

fn spec(data: &RunData) -> String {
    improvement::spec(&area(), "2026-11-18", Some("2026-11-16"), data).unwrap()
}

// --- The prompt ---

#[test]
fn task_improvement_spec_says_no_must_fix_and_at_most_10_findings() {
    let text = spec(&data(FACTS_HEADER, &[], Some("2026-11-16")));
    assert!(text.contains("no must-fix"), "{text}");
    assert!(text.contains("at most 10 findings"), "{text}");
}

#[test]
fn task_improvement_spec_names_kind_7_and_asks_for_run_data_evidence() {
    let text = spec(&data(FACTS_HEADER, &[], Some("2026-11-16")));
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
    let text = spec(&data(FACTS_HEADER, &[], Some("2026-11-16")));
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
    let data = data(FACTS_HEADER, &[], None);
    let text = improvement::spec(&area(), "2026-11-18", None, &data).unwrap();
    assert!(text.contains("no improvement review yet"), "{text}");
}

// --- The 30 KB cap ---

#[test]
fn task_improvement_spec_run_data_under_the_cap_is_whole() {
    let data = data(&facts(10, 100), &[], Some("2026-11-16"));
    assert_eq!(data.left_out, 0);
    assert!(data.text.contains("issue-0000"));
    assert!(data.text.contains("issue-0009"));
    assert!(spec(&data).contains("All the run data since 2026-11-16 is below."));
}

/// The run data's size with `n` rows of 100 bytes and no loop log.
fn size_with(n: usize) -> usize {
    data(&facts(n, 100), &[], Some("2026-11-16")).text.len()
}

#[test]
fn task_improvement_spec_run_data_at_the_cap_keeps_every_row_and_one_over_leaves_one_out() {
    // The most rows that fit: each is 100 bytes.
    let fits = (CAP - size_with(0)) / 100;
    let at_cap = data(&facts(fits, 100), &[], Some("2026-11-16"));
    assert_eq!(at_cap.left_out, 0);
    assert!(at_cap.text.len() <= CAP);

    let over = data(&facts(fits + 1, 100), &[], Some("2026-11-16"));
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
        end: start + 13 * 3600,
        text: log,
    }];
    let data = data(&facts(600, 100), &logs, Some("2026-11-16"));

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

#[test]
fn task_improvement_spec_a_loop_started_before_the_window_gives_only_its_rows_in_it() {
    let log = "# Loop log\n\nRepository: o/r\n\n| # | Issue | Size | Slot | Started | Ready | W/R | \
               Rounds | Outcome | Stops | Idle |\n|---|---|---|---|---|---|---|---|---|---|---|\n\
               | 1 | #801 before | S | - | 23:30:00 | - | - | - | merged | none | - |\n\
               | 2 | #802 after midnight | S | - | 00:30:00 | - | - | - | merged | none | - |\n";
    let logs = [LoopLog {
        start: parse_timestamp("2026-11-15T23:00:00Z").unwrap(),
        end: parse_timestamp("2026-11-16T01:00:00Z").unwrap(),
        text: log.to_owned(),
    }];

    let data = data(FACTS_HEADER, &logs, Some("2026-11-16"));

    assert!(!data.text.contains("#801"), "{}", data.text);
    assert!(
        data.text
            .contains("| 2026-11-15T23:00:00Z | 2026-11-16 | 2 | #802 after midnight |"),
        "{}",
        data.text
    );
    assert_eq!(data.left_out, 0);
}

// --- Review M-1 of issue 715: a loop-log row's day ---

const LOG_HEAD: &str = "# Loop log\n\nRepository: o/r\n\n| # | Issue | Size | Slot | Started | Ready | \
                        W/R | Rounds | Outcome | Stops | Idle |\n\
                        |---|---|---|---|---|---|---|---|---|---|---|\n";

fn ts(stamp: &str) -> u64 {
    parse_timestamp(stamp).unwrap()
}

/// A log row `n` for issue `issue` started at `clock`.
fn log_row(n: usize, issue: u32, clock: &str) -> String {
    format!("| {n} | #{issue} t{issue} | S | - | {clock} | - | - | - | merged | none | - |\n")
}

fn anchor(issue: u32, stamp: &str) -> Anchor {
    Anchor {
        key: format!("#{issue}"),
        at: ts(stamp),
    }
}

/// A loop that ran from 2026-11-15T10:00 to 2026-11-17T13:00 with a row on each day.
fn three_day_log() -> LoopLog {
    let mut text = LOG_HEAD.to_owned();
    text.push_str(&log_row(1, 801, "11:00:00"));
    text.push_str(&log_row(2, 802, "09:00:00"));
    text.push_str(&log_row(3, 803, "12:00:00"));
    LoopLog {
        start: ts("2026-11-15T10:00:00Z"),
        end: ts("2026-11-17T13:00:00Z"),
        text,
    }
}

fn three_day_anchors() -> Vec<Anchor> {
    vec![
        anchor(801, "2026-11-15T11:00:04Z"),
        anchor(802, "2026-11-16T09:00:02Z"),
        anchor(803, "2026-11-17T12:00:07Z"),
    ]
}

#[test]
fn task_improvement_spec_a_loop_running_for_days_keeps_its_rows_of_the_window() {
    let data = improvement::run_data(
        FACTS_HEADER,
        &[],
        &[three_day_log()],
        &three_day_anchors(),
        Some("2026-11-17"),
    );

    assert!(
        data.text
            .contains("| 2026-11-15T10:00:00Z | 2026-11-17 | 3 | #803 t803 |"),
        "{}",
        data.text
    );
    assert!(!data.text.contains("#801"), "{}", data.text);
    assert!(!data.text.contains("#802"), "{}", data.text);
}

#[test]
fn task_improvement_spec_a_next_day_row_later_than_the_loops_start_clock_keeps_its_day() {
    let data = improvement::run_data(
        FACTS_HEADER,
        &[],
        &[three_day_log()],
        &three_day_anchors(),
        Some("2026-11-15"),
    );

    assert!(
        data.text
            .contains("| 2026-11-15T10:00:00Z | 2026-11-15 | 1 | #801 t801 |"),
        "{}",
        data.text
    );
    assert!(
        data.text
            .contains("| 2026-11-15T10:00:00Z | 2026-11-16 | 2 | #802 t802 |"),
        "{}",
        data.text
    );
    let mut text = LOG_HEAD.to_owned();
    text.push_str(&log_row(1, 804, "12:00:00"));
    let log = LoopLog {
        start: ts("2026-11-15T10:00:00Z"),
        end: ts("2026-11-16T13:00:00Z"),
        text,
    };
    let data = improvement::run_data(
        FACTS_HEADER,
        &[],
        &[log],
        &[anchor(804, "2026-11-16T12:00:01Z")],
        Some("2026-11-16"),
    );
    assert!(
        data.text.contains("| 2026-11-16 | 1 | #804 t804 |"),
        "{}",
        data.text
    );
}

#[test]
fn task_improvement_spec_a_loop_of_less_than_a_day_tells_the_day_without_a_record() {
    let mut text = LOG_HEAD.to_owned();
    text.push_str(&log_row(1, 805, "02:00:00"));
    let log = LoopLog {
        start: ts("2026-11-15T22:00:00Z"),
        end: ts("2026-11-16T03:00:00Z"),
        text,
    };

    let data = improvement::run_data(FACTS_HEADER, &[], &[log], &[], Some("2026-11-16"));

    assert!(
        data.text.contains("| 2026-11-16 | 1 | #805 t805 |"),
        "{}",
        data.text
    );
}

#[test]
fn task_improvement_spec_a_row_no_record_dates_is_kept_with_its_day_unknown() {
    let data = improvement::run_data(
        FACTS_HEADER,
        &[],
        &[three_day_log()],
        &[],
        Some("2026-11-17"),
    );

    // Each clock fits several days up to the 17th: none is guessed.
    assert!(
        data.text
            .contains("| unknown (2026-11-15 to 2026-11-17) | 3 | #803 t803 |"),
        "{}",
        data.text
    );
    // 09:00 is before the loop's start clock, so its first day is the 16th.
    assert!(
        data.text
            .contains("| unknown (2026-11-16 to 2026-11-17) | 2 | #802 t802 |"),
        "{}",
        data.text
    );
    // The window goes by the last day a row can be: a clock that fits no day in it is left out.
    let mut text = LOG_HEAD.to_owned();
    text.push_str(&log_row(1, 806, "14:00:00"));
    let log = LoopLog {
        start: ts("2026-11-15T10:00:00Z"),
        end: ts("2026-11-17T13:00:00Z"),
        text,
    };
    let data = improvement::run_data(FACTS_HEADER, &[], &[log], &[], Some("2026-11-17"));
    assert!(!data.text.contains("#806"), "{}", data.text);
}

#[test]
fn task_improvement_spec_over_the_cap_rows_of_later_days_go_first_out_of_log_order() {
    // Parallel tasks: #902 started first but ended (and was logged) after #901.
    let mut text = LOG_HEAD.to_owned();
    text.push_str(&log_row(1, 901, "11:00:00"));
    text.push_str(&log_row(2, 902, "05:30:00"));
    let log = LoopLog {
        start: ts("2026-11-15T05:00:00Z"),
        end: ts("2026-11-17T12:00:00Z"),
        text,
    };
    let anchors = [
        anchor(901, "2026-11-17T11:00:00Z"),
        anchor(902, "2026-11-15T05:30:00Z"),
    ];
    // Tasks from 2026-11-17T00:00, one a minute, more than fit.
    let data = improvement::run_data(&facts(600, 100), &[], &[log], &anchors, None);

    assert!(data.text.len() <= CAP);
    assert!(data.left_out > 0);
    assert!(
        data.text.contains("| 2026-11-17 | 1 | #901 t901 |"),
        "the newest row is kept: {}",
        &data.text[data.text.len() - 400..]
    );
    assert!(!data.text.contains("#902"), "the oldest row is left out");
}

// --- Review M-3 of issue 715: the cap counts every byte it renders ---

/// The facts table with one row `len` bytes long, line end included, padded with `pad`.
fn one_row(len: usize, pad: char) -> String {
    let row = "| issue-0001 | 1 | 2026-11-17T00:00:00Z | S | 1 | 0 | - | - | - | - | - | ";
    let end = "finished | - |\n";
    let room = len - row.len() - end.len();
    let padding: String = std::iter::repeat_n(pad, room / pad.len_utf8()).collect();
    let fill = " ".repeat(room - padding.len());
    format!("{FACTS_HEADER}{row}{padding}{fill}{end}")
}

#[test]
fn task_improvement_spec_without_a_loop_log_a_row_up_to_the_cap_is_kept_and_one_byte_over_isnt() {
    for pad in [' ', 'é', '…'] {
        let fixed = data(FACTS_HEADER, &[], Some("2026-11-16")).text.len();
        let at_cap = data(&one_row(CAP - fixed, pad), &[], Some("2026-11-16"));
        assert_eq!(at_cap.left_out, 0, "pad {pad:?}");
        assert_eq!(at_cap.text.len(), CAP, "pad {pad:?}");

        let over = data(&one_row(CAP - fixed + 1, pad), &[], Some("2026-11-16"));
        assert_eq!(over.left_out, 1, "pad {pad:?}");
        assert!(over.text.len() <= CAP, "pad {pad:?}");
    }
}

// --- Review M-2 of issue 715: what the run data couldn't read ---

#[test]
fn task_improvement_spec_records_task_facts_skipped_make_the_spec_say_incomplete() {
    let skipped = vec!["/b/.sbxm/tasks/issue-9/task.json skipped: not a task record".to_owned()];
    let data = improvement::run_data(FACTS_HEADER, &skipped, &[], &[], Some("2026-11-16"));

    let text = spec(&data);

    assert!(!text.contains("All the run data"), "{text}");
    assert!(text.contains("the run data is incomplete"), "{text}");
    assert!(text.contains("issue-9/task.json skipped"), "{text}");
}
