//! Issue 343 (M2a slice 14b): Jev's raw questions in a request, under 14a's budget and
//! splitting rules (decision 241), without a sandbox.

use std::collections::BTreeMap;

use sbxm::eval::jev::{self, Answered, RawAnswer};
use sbxm::run::config::{Criterion, CriterionKind, JevQuestion, QuestionKind};
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("fixtures/jev/request-raw-questions.json");

fn rubric() -> Vec<Criterion> {
    vec![
        Criterion {
            id: "correct".into(),
            kind: CriterionKind::PassFail,
            levels: vec![],
            weight: 1.0,
            notes: Some("the function returns the right sum".into()),
        },
        Criterion {
            id: "quality".into(),
            kind: CriterionKind::Scale,
            levels: ["poor", "fair", "good"].map(String::from).into(),
            weight: 0.5,
            notes: None,
        },
    ]
}

fn noul(id: &str, question: &str) -> JevQuestion {
    JevQuestion {
        id: id.into(),
        kind: QuestionKind::Noul,
        question: question.into(),
        options: BTreeMap::new(),
        levels: vec![],
    }
}

fn choice(id: &str, question: &str, options: &[(&str, &str)]) -> JevQuestion {
    JevQuestion {
        id: id.into(),
        kind: QuestionKind::Choice,
        question: question.into(),
        options: options
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        levels: vec![],
    }
}

fn score(id: &str, question: &str, levels: &[&str]) -> JevQuestion {
    JevQuestion {
        id: id.into(),
        kind: QuestionKind::Score,
        question: question.into(),
        options: BTreeMap::new(),
        levels: levels.iter().map(|l| l.to_string()).collect(),
    }
}

fn raw() -> Vec<JevQuestion> {
    vec![
        noul("tests", "Did the contestant add tests?"),
        choice(
            "approach",
            "Which approach did the contestant take?",
            &[
                ("refactor", "Restructured the existing code"),
                ("rewrite", "Replaced it"),
            ],
        ),
        score(
            "depth",
            "How fully are the edge cases handled?",
            &["none", "partial", "full"],
        ),
    ]
}

const DIFF: &str = "diff --git a/src/sum.rs b/src/sum.rs\n--- a/src/sum.rs\n+++ b/src/sum.rs\n@@ -1 +1 @@\n-fn sum() {}\n+fn sum(a: i32, b: i32) -> i32 { a + b }\n";

#[test]
fn raw_questions_go_next_to_the_rubric_in_one_request() {
    let plan = jev::plan("Write sum.", &rubric(), &raw(), "Added sum.", Some(DIFF)).unwrap();
    assert_eq!(plan.requests.len(), 1);
    let body = jev::request_body("jev-latest", &plan.requests[0].state, &plan.questions);
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(
        body,
        golden,
        "{}",
        serde_json::to_string_pretty(&body).unwrap()
    );
    let questions = &body["questions"];
    assert_eq!(questions["tests"]["type"], "noul");
    // S12: a Choice's `criteria` is an object id -> description.
    assert_eq!(questions["approach"]["type"], "choice");
    assert_eq!(
        questions["approach"]["criteria"],
        json!({"refactor": "Restructured the existing code", "rewrite": "Replaced it"})
    );
    assert_eq!(questions["depth"]["type"], "score");
    assert_eq!(
        questions["depth"]["criteria"],
        json!(["none", "partial", "full"])
    );
}

// ---- the 32k limit: the state plus the longest question -----------------------------------

fn file(name: &str, body_chars: usize) -> String {
    format!(
        "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1 @@\n+{}\n",
        "x".repeat(body_chars)
    )
}

/// A raw question longer than every rubric question.
fn long_question() -> Vec<JevQuestion> {
    vec![noul("long", &"q".repeat(30_000))]
}

fn two_files(first: usize) -> String {
    [file("a.rs", first), file("b.rs", 100)].concat()
}

#[test]
fn a_raw_question_that_is_the_longest_counts_and_at_the_limit_is_sent_whole() {
    let questions = jev::questions("Task", &rubric(), &long_question());
    let tokens = |id: &str| questions[id].to_string().chars().count().div_ceil(3);
    assert!(tokens("long") > tokens("correct") && tokens("long") > tokens("quality"));

    let whole = |n: usize| {
        jev::plan(
            "Task",
            &rubric(),
            &long_question(),
            "answer",
            Some(&two_files(n)),
        )
        .unwrap()
        .requests
        .len()
            == 1
    };
    let (mut low, mut high) = (0, 3 * jev::TOKEN_CAP);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if whole(mid) {
            low = mid
        } else {
            high = mid - 1
        }
    }
    // Exactly at the limit: one request, with the whole diff.
    let at = jev::plan(
        "Task",
        &rubric(),
        &long_question(),
        "answer",
        Some(&two_files(low)),
    )
    .unwrap();
    assert_eq!(at.requests.len(), 1);
    assert!(!at.truncated);
    assert_eq!(
        jev::estimate(&at.requests[0].state, &at.questions),
        jev::TOKEN_CAP
    );
    // One token over: 14a's path, the diff split per file.
    let over = jev::plan(
        "Task",
        &rubric(),
        &long_question(),
        "answer",
        Some(&two_files(low + 1)),
    )
    .unwrap();
    assert_eq!(over.requests.len(), 2);
    assert!(over.requests[0].state.contains("part 1 of 2"));
    for request in &over.requests {
        assert!(jev::estimate(&request.state, &over.questions) <= jev::TOKEN_CAP);
    }
}

