//! M2a slice 12a: code-side scoring and ranking (P9, decisions 19, 120).
//! Each criterion is 0-1; a contestant's score is the weight-normalised mean
//! over its judged criteria, repeats averaged; checks are reported alongside;
//! ties rank equal; nothing unscored counts as 0.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{run, run_show};
use sbxm::eval::score::{self, PairInput};
use sbxm::run::config::{Criterion, CriterionKind};
use serde_json::{Value, json};

fn criterion(id: &str, kind: CriterionKind, weight: f64) -> Criterion {
    Criterion {
        id: id.into(),
        kind,
        levels: if kind == CriterionKind::Scale {
            vec!["poor".into(), "fair".into(), "good".into()]
        } else {
            vec![]
        },
        weight,
        notes: None,
    }
}

fn rubric() -> Vec<Criterion> {
    vec![
        criterion("correct", CriterionKind::PassFail, 1.0),
        criterion("clarity", CriterionKind::Scale, 0.5),
    ]
}

fn scores(pairs: &[(&str, f64)]) -> Option<BTreeMap<String, f64>> {
    Some(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
}

fn pair(contestant: usize, repeat: u32, judge: Option<BTreeMap<String, f64>>) -> PairInput {
    PairInput {
        contestant,
        repeat,
        status: "completed".into(),
        judge,
        checks_passed: 0,
        checks_total: 0,
    }
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} vs {b}");
}

// ---- weighting -------------------------------------------------------------------

#[test]
fn the_score_is_the_weight_normalised_mean_of_the_judged_criteria() {
    // correct = 1.0 (weight 1.0), clarity = 0.5 (weight 0.5): (1*1 + 0.5*0.5) / 1.5
    let ranking = score::rank(
        &rubric(),
        1,
        1,
        &[pair(0, 0, scores(&[("correct", 1.0), ("clarity", 0.5)]))],
    );

    close(ranking.contestants[0].score.unwrap(), 1.25 / 1.5);
    assert!(ranking.contestants[0].unscored.is_empty());
}

#[test]
fn weights_matter_so_the_same_raw_scores_can_be_re_ranked_under_new_rules() {
    let raw = |a: (f64, f64), b: (f64, f64)| {
        vec![
            pair(0, 0, scores(&[("correct", a.0), ("clarity", a.1)])),
            pair(1, 0, scores(&[("correct", b.0), ("clarity", b.1)])),
        ]
    };
    // A is right but unclear; B is wrong but clear.
    let pairs = raw((1.0, 0.0), (0.0, 1.0));

    let correctness_first = score::rank(&rubric(), 2, 1, &pairs);
    assert_eq!(correctness_first.contestants[0].contestant, 0);

    let mut clarity_first = rubric();
    clarity_first[0].weight = 0.1;
    clarity_first[1].weight = 5.0;
    let reranked = score::rank(&clarity_first, 2, 1, &pairs);
    assert_eq!(reranked.contestants[0].contestant, 1);
}

#[test]
fn an_unscored_criterion_is_left_out_and_listed_never_counted_as_zero() {
    let ranking = score::rank(&rubric(), 1, 1, &[pair(0, 0, scores(&[("correct", 1.0)]))]);

    let c = &ranking.contestants[0];
    close(c.score.unwrap(), 1.0);
    assert_eq!(c.unscored, ["clarity"]);
}

#[test]
fn a_pair_with_no_judged_criterion_has_no_score() {
    let ranking = score::rank(&rubric(), 1, 1, &[pair(0, 0, scores(&[]))]);
    assert_eq!(ranking.contestants[0].score, None);
    let no_judge = score::rank(&rubric(), 1, 1, &[pair(0, 0, None)]);
    assert_eq!(no_judge.contestants[0].score, None);
    assert_eq!(no_judge.contestants[0].rank, None);
}

// ---- repeats ----------------------------------------------------------------------

#[test]
fn repeats_are_averaged_per_contestant() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 1.0), ("clarity", 1.0)])),
        pair(0, 1, scores(&[("correct", 0.0), ("clarity", 0.0)])),
    ];

    let ranking = score::rank(&rubric(), 1, 2, &pairs);

    let c = &ranking.contestants[0];
    close(c.score.unwrap(), 0.5);
    assert_eq!((c.repeats_scored, c.repeats_total), (2, 2));
}

#[test]
fn a_repeat_the_judge_could_not_score_is_left_out_of_the_average() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 1.0), ("clarity", 1.0)])),
        pair(0, 1, None),
    ];

    let ranking = score::rank(&rubric(), 1, 2, &pairs);

    let c = &ranking.contestants[0];
    close(c.score.unwrap(), 1.0);
    assert_eq!((c.repeats_scored, c.repeats_total), (1, 2));
}

