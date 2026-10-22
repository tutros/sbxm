//! Issue 285 (decision 241): Jev's requests (mapping, budget, chunking), its responses and its
//! HTTP statuses, without a sandbox.

use std::path::PathBuf;

use sbxm::backend::SecretSource;
use sbxm::eval::jev::{self, Answered, Outcome, Scored};
use sbxm::run::config::{Criterion, CriterionKind};
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("fixtures/jev/request-two-criteria.json");

fn pass_fail(id: &str, notes: Option<&str>) -> Criterion {
    Criterion {
        id: id.into(),
        kind: CriterionKind::PassFail,
        levels: vec![],
        weight: 1.0,
        notes: notes.map(str::to_owned),
    }
}

fn scale(id: &str, levels: &[&str]) -> Criterion {
    Criterion {
        id: id.into(),
        kind: CriterionKind::Scale,
        levels: levels.iter().map(|l| l.to_string()).collect(),
        weight: 0.5,
        notes: None,
    }
}

fn rubric() -> Vec<Criterion> {
    vec![
        pass_fail("correct", Some("the function returns the right sum")),
        scale("quality", &["poor", "fair", "good"]),
    ]
}

const DIFF: &str = "diff --git a/src/sum.rs b/src/sum.rs\n--- a/src/sum.rs\n+++ b/src/sum.rs\n@@ -1 +1 @@\n-fn sum() {}\n+fn sum(a: i32, b: i32) -> i32 { a + b }\n";

// ---- request ------------------------------------------------------------------------------

#[test]
fn the_request_matches_the_golden_file() {
    let plan = jev::plan("Write sum.", &rubric(), "Added sum.", Some(DIFF)).unwrap();
    assert_eq!(plan.requests.len(), 1);
    assert!(!plan.truncated);
    let body = jev::request_body("jev-latest", &plan.requests[0].state, &plan.questions);
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(
        body,
        golden,
        "{}",
        serde_json::to_string_pretty(&body).unwrap()
    );
    // The shapes the API documents, the model present, the levels in order.
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["questions"]["correct"]["type"], "noul");
    assert!(body["questions"]["correct"]["criteria"]["true"].is_string());
    assert!(body["questions"]["correct"]["criteria"]["false"].is_string());
    assert_eq!(body["questions"]["quality"]["type"], "score");
    assert_eq!(
        body["questions"]["quality"]["criteria"],
        json!(["poor", "fair", "good"])
    );
    let state = body["state"].as_str().unwrap();
    assert!(state.contains("Added sum.") && state.contains("+fn sum(a: i32"));
}

#[test]
fn the_state_never_carries_the_transcript() {
    // The plan takes only the answer and the diff; a transcript sentinel given as neither
    // cannot appear (the runner's tests check it end to end).
    let plan = jev::plan("Task", &rubric(), "the answer", Some(DIFF)).unwrap();
    let body = jev::request_body("m", &plan.requests[0].state, &plan.questions).to_string();
    assert!(!body.contains("TRANSCRIPT-SENTINEL"));
}

#[test]
fn instructions_carry_the_id_the_notes_and_the_task() {
    let q = jev::questions("Write sum.", &rubric());
    let text = q["correct"]["instructions"].as_str().unwrap();
    assert!(text.contains("correct") && text.contains("right sum") && text.contains("Write sum."));
    assert!(
        q["quality"]["instructions"]
            .as_str()
            .unwrap()
            .contains("quality")
    );
}

// ---- budget -------------------------------------------------------------------------------

/// The estimate the plan uses: 1 token per 3 characters, 300 overhead, 70 per extra question.
fn one_question() -> Vec<Criterion> {
    vec![pass_fail("ok", None)]
}

#[test]
fn a_task_over_the_budget_on_its_own_makes_no_call_and_names_the_task() {
    let task = "t".repeat(3 * jev::TOKEN_CAP);
    let err = jev::plan(&task, &one_question(), "short", Some(DIFF)).unwrap_err();
    assert!(err.contains("task") && err.contains("no call"), "{err}");
}