#[test]
fn a_question_too_big_on_its_own_makes_no_call_and_names_the_question() {
    let questions = vec![
        noul("small", "Short?"),
        noul("huge", &"q".repeat(3 * jev::TOKEN_CAP)),
    ];
    let err = jev::plan("Task", &rubric(), &questions, "answer", Some(DIFF)).unwrap_err();
    assert!(
        err.contains("question `huge`") && err.contains("no call"),
        "{err}"
    );
    assert!(!err.contains("qqqq"), "{err}");
}

#[test]
fn a_task_too_big_is_still_named_as_the_task_with_raw_questions() {
    let task = "t".repeat(3 * jev::TOKEN_CAP);
    let err = jev::plan(&task, &rubric(), &raw(), "answer", Some(DIFF)).unwrap_err();
    assert!(err.contains("the task is too long"), "{err}");
}

/// 255 options, each with a description of `chars` characters.
fn many_options(chars: usize) -> JevQuestion {
    let options: Vec<(String, String)> = (0..255)
        .map(|i| (format!("o{i}"), format!("{i:03}{}", "d".repeat(chars))))
        .collect();
    JevQuestion {
        options: options.into_iter().collect(),
        ..choice("approach", "Which?", &[])
    }
}

#[test]
fn the_largest_choice_counts_every_option_toward_its_size() {
    // 255 descriptions of 300 characters: about 26k tokens, the longest question by far.
    let question = many_options(300);
    let questions = jev::questions("Task", &rubric(), std::slice::from_ref(&question));
    let size = questions["approach"].to_string().chars().count();
    assert!(size > 255 * 300, "{size}");
    let plan = jev::plan(
        "Task",
        &rubric(),
        std::slice::from_ref(&question),
        "answer",
        Some(DIFF),
    )
    .unwrap();
    for request in &plan.requests {
        let estimate = jev::estimate(&request.state, &plan.questions);
        assert!(
            estimate > 255 * 100 && estimate <= jev::TOKEN_CAP,
            "{estimate}"
        );
    }
    // Longer descriptions make the same question too big on its own.
    let err = jev::plan(
        "Task",
        &rubric(),
        &[many_options(400)],
        "answer",
        Some(DIFF),
    )
    .unwrap_err();
    assert!(err.contains("question `approach`"), "{err}");
}

// ---- answers ------------------------------------------------------------------------------

fn response(answers: Value) -> String {
    json!({"model": "jev-1.14.0", "answers": answers, "usage": {"input_tokens": 400, "output_tokens": 40}})
        .to_string()
}

fn rubric_answers() -> Value {
    json!({
        "correct": {"type": "noul", "noul": 0.9},
        "quality": {"type": "score", "score": 2.0, "confidence": 0.8},
    })
}

fn with(extra: Value) -> Value {
    let mut all = rubric_answers();
    for (k, v) in extra.as_object().unwrap() {
        all[k] = v.clone();
    }
    all
}

#[test]
fn raw_answers_are_kept_in_jevs_own_terms_next_to_the_criteria() {
    let parsed = jev::parse_response(
        &response(with(json!({
            "tests": {"type": "noul", "noul": 0.95},
            "approach": {"type": "choice", "choice": "refactor", "confidence": 0.9,
                         "probabilities": {"refactor": 0.72, "rewrite": 0.28}},
            "depth": {"type": "score", "score": 1.82, "confidence": 0.73,
                      "legend": {}, "probabilities": {}},
        }))),
        &rubric(),
        &raw(),
    )
    .unwrap();
    // The rubric is scored as before; the raw questions never join the criteria.
    assert_eq!(
        parsed.criteria.keys().collect::<Vec<_>>(),
        ["correct", "quality"]
    );
    assert_eq!(parsed.questions["tests"], RawAnswer::Noul { yes: 0.95 });
    assert_eq!(
        parsed.questions["approach"],
        RawAnswer::Choice {
            choice: "refactor".into(),
            probability: 0.72,
            probabilities: [("refactor".into(), 0.72), ("rewrite".into(), 0.28)].into(),
        }
    );
    assert_eq!(
        parsed.questions["depth"],
        RawAnswer::Score {
            score: 1.82,
            confidence: Some(0.73)
        }
    );
    assert!(parsed.unanswered.is_empty());
}