#[test]
fn repeats_of_one_contestant_never_mix_with_another() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 1.0)])),
        pair(1, 0, scores(&[("correct", 0.0)])),
        pair(0, 1, scores(&[("correct", 1.0)])),
        pair(1, 1, scores(&[("correct", 1.0)])),
    ];

    let ranking = score::rank(&rubric(), 2, 2, &pairs);

    let by = |i: usize| {
        ranking
            .contestants
            .iter()
            .find(|c| c.contestant == i)
            .unwrap()
            .score
            .unwrap()
    };
    close(by(0), 1.0);
    close(by(1), 0.5);
}

// ---- ranking ------------------------------------------------------------------------

#[test]
fn contestants_are_ranked_best_first_and_ties_share_a_rank() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 0.5)])),
        pair(1, 0, scores(&[("correct", 1.0)])),
        pair(2, 0, scores(&[("correct", 0.5)])),
        pair(3, 0, scores(&[("correct", 0.0)])),
    ];

    let ranking = score::rank(&rubric(), 4, 1, &pairs);

    let order: Vec<(usize, Option<usize>)> = ranking
        .contestants
        .iter()
        .map(|c| (c.contestant, c.rank))
        .collect();
    // 1, then two tied for 2, then 4 (competition ranking).
    assert_eq!(
        order,
        [(1, Some(1)), (0, Some(2)), (2, Some(2)), (3, Some(4))]
    );
}

#[test]
fn scores_equal_up_to_rounding_noise_tie() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 0.1 + 0.2)])),
        pair(1, 0, scores(&[("correct", 0.3)])),
    ];

    let ranking = score::rank(&rubric(), 2, 1, &pairs);

    assert_eq!(ranking.contestants[0].rank, Some(1));
    assert_eq!(ranking.contestants[1].rank, Some(1));
}

#[test]
fn unscored_contestants_are_unranked_and_last() {
    let pairs = [pair(0, 0, None), pair(1, 0, scores(&[("correct", 0.2)]))];

    let ranking = score::rank(&rubric(), 2, 1, &pairs);

    assert_eq!(ranking.contestants[0].contestant, 1);
    assert_eq!(ranking.contestants[1].contestant, 0);
    assert_eq!(ranking.contestants[1].rank, None);
}

// ---- status and checks -----------------------------------------------------------------

#[test]
fn a_timed_out_pair_is_scored_on_its_partial_output_and_marked() {
    let mut timed_out = pair(0, 0, scores(&[("correct", 1.0), ("clarity", 1.0)]));
    timed_out.status = "timed_out".into();
    let mut failed = pair(1, 0, scores(&[("correct", 0.0)]));
    failed.status = "failed".into();

    let ranking = score::rank(&rubric(), 2, 1, &[timed_out, failed]);

    let find = |i: usize| {
        ranking
            .contestants
            .iter()
            .find(|c| c.contestant == i)
            .unwrap()
    };
    assert!(find(0).score.is_some() && find(0).timed_out == 1 && find(0).failed == 0);
    assert!(find(1).score.is_some() && find(1).failed == 1);
}

#[test]
fn checks_are_reported_alongside_and_never_change_the_score() {
    let mut with_checks = pair(0, 0, scores(&[("correct", 1.0)]));
    with_checks.checks_passed = 1;
    with_checks.checks_total = 4;
    let mut clean = pair(1, 0, scores(&[("correct", 1.0)]));
    clean.checks_passed = 4;
    clean.checks_total = 4;

    let ranking = score::rank(&rubric(), 2, 1, &[with_checks, clean]);

    let find = |i: usize| {
        ranking
            .contestants
            .iter()
            .find(|c| c.contestant == i)
            .unwrap()
    };
    assert_eq!((find(0).checks_passed, find(0).checks_total), (1, 4));
    close(find(0).score.unwrap(), find(1).score.unwrap());
    assert_eq!(ranking.contestants[0].rank, ranking.contestants[1].rank);
}

// ---- rendering ---------------------------------------------------------------------------

