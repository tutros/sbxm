//! M2a slice 11, pure parts: blind labels, the judge prompt, parsing the
//! judge's reply and mapping values to 0-1 scores (decisions 18, 19, 21; P3, P9).

use sbxm::eval::judge::{self, Candidate};
use sbxm::eval::rubric;
use sbxm::run::config::{Criterion, CriterionKind};
use serde_json::json;

fn pass_fail(id: &str) -> Criterion {
    Criterion {
        id: id.into(),
        kind: CriterionKind::PassFail,
        levels: vec![],
        weight: 1.0,
        notes: None,
    }
}

fn scale(id: &str, levels: &[&str], notes: Option<&str>) -> Criterion {
    Criterion {
        id: id.into(),
        kind: CriterionKind::Scale,
        levels: levels.iter().map(|l| l.to_string()).collect(),
        weight: 0.5,
        notes: notes.map(String::from),
    }
}

fn rubric_two() -> Vec<Criterion> {
    vec![
        pass_fail("correctness"),
        scale(
            "quality",
            &["poor", "fair", "good", "excellent"],
            Some("idiomatic, no dead code"),
        ),
    ]
}

// ---- scores ------------------------------------------------------------------

#[test]
fn pass_fail_maps_to_zero_or_one() {
    let c = pass_fail("a");
    assert_eq!(rubric::score(&c, &json!(true)), Some(1.0));
    assert_eq!(rubric::score(&c, &json!(false)), Some(0.0));
    assert_eq!(rubric::score(&c, &json!("yes")), None);
    assert_eq!(rubric::score(&c, &json!(null)), None);
}

#[test]
fn a_scale_maps_its_level_index_over_n_minus_one() {
    let c = scale("q", &["poor", "fair", "good", "excellent"], None);
    assert_eq!(rubric::score(&c, &json!("poor")), Some(0.0));
    assert_eq!(rubric::score(&c, &json!("good")), Some(2.0 / 3.0));
    assert_eq!(rubric::score(&c, &json!("excellent")), Some(1.0));
    assert_eq!(rubric::score(&c, &json!("great")), None);
    assert_eq!(rubric::score(&c, &json!(true)), None);
    let two = scale("q", &["no", "yes"], None);
    assert_eq!(rubric::score(&two, &json!("yes")), Some(1.0));
}

// ---- blind labels ------------------------------------------------------------

#[test]
fn labels_are_a_permutation_of_letters_in_order() {
    let labels = judge::assign_labels("2026-09-30-abc123", 0, &[0, 1, 2]);

    let letters: Vec<char> = labels.iter().map(|(_, l)| *l).collect();
    assert_eq!(letters, ['A', 'B', 'C']);
    let mut contestants: Vec<usize> = labels.iter().map(|(c, _)| *c).collect();
    contestants.sort();
    assert_eq!(contestants, [0, 1, 2]);
}

#[test]
fn labels_are_deterministic_for_a_run_and_repeat() {
    let first = judge::assign_labels("2026-09-30-abc123", 1, &[0, 1, 2, 3]);
    let again = judge::assign_labels("2026-09-30-abc123", 1, &[0, 1, 2, 3]);
    assert_eq!(first, again);
}

#[test]
fn the_order_varies_across_runs_and_repeats_so_no_contestant_is_always_first() {
    let mut firsts = std::collections::HashSet::new();
    for repeat in 0..8 {
        for run in [
            "2026-09-30-aaaaaa",
            "2026-09-30-bbbbbb",
            "2026-09-30-cccccc",
        ] {
            firsts.insert(judge::assign_labels(run, repeat, &[0, 1, 2])[0].0);
        }
    }
    assert!(firsts.len() >= 2, "{firsts:?}");
}

#[test]
fn only_the_given_contestants_get_labels() {
    let labels = judge::assign_labels("2026-09-30-abc123", 0, &[1, 3]);
    assert_eq!(labels.len(), 2);
    assert!(labels.iter().all(|(c, _)| *c == 1 || *c == 3));
    assert_eq!(
        labels.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
        ['A', 'B']
    );
}

// ---- the prompt --------------------------------------------------------------

fn candidate(label: char, answer: &str, diff: Option<&str>) -> Candidate {
    Candidate {
        label,
        answer: answer.into(),
        diff: diff.map(String::from),
    }
}

#[test]
fn the_prompt_holds_the_task_the_rubric_and_every_candidate_under_its_blind_label() {
    let candidates = [
        candidate('A', "answer of a", Some("diff --git a/x b/x\n+one\n")),
        candidate('B', "answer of b", Some("")),
        candidate('C', "", None),
    ];

    let prompt = judge::build_prompt("Implement feature X", &rubric_two(), &candidates);

    assert!(prompt.contains("Implement feature X"));
    // The rubric: ids, kinds, levels worst to best, notes.
    assert!(prompt.contains("correctness") && prompt.contains("pass_fail"));
    assert!(prompt.contains("quality") && prompt.contains("poor, fair, good, excellent"));
    assert!(prompt.contains("idiomatic, no dead code"));
    // Each candidate under its label, with its answer and diff.
    assert!(prompt.contains("### Candidate A") && prompt.contains("answer of a"));
    assert!(prompt.contains("+one"));
    assert!(prompt.contains("### Candidate B") && prompt.contains("answer of b"));
    assert!(prompt.contains("(no changes)"));
    assert!(
        prompt.contains("### Candidate C")
            && prompt.contains("(no answer)")
            && prompt.contains("(no diff available)")
    );
    // The reply format names every label and criterion.
    assert!(prompt.contains("\"A\"") && prompt.contains("\"C\""));
    assert!(prompt.contains("\"correctness\"") && prompt.contains("\"quality\""));
}