#[test]
fn an_answer_over_the_budget_with_the_task_makes_no_call_and_names_the_answer() {
    let answer = "a".repeat(3 * jev::TOKEN_CAP);
    let err = jev::plan("Task", &one_question(), &answer, Some(DIFF)).unwrap_err();
    assert!(err.contains("answer") && err.contains("no call"), "{err}");
    assert!(!err.contains("aaaa"), "{err}");
}

#[test]
fn an_answer_just_under_the_cap_is_planned() {
    // Find the longest answer that still plans, then show one more character is refused.
    let fits = |n: usize| jev::plan("Task", &one_question(), &"a".repeat(n), None).is_ok();
    let (mut low, mut high) = (0, 3 * jev::TOKEN_CAP);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(mid) { low = mid } else { high = mid - 1 }
    }
    assert!(low > 3 * (jev::TOKEN_CAP - 600), "{low}");
    let plan = jev::plan("Task", &one_question(), &"a".repeat(low), None).unwrap();
    assert_eq!(plan.requests.len(), 1);
    assert!(jev::plan("Task", &one_question(), &"a".repeat(low + 3), None).is_err());
}

// ---- chunking -----------------------------------------------------------------------------

fn file(name: &str, body_chars: usize) -> String {
    format!(
        "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1 @@\n+{}\n",
        "x".repeat(body_chars)
    )
}

#[test]
fn a_diff_over_the_budget_is_split_per_file() {
    // Three files of about 15k tokens each: no two fit together.
    let files: Vec<String> = ["a.rs", "b.rs", "c.rs"].map(|n| file(n, 45_000)).into();
    let diff = files.concat();
    let plan = jev::plan("Task", &rubric(), "answer", Some(&diff)).unwrap();
    assert_eq!(plan.requests.len(), 3);
    assert!(!plan.truncated);
    for (i, (request, file)) in plan.requests.iter().zip(&files).enumerate() {
        assert!(request.state.contains(file.as_str()));
        assert!(request.state.contains(&format!("part {} of 3", i + 1)));
        assert!(request.state.contains("answer"));
        assert_eq!(request.weight, file.chars().count());
    }
}

#[test]
fn small_files_share_a_chunk() {
    let diff = [
        file("a.rs", 45_000),
        file("b.rs", 100),
        file("c.rs", 100),
        file("d.rs", 45_000),
    ]
    .concat();
    let plan = jev::plan("Task", &rubric(), "answer", Some(&diff)).unwrap();
    assert_eq!(
        plan.requests.len(),
        2,
        "{:?}",
        plan.requests.iter().map(|r| r.weight).collect::<Vec<_>>()
    );
    assert!(plan.requests[0].state.contains("b/b.rs") && plan.requests[0].state.contains("b/c.rs"));
}

#[test]
fn a_single_file_over_the_budget_is_truncated_never_sent_whole() {
    let big = file("big.rs", 200_000);
    let plan = jev::plan("Task", &rubric(), "answer", Some(&big)).unwrap();
    assert!(plan.truncated);
    assert_eq!(plan.requests.len(), 1);
    let state = &plan.requests[0].state;
    assert!(state.contains("file truncated"));
    assert!(state.len() < big.len());
    let body = jev::request_body("m", state, &plan.questions);
    // Within the cap by the plan's own estimate.
    assert!(body["state"].as_str().unwrap().chars().count() / 3 < jev::TOKEN_CAP);
}

#[test]
fn the_longest_accepted_answer_with_an_oversized_file_stays_within_the_cap() {
    // Review M-1: at the answer's boundary even the cut note with nothing kept doesn't fit.
    let big = file("big.rs", 100_000);
    let plan = jev::plan("Task", &rubric(), &longest_accepted_answer(), Some(&big)).unwrap();
    assert!(plan.truncated);
    for request in &plan.requests {
        let estimate = jev::estimate(&request.state, &plan.questions);
        assert!(
            estimate <= jev::TOKEN_CAP,
            "{estimate} > {}",
            jev::TOKEN_CAP
        );
    }
}