#[test]
fn the_ranking_is_rendered_best_first_with_its_caveats() {
    let mut timed_out = pair(0, 1, scores(&[("correct", 0.0)]));
    timed_out.status = "timed_out".into();
    timed_out.checks_passed = 1;
    timed_out.checks_total = 2;
    let pairs = [
        pair(0, 0, scores(&[("correct", 1.0)])),
        timed_out,
        pair(1, 0, scores(&[("correct", 1.0), ("clarity", 1.0)])),
        pair(1, 1, scores(&[("correct", 1.0), ("clarity", 1.0)])),
        pair(2, 0, None),
        pair(2, 1, None),
    ];
    let ranking = score::rank(&rubric(), 3, 2, &pairs);
    let labels = [
        "claude/model-a".to_owned(),
        "codex/model-b".to_owned(),
        "antigravity/model-c".to_owned(),
    ];

    let text = score::render(&ranking, &labels);

    let expected = "\
Ranking (judge scores 0-1, repeats averaged)
  1. contestants[1] codex/model-b: 1.00 (2/2 repeats scored)
  2. contestants[0] claude/model-a: 0.50 (2/2 repeats scored; checks 1/2 passed; 1 timed out; unscored: clarity)
  -  contestants[2] antigravity/model-c: not scored (the judge scored none of its repeats)
";
    assert_eq!(text, expected);
}

#[test]
fn tied_contestants_print_the_same_rank() {
    let pairs = [
        pair(0, 0, scores(&[("correct", 1.0)])),
        pair(1, 0, scores(&[("correct", 1.0)])),
    ];
    let ranking = score::rank(&rubric(), 2, 1, &pairs);

    let text = score::render(&ranking, &["a/x".to_owned(), "b/y".to_owned()]);

    assert!(text.contains("  1. contestants[0] a/x: 1.00"), "{text}");
    assert!(text.contains("  1. contestants[1] b/y: 1.00"), "{text}");
}

// ---- from a saved run ------------------------------------------------------------------------

const ID: &str = "2026-09-30-abc123";

fn meta(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("runs").join(ID)
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn save_pair(env: &Env, i: usize, r: u32, status: &str, evals: Option<Value>) {
    let dir = meta(env).join(i.to_string()).join(r.to_string());
    write(
        &dir.join("result.json"),
        &json!({"status": status, "error": null, "usage": null, "diff": {"status": "none"}})
            .to_string(),
    );
    if let Some(evals) = evals {
        write(&dir.join("evals.json"), &evals.to_string());
    }
}

fn judged(correct: bool, clarity: &str, score: f64) -> Value {
    json!({"judge": {"label": "A", "status": "ok", "error": null, "criteria": {
        "correct": {"value": correct, "score": if correct {1.0} else {0.0}, "reason": ""},
        "clarity": {"value": clarity, "score": score, "reason": ""}}, "unscored": []}})
}

fn saved_run() -> Env {
    let env = Env::new();
    write(
        &meta(&env).join("run.json"),
        &json!({"run_id": ID, "started_at": "2026-09-30T18:00:00Z", "completed_at": "2026-09-30T18:01:00Z",
                "sbx_version": "0.43.0", "profile": "default", "repeat": 2,
                "contestants": [
                    {"index": 0, "harness": "claude", "model": "model-a", "profile": "default"},
                    {"index": 1, "harness": "codex", "model": "model-b", "profile": "default"}]})
        .to_string(),
    );
    write(
        &meta(&env).join("run-config.toml"),
        "[task]\nprompt = \"p\"\n\n[run]\nrepeat = 2\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"model-a\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"model-b\"\n\n\
         [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [[eval.rubric]]\nid = \"clarity\"\nkind = \"scale\"\nlevels = [\"poor\", \"fair\", \"good\"]\nweight = 0.5\n\n\
         [eval.judge]\nharness = \"claude\"\nmodel = \"j\"\n",
    );
    save_pair(&env, 0, 0, "completed", Some(judged(true, "good", 1.0)));
    save_pair(&env, 0, 1, "completed", Some(judged(true, "poor", 0.0)));
    save_pair(&env, 1, 0, "timed_out", Some(judged(false, "fair", 0.5)));
    save_pair(&env, 1, 1, "completed", Some(judged(false, "poor", 0.0)));
    env
}

#[test]
fn a_saved_run_is_ranked_from_its_files_and_the_config_copy() {
    let env = saved_run();

    let ranking = score::load(&meta(&env)).unwrap().unwrap();

    assert_eq!(ranking.contestants[0].contestant, 0);
    // Repeat 1: correct 1.0 and clarity 1.0 give (1*1 + 1*0.5)/1.5 = 1.0.
    // Repeat 2: correct 1.0 and clarity 0.0 give (1*1 + 0*0.5)/1.5 = 2/3. Mean 5/6.
    close(
        ranking.contestants[0].score.unwrap(),
        (1.0 + 1.0 / 1.5) / 2.0,
    );
    assert_eq!(ranking.contestants[1].timed_out, 1);
    assert!(ranking.contestants[1].score.unwrap() < ranking.contestants[0].score.unwrap());
}

#[test]
fn editing_the_config_copys_weights_re_ranks_the_saved_run() {
    let env = saved_run();
    let before = score::load(&meta(&env)).unwrap().unwrap().contestants[0]
        .score
        .unwrap();

    let path = meta(&env).join("run-config.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("weight = 0.5", "weight = 50.0");
    std::fs::write(&path, text).unwrap();
    let after = score::load(&meta(&env)).unwrap().unwrap().contestants[0]
        .score
        .unwrap();

    assert!((before - after).abs() > 1e-6, "{before} {after}");
}

#[test]
fn a_run_without_a_judge_has_no_ranking() {
    let env = saved_run();
    let path = meta(&env).join("run-config.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("[eval.judge]\nharness = \"claude\"\nmodel = \"j\"\n", "");
    std::fs::write(&path, text).unwrap();

    assert!(score::load(&meta(&env)).unwrap().is_none());
}

#[test]
fn a_pair_that_was_never_saved_counts_as_unscored_not_zero() {
    let env = saved_run();
    std::fs::remove_dir_all(meta(&env).join("1").join("1")).unwrap();

    let ranking = score::load(&meta(&env)).unwrap().unwrap();

    let two = ranking
        .contestants
        .iter()
        .find(|c| c.contestant == 1)
        .unwrap();
    assert_eq!((two.repeats_scored, two.repeats_total), (1, 2));
}

// ---- in the run summary and in run show ---------------------------------------------------------

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");

fn codex_reply(text: &str) -> String {
    let item = json!({"type": "item.completed", "item": {"id": "i", "type": "agent_message", "text": text}});
    format!(
        "{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}\n{item}\n{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":2}}}}\n"
    )
}