#[test]
fn the_prompt_never_names_a_harness_model_status_or_contestant_index() {
    let candidates = [candidate('A', "x", Some("")), candidate('B', "y", Some(""))];

    let prompt = judge::build_prompt("task", &rubric_two(), &candidates).to_lowercase();

    for leak in [
        "claude",
        "codex",
        "antigravity",
        "contestants[",
        "timed out",
        "timed_out",
        "failed",
        "gpt",
        "gemini",
    ] {
        assert!(!prompt.contains(leak), "{leak}");
    }
}

#[test]
fn very_long_answers_and_diffs_are_cut_with_a_note() {
    let long = "z".repeat(200_000);
    let candidates = [
        candidate('A', &long, Some(&long)),
        candidate('B', "short", Some("tiny")),
    ];

    let prompt = judge::build_prompt("task", &[pass_fail("a")], &candidates);

    assert!(prompt.len() < 150_000, "{}", prompt.len());
    assert!(prompt.contains("[answer truncated:") && prompt.contains("[diff truncated:"));
    assert!(prompt.contains("short") && prompt.contains("tiny"));
}

// ---- parsing the reply --------------------------------------------------------

fn parse(text: &str) -> Result<judge::Parsed, String> {
    judge::parse_response(text, &rubric_two(), &['A', 'B'])
}

const GOOD: &str = r#"{"A": {"correctness": {"value": true, "reason": "works"}, "quality": {"value": "good", "reason": "tidy"}},
 "B": {"correctness": {"value": false, "reason": "crashes"}, "quality": {"value": "poor", "reason": "messy"}}}"#;

#[test]
fn a_well_formed_reply_gives_values_scores_and_reasons() {
    let parsed = parse(GOOD).unwrap();

    let a = &parsed.candidates[&'A'];
    assert_eq!(a.criteria["correctness"].value, json!(true));
    assert_eq!(a.criteria["correctness"].score, 1.0);
    assert_eq!(a.criteria["correctness"].reason, "works");
    assert_eq!(a.criteria["quality"].value, json!("good"));
    assert_eq!(a.criteria["quality"].score, 2.0 / 3.0);
    assert!(a.unscored.is_empty());
    let b = &parsed.candidates[&'B'];
    assert_eq!(b.criteria["correctness"].score, 0.0);
    assert_eq!(b.criteria["quality"].score, 0.0);
}

#[test]
fn json_inside_prose_or_code_fences_is_found() {
    let fenced = format!("Here is my verdict:\n```json\n{GOOD}\n```\nHope that helps.");
    assert_eq!(parse(&fenced).unwrap(), parse(GOOD).unwrap());
    let prose = format!("Sure. {GOOD} Done.");
    assert_eq!(parse(&prose).unwrap(), parse(GOOD).unwrap());
}

#[test]
fn bare_values_without_a_reason_are_accepted() {
    let parsed = parse(r#"{"A": {"correctness": true, "quality": "fair"}, "B": {"correctness": false, "quality": "excellent"}}"#).unwrap();

    assert_eq!(
        parsed.candidates[&'A'].criteria["quality"].value,
        json!("fair")
    );
    assert_eq!(parsed.candidates[&'A'].criteria["quality"].reason, "");
    assert_eq!(parsed.candidates[&'B'].criteria["quality"].score, 1.0);
}

#[test]
fn a_level_is_matched_ignoring_case_and_spaces() {
    let parsed = parse(r#"{"A": {"correctness": true, "quality": " GOOD "}, "B": {"correctness": true, "quality": "Poor"}}"#).unwrap();

    assert_eq!(
        parsed.candidates[&'A'].criteria["quality"].value,
        json!("good")
    );
    assert_eq!(
        parsed.candidates[&'B'].criteria["quality"].value,
        json!("poor")
    );
}

#[test]
fn a_missing_or_invalid_criterion_is_unscored_never_zero() {
    let parsed = parse(
        r#"{"A": {"correctness": "maybe"}, "B": {"correctness": true, "quality": "superb"}}"#,
    )
    .unwrap();

    let a = &parsed.candidates[&'A'];
    assert!(a.criteria.is_empty());
    assert_eq!(a.unscored, ["correctness", "quality"]);
    let b = &parsed.candidates[&'B'];
    assert!(b.criteria.contains_key("correctness") && !b.criteria.contains_key("quality"));
    assert_eq!(b.unscored, ["quality"]);
}

#[test]
fn a_candidate_the_judge_skipped_is_entirely_unscored() {
    let parsed = parse(r#"{"A": {"correctness": true, "quality": "good"}}"#).unwrap();

    assert_eq!(parsed.candidates[&'B'].unscored, ["correctness", "quality"]);
    assert!(parsed.candidates[&'B'].criteria.is_empty());
}

#[test]
fn labels_and_criteria_nobody_asked_about_are_ignored() {
    let parsed = parse(r#"{"A": {"correctness": true, "quality": "good", "vibes": "high"}, "Z": {"correctness": true}, "B": {"correctness": false, "quality": "poor"}}"#).unwrap();

    assert_eq!(parsed.candidates.len(), 2);
    assert!(!parsed.candidates[&'A'].criteria.contains_key("vibes"));
}

#[test]
fn a_reply_without_a_json_object_is_an_error_naming_what_was_seen() {
    for text in ["", "I think A is better.", "[1, 2, 3]", "{not json"] {
        let err = parse(text).unwrap_err();
        assert!(err.contains("no JSON object"), "{text:?}: {err}");
    }
    let err = parse("I think A is better than B, clearly.").unwrap_err();
    assert!(err.contains("I think A is better"), "{err}");
}