/// The longest answer `plan` accepts on its own (found by bisection).
fn longest_accepted_answer() -> String {
    let accepted = |n: usize| jev::plan("Task", &rubric(), &"a".repeat(n), None).is_ok();
    let (mut low, mut high) = (0, 3 * jev::TOKEN_CAP);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if accepted(mid) {
            low = mid
        } else {
            high = mid - 1
        }
    }
    "a".repeat(low)
}

#[test]
fn the_longest_accepted_answer_with_10_000_files_is_split_within_the_cap() {
    // Review M-1 (round 2): a five-digit part count must not make a valid diff an error.
    let diff: String = (0..10_000)
        .map(|i| file(&format!("f{i}.rs"), 100))
        .collect();
    let answer = longest_accepted_answer();
    let plan = jev::plan("Task", &rubric(), &answer, Some(&diff)).unwrap();
    assert_eq!(plan.requests.len(), 10_000);
    assert!(plan.requests[9_999].state.contains("part 10000 of 10000"));
    for request in &plan.requests {
        assert!(jev::estimate(&request.state, &plan.questions) <= jev::TOKEN_CAP);
    }
}

#[test]
fn no_diff_and_an_empty_diff_are_said_so() {
    let none = jev::plan("Task", &rubric(), "answer", None).unwrap();
    assert!(none.requests[0].state.contains("(no diff available)"));
    let empty = jev::plan("Task", &rubric(), "answer", Some("")).unwrap();
    assert!(empty.requests[0].state.contains("(no changes)"));
}

// ---- responses ----------------------------------------------------------------------------

fn response(answers: Value) -> String {
    json!({"model": "jev-1.13.0", "answers": answers, "usage": {"input_tokens": 369, "output_tokens": 33}})
        .to_string()
}

#[test]
fn noul_and_score_answers_map_to_0_to_1() {
    let parsed = jev::parse_response(
        &response(json!({
            "correct": {"type": "noul", "noul": 0.95},
            "quality": {"type": "score", "score": 1.05, "confidence": 0.73, "legend": {}, "probabilities": {}},
        })),
        &rubric(),
    )
    .unwrap();
    assert_eq!(parsed.model, "jev-1.13.0");
    assert_eq!(
        parsed.criteria["correct"],
        Scored {
            score: 0.95,
            confidence: None
        }
    );
    assert_eq!(parsed.criteria["quality"].score, 0.525);
    assert_eq!(parsed.criteria["quality"].confidence, Some(0.73));
    assert!(parsed.unscored.is_empty());
    assert_eq!((parsed.input_tokens, parsed.output_tokens), (369, 33));
}

#[test]
fn out_of_range_values_are_clamped() {
    let parsed = jev::parse_response(
        &response(json!({
            "correct": {"type": "noul", "noul": 1.7},
            "quality": {"type": "score", "score": -0.4, "confidence": 2.0},
        })),
        &rubric(),
    )
    .unwrap();
    assert_eq!(parsed.criteria["correct"].score, 1.0);
    assert_eq!(parsed.criteria["quality"].score, 0.0);
    assert_eq!(parsed.criteria["quality"].confidence, Some(1.0));
}

#[test]
fn a_missing_or_mistyped_answer_is_unscored_never_0() {
    let parsed = jev::parse_response(
        &response(json!({"quality": {"type": "noul", "noul": 0.5}})),
        &rubric(),
    )
    .unwrap();
    assert!(parsed.criteria.is_empty());
    assert_eq!(parsed.unscored, ["correct", "quality"]);
}