#[test]
fn a_missing_mistyped_or_unknown_raw_answer_is_unanswered_and_scores_stay() {
    let parsed = jev::parse_response(
        &response(with(json!({
            "tests": {"type": "score", "score": 1.0},
            "approach": {"type": "choice", "choice": "delete-everything", "confidence": 1.0},
        }))),
        &rubric(),
        &raw(),
    )
    .unwrap();
    assert!(parsed.questions.is_empty());
    assert_eq!(parsed.unanswered, ["tests", "approach", "depth"]);
    assert_eq!(parsed.criteria.len(), 2);
    assert!(parsed.unscored.is_empty());
}

#[test]
fn raw_values_are_clamped_and_a_distribution_keeps_only_the_options() {
    let parsed = jev::parse_response(
        &response(with(json!({
            "tests": {"type": "noul", "noul": 1.4},
            "approach": {"type": "choice", "choice": "rewrite",
                         "probabilities": {"rewrite": 1.3, "refactor": -0.1, "other": 0.5}},
            "depth": {"type": "score", "score": 7.0, "confidence": -1.0},
        }))),
        &rubric(),
        &raw(),
    )
    .unwrap();
    assert_eq!(parsed.questions["tests"], RawAnswer::Noul { yes: 1.0 });
    assert_eq!(
        parsed.questions["approach"],
        RawAnswer::Choice {
            choice: "rewrite".into(),
            probability: 1.0,
            probabilities: [("refactor".into(), 0.0), ("rewrite".into(), 1.0)].into(),
        }
    );
    // Three levels: 0 to 2.
    assert_eq!(
        parsed.questions["depth"],
        RawAnswer::Score {
            score: 2.0,
            confidence: Some(0.0)
        }
    );
}

#[test]
fn a_choice_without_its_picks_probability_falls_back_to_its_confidence() {
    let parsed = jev::parse_response(
        &response(with(json!({
            "approach": {"type": "choice", "choice": "refactor", "confidence": 0.6},
        }))),
        &rubric(),
        &raw(),
    )
    .unwrap();
    assert_eq!(
        parsed.questions["approach"],
        RawAnswer::Choice {
            choice: "refactor".into(),
            probability: 0.6,
            probabilities: BTreeMap::new(),
        }
    );
}

fn answered(questions: &[(&str, RawAnswer)]) -> Answered {
    Answered {
        model: "jev-1.14.0".into(),
        questions: questions
            .iter()
            .map(|(id, a)| (id.to_string(), a.clone()))
            .collect(),
        input_tokens: 10,
        output_tokens: 1,
        ..Answered::default()
    }
}

fn picked(choice: &str, probabilities: &[(&str, f64)]) -> RawAnswer {
    let probabilities: BTreeMap<String, f64> = probabilities
        .iter()
        .map(|(id, p)| (id.to_string(), *p))
        .collect();
    RawAnswer::Choice {
        choice: choice.into(),
        probability: probabilities[choice],
        probabilities,
    }
}

#[test]
fn chunk_answers_combine_by_size_and_a_choice_picks_the_most_probable_mean() {
    let first = answered(&[
        ("tests", RawAnswer::Noul { yes: 1.0 }),
        (
            "approach",
            picked("refactor", &[("refactor", 0.6), ("rewrite", 0.4)]),
        ),
        (
            "depth",
            RawAnswer::Score {
                score: 2.0,
                confidence: Some(0.8),
            },
        ),
    ]);
    let second = answered(&[
        ("tests", RawAnswer::Noul { yes: 0.0 }),
        (
            "approach",
            picked("rewrite", &[("refactor", 0.0), ("rewrite", 1.0)]),
        ),
    ]);
    let combined = jev::combine(&[(100, first), (300, second)], &rubric(), &raw());
    assert_eq!(combined.questions["tests"], RawAnswer::Noul { yes: 0.25 });
    // refactor: (100 * 0.6) / 400 = 0.15; rewrite: (100 * 0.4 + 300 * 1.0) / 400 = 0.85.
    assert_eq!(
        combined.questions["approach"],
        RawAnswer::Choice {
            choice: "rewrite".into(),
            probability: 0.85,
            probabilities: [("refactor".into(), 0.15), ("rewrite".into(), 0.85)].into(),
        }
    );
    // Only the first chunk answered depth.
    assert_eq!(
        combined.questions["depth"],
        RawAnswer::Score {
            score: 2.0,
            confidence: Some(0.8)
        }
    );
    assert!(combined.unanswered.is_empty());
}

#[test]
fn a_raw_question_no_chunk_answered_stays_unanswered() {
    let combined = jev::combine(
        &[(100, answered(&[])), (100, answered(&[]))],
        &rubric(),
        &raw(),
    );
    assert!(combined.questions.is_empty());
    assert_eq!(combined.unanswered, ["tests", "approach", "depth"]);
}