fn judged_run() -> (Env, run::Summary, String) {
    let env = Env::new();
    let verdict = json!({"A": {"correct": true, "clarity": "good"}, "B": {"correct": false, "clarity": "poor"}}).to_string();
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_default_exec_output(ExecOutput {
            stdout: CLAUDE_PONG.into(),
            stderr: String::new(),
            exit_code: Some(0),
        })
        .with_exec_output_matching(
            "judge-prompt.md",
            ExecOutput {
                stdout: codex_reply(&verdict),
                stderr: String::new(),
                exit_code: Some(0),
            },
        );
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        "[task]\nprompt = \"p\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n\
         [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [[eval.rubric]]\nid = \"clarity\"\nkind = \"scale\"\nlevels = [\"poor\", \"fair\", \"good\"]\nweight = 0.5\n\n\
         [eval.judge]\nharness = \"codex\"\nmodel = \"j\"\n",
    )
    .unwrap();
    let mut out = Vec::new();
    let summary = run::run(
        &env.config_dir(),
        &path,
        &backend,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();
    (env, summary, String::from_utf8(out).unwrap())
}

#[test]
fn the_run_summary_prints_the_ranking_before_the_results_line() {
    let (_, _, out) = judged_run();

    let lines: Vec<&str> = out.lines().collect();
    let heading = lines
        .iter()
        .position(|l| *l == "Ranking (judge scores 0-1, repeats averaged)")
        .expect(&out);
    assert!(lines[heading + 1].starts_with("  1. contestants["), "{out}");
    assert!(
        lines[heading + 1].contains(": 1.00 (1/1 repeats scored)"),
        "{out}"
    );
    assert!(
        lines[heading + 2].starts_with("  2. contestants[")
            && lines[heading + 2].contains(": 0.00 "),
        "{out}"
    );
    assert!(lines.last().unwrap().starts_with("Results: "), "{out}");
}

#[test]
fn run_show_prints_the_same_ranking_under_its_header() {
    let (env, summary, _) = judged_run();

    let shown = run_show::render(
        &env.config_dir(),
        &summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();

    let header_end = shown.find("\n\n").unwrap();
    let after = &shown[header_end + 2..];
    assert!(
        after.starts_with("Ranking (judge scores 0-1, repeats averaged)\n  1. contestants["),
        "{shown}"
    );
}

#[test]
fn a_run_with_no_judge_prints_no_ranking() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    });
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, "[task]\nprompt = \"p\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n").unwrap();
    let mut out = Vec::new();
    let summary = run::run(
        &env.config_dir(),
        &path,
        &backend,
        &mut out,
        &mut Vec::new(),
    )
    .unwrap();

    assert!(!String::from_utf8(out).unwrap().contains("Ranking"));
    let shown = run_show::render(
        &env.config_dir(),
        &summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();
    assert!(!shown.contains("Ranking"));
}