#[test]
fn a_malformed_response_is_an_error_without_its_text() {
    for text in [
        "not json SECRET-ish",
        r#"{"answers": {}}"#,
        r#"{"model": "jev-1.13.0", "answers": []}"#,
    ] {
        let err = jev::parse_response(text, &rubric()).unwrap_err();
        assert!(err.contains("malformed"), "{err}");
        assert!(!err.contains("SECRET"), "{err}");
    }
}

#[test]
fn chunks_combine_as_the_size_weighted_mean() {
    let answered = |correct: f64, quality: Option<f64>| Answered {
        model: "jev-1.13.0".into(),
        criteria: [(
            "correct".to_owned(),
            Scored {
                score: correct,
                confidence: None,
            },
        )]
        .into_iter()
        .chain(quality.map(|q| {
            (
                "quality".to_owned(),
                Scored {
                    score: q,
                    confidence: Some(0.5),
                },
            )
        }))
        .collect(),
        unscored: vec![],
        input_tokens: 10,
        output_tokens: 1,
    };
    let combined = jev::combine(
        &[(300, answered(1.0, Some(0.5))), (100, answered(0.0, None))],
        &rubric(),
    );
    assert_eq!(combined.criteria["correct"].score, 0.75);
    // Only the first chunk scored quality.
    assert_eq!(combined.criteria["quality"].score, 0.5);
    assert_eq!(combined.input_tokens, 20);
    assert_eq!(combined.model, "jev-1.13.0");
}

// ---- statuses -----------------------------------------------------------------------------

fn command() -> SecretSource {
    SecretSource::Command(PathBuf::from("/opt/jev-token"))
}

#[test]
fn rate_limits_and_overload_are_retried() {
    assert_eq!(jev::classify(429, "", &command()), Outcome::Retry(429));
    assert_eq!(jev::classify(529, "{}", &command()), Outcome::Retry(529));
    assert_eq!(jev::classify(200, "{}", &command()), Outcome::Answered);
}

#[test]
fn a_401_names_the_config_key() {
    let body = r#"{"detail":{"error_type":"authentication_error","message":"Cannot authenticate with the server."}}"#;
    let Outcome::Failed(why) = jev::classify(401, body, &command()) else {
        panic!()
    };
    assert!(
        why.contains("eval.jev.token_command /opt/jev-token") && why.contains("401"),
        "{why}"
    );
    let Outcome::Failed(why) = jev::classify(401, body, &SecretSource::Ref("op://v/j/t".into()))
    else {
        panic!()
    };
    assert!(why.contains("eval.jev.token_ref"), "{why}");
}

#[test]
fn a_400_keeps_the_error_type_and_a_usage_errors_message() {
    let Outcome::Failed(why) = jev::classify(
        400,
        r#"{"detail":{"error_type":"api_usage_error","message":"Unknown model: jev-x"}}"#,
        &command(),
    ) else {
        panic!()
    };
    assert!(
        why.contains("api_usage_error: Unknown model: jev-x"),
        "{why}"
    );
    let Outcome::Failed(why) = jev::classify(
        400,
        r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#,
        &command(),
    ) else {
        panic!()
    };
    assert!(why.contains("max_tokens_exceeded"), "{why}");
}

#[test]
fn a_422_keeps_only_loc_and_msg_never_the_echoed_request() {
    let body = json!({"detail": [{
        "type": "missing", "loc": ["body", "model"], "msg": "Field required",
        "input": {"state": "ECHOED-SENTINEL", "questions": {}},
    }]})
    .to_string();
    let Outcome::Failed(why) = jev::classify(422, &body, &command()) else {
        panic!()
    };
    assert!(why.contains("body.model: Field required"), "{why}");
    assert!(
        !why.contains("ECHOED-SENTINEL") && !why.contains("missing"),
        "{why}"
    );
}

#[test]
fn another_status_never_repeats_the_body() {
    let Outcome::Failed(why) = jev::classify(500, "RAW-BODY-SENTINEL", &command()) else {
        panic!()
    };
    assert!(why.contains("500") && !why.contains("RAW-BODY"), "{why}");
}
