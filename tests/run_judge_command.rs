//! M2a slice 11, end to end with the fake backend: the judge runs after every
//! pair is done, its verdicts land in each pair's evals.json with the blind
//! label, the mapping is stored per repeat index, and `run show` reveals it.

mod common;

use std::path::PathBuf;

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{run, run_show};
use serde_json::{Value, json};

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TASK: &str = "[task]\nprompt = \"Implement feature X\"\n\n";
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n";
const RUBRIC: &str = "[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                      [[eval.rubric]]\nid = \"quality\"\nkind = \"scale\"\nlevels = [\"poor\", \"good\"]\nweight = 0.5\n\n";
const JUDGE: &str = "[eval.judge]\nharness = \"codex\"\nmodel = \"judge-model\"\n";

fn claude_reply(text: &str) -> String {
    let result = json!({
        "type": "result", "subtype": "success", "is_error": false, "result": text,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    format!("{result}\n")
}

/// A Codex NDJSON run whose final agent message is `text`.
fn codex_reply(text: &str) -> String {
    let item = json!({"type": "item.completed", "item": {"id": "i", "type": "agent_message", "text": text}});
    format!(
        "{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}\n{item}\n{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":2}}}}\n"
    )
}

fn out(stdout: &str, stderr: &str, code: i32) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(code),
    }
}

fn verdict() -> String {
    json!({
        "A": {"correctness": {"value": true, "reason": "works"}, "quality": {"value": "good", "reason": "tidy"}},
        "B": {"correctness": {"value": false, "reason": "crashes"}, "quality": {"value": "poor", "reason": "messy"}},
    })
    .to_string()
}

/// Claude contestants answer PONG; the Codex judge gives `judge_reply`.
fn backend(judge_reply: ExecOutput) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_default_exec_output(out(CLAUDE_PONG, "", 0))
        .with_exec_output_matching("judge-prompt.md", judge_reply)
}

struct Ran {
    env: Env,
    summary: run::Summary,
    out: String,
    warn: String,
}

fn go(run_table: &str, judge: &str, backend: &FakeBackend) -> Ran {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!("{TASK}{run_table}{TWO_CLAUDES}{RUBRIC}{judge}"),
    )
    .unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(&env.config_dir(), &path, backend, &mut out, &mut warn).unwrap();
    Ran {
        env,
        summary,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn meta(ran: &Ran) -> PathBuf {
    ran.env
        .base_dir()
        .join(".sbxm")
        .join("runs")
        .join(&ran.summary.run_id)
}

fn read(path: PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn judge_of(ran: &Ran, contestant: usize, repeat: u32) -> Value {
    read(
        meta(ran)
            .join(contestant.to_string())
            .join(repeat.to_string())
            .join("evals.json"),
    )["judge"]
        .clone()
}

#[test]
fn every_pair_gets_its_label_and_scores_in_evals_json() {
    let ran = go("", JUDGE, &backend(out(&codex_reply(&verdict()), "", 0)));

    let (a, b) = (judge_of(&ran, 0, 0), judge_of(&ran, 1, 0));
    let mut labels = [a["label"].as_str().unwrap(), b["label"].as_str().unwrap()];
    labels.sort();
    assert_eq!(labels, ["A", "B"]);
    for entry in [&a, &b] {
        assert_eq!(entry["status"], "ok");
        assert!(entry["error"].is_null());
        assert_eq!(
            entry["judge"],
            json!({"harness": "codex", "model": "judge-model"})
        );
        assert_eq!(entry["unscored"], json!([]));
    }
    // The labelled scores land on the right contestant.
    let label_a = if a["label"] == "A" { &a } else { &b };
    assert_eq!(
        label_a["criteria"]["correctness"],
        json!({"value": true, "score": 1.0, "reason": "works"})
    );
    let label_b = if a["label"] == "A" { &b } else { &a };
    assert_eq!(label_b["criteria"]["quality"]["value"], "poor");
    assert_eq!(label_b["criteria"]["quality"]["score"], 0.0);
}

#[test]
fn the_label_mapping_is_stored_per_repeat_index_with_the_judges_reply() {
    let ran = go(
        "[run]\nrepeat = 2\n\n",
        JUDGE,
        &backend(out(&codex_reply(&verdict()), "", 0)),
    );

    for repeat in 0..2 {
        let dir = meta(&ran).join("judge").join(repeat.to_string());
        let record = read(dir.join("judge.json"));
        assert_eq!(record["repeat"], repeat);
        assert_eq!(record["status"], "ok");
        assert_eq!(
            record["sandbox"],
            format!("sbxm-run-{}-judge-{repeat}", ran.summary.run_id)
        );
        // Label to contestant index, both contestants present.
        let mapping = record["labels"].as_object().unwrap();
        let mut contestants: Vec<u64> = mapping.values().map(|v| v.as_u64().unwrap()).collect();
        contestants.sort();
        assert_eq!(contestants, [0, 1]);
        assert_eq!(
            std::fs::read_to_string(dir.join("reply.txt")).unwrap(),
            verdict()
        );
        assert!(
            !std::fs::read_to_string(dir.join("transcript.jsonl"))
                .unwrap()
                .is_empty()
        );
        // Each pair's own label agrees with the mapping.
        for (label, contestant) in mapping {
            assert_eq!(
                judge_of(&ran, contestant.as_u64().unwrap() as usize, repeat)["label"],
                *label
            );
        }
    }
}

#[test]
fn the_judge_runs_once_per_repeat_after_every_pair_and_is_removed() {
    let fake = backend(out(&codex_reply(&verdict()), "", 0));
    let ran = go("[run]\nrepeat = 2\n\n", JUDGE, &fake);

    let log = fake.log();
    let id = &ran.summary.run_id;
    let last_pair_rm = log
        .iter()
        .rposition(|l| l.starts_with("rm ") && !l.contains("-judge-"))
        .unwrap();
    let first_judge_create = log
        .iter()
        .position(|l| l == &format!("create sbxm-run-{id}-judge-0"))
        .unwrap();
    assert!(last_pair_rm < first_judge_create, "{log:?}");
    for repeat in 0..2 {
        assert!(log.contains(&format!("create sbxm-run-{id}-judge-{repeat}")));
        assert!(log.contains(&format!("rm sbxm-run-{id}-judge-{repeat}")));
    }
    assert!(
        ran.out
            .contains("Judge codex/judge-model: repeat 1/2 scored 2 contestants"),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("Judge codex/judge-model: repeat 2/2 scored 2 contestants"),
        "{}",
        ran.out
    );
    // The run completed.
    assert!(read(meta(&ran).join("run.json"))["completed_at"].is_string());
}

#[test]
fn a_failing_judge_is_recorded_warned_about_and_does_not_fail_the_run() {
    let ran = go("", JUDGE, &backend(out(&codex_reply("I prefer A."), "", 0)));

    for contestant in 0..2 {
        let entry = judge_of(&ran, contestant, 0);
        assert_eq!(entry["status"], "error");
        assert!(entry["error"].as_str().unwrap().contains("no JSON object"));
        // The label is still recorded, so the mapping is not lost.
        assert!(entry["label"].is_string());
    }
    assert!(
        ran.warn.contains("warning: the judge failed for repeat 1"),
        "{}",
        ran.warn
    );
    assert!(
        ran.out.contains(
            "Judge codex/judge-model: repeat 1/1 failed: the judge's reply has no JSON object"
        ),
        "{}",
        ran.out
    );
    assert!(read(meta(&ran).join("run.json"))["completed_at"].is_string());
}

#[test]
fn criteria_the_judge_skipped_are_listed_as_unscored() {
    let partial =
        json!({"A": {"correctness": true}, "B": {"correctness": false, "quality": "poor"}})
            .to_string();
    let ran = go("", JUDGE, &backend(out(&codex_reply(&partial), "", 0)));

    let entries = [judge_of(&ran, 0, 0), judge_of(&ran, 1, 0)];
    let a = entries.iter().find(|e| e["label"] == "A").unwrap();
    assert_eq!(a["unscored"], json!(["quality"]));
    assert!(a["criteria"].get("quality").is_none());
}

#[test]
fn without_a_judge_nothing_judge_related_is_written_or_printed() {
    let ran = go("", "", &backend(out("", "", 0)));

    assert!(!meta(&ran).join("judge").exists());
    assert!(!ran.out.contains("Judge"), "{}", ran.out);
    assert!(!meta(&ran).join("0/0/evals.json").exists());
}

#[test]
fn a_shared_provider_warning_reaches_the_warning_stream() {
    let claude_judge = "[eval.judge]\nharness = \"claude\"\nmodel = \"j\"\n";
    let fake = FakeBackend::with_secrets(&["anthropic"])
        .with_default_exec_output(out(CLAUDE_PONG, "", 0))
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));

    let ran = go("", claude_judge, &fake);

    assert!(
        ran.warn.starts_with("warning: the judge (claude) uses the same provider (anthropic) as contestants[0] (claude), contestants[1] (claude)"),
        "{}",
        ran.warn
    );
}

#[test]
fn the_checks_and_the_judge_share_one_evals_json() {
    let checks = "[[eval.checks]]\nid = \"t\"\ncommand = \"true\"\n\n";
    let env_run = go(
        "",
        &format!("{checks}{JUDGE}"),
        &backend(out(&codex_reply(&verdict()), "", 0)),
    );

    let evals = read(meta(&env_run).join("0/0/evals.json"));
    assert!(
        evals["checks"].is_array() && evals["judge"].is_object(),
        "{evals}"
    );
}

#[test]
fn run_show_reveals_the_label_scores_and_reasons() {
    let ran = go("", JUDGE, &backend(out(&codex_reply(&verdict()), "", 0)));

    let shown = run_show::render(
        &ran.env.config_dir(),
        &ran.summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();

    // The candidate that got label A won on both criteria; find it by its label.
    let a_contestant = (0..2)
        .find(|&i| judge_of(&ran, i, 0)["label"] == "A")
        .unwrap();
    // Contestant blocks start at a line beginning `contestants[`; the ranking's lines are indented.
    let block = shown.split("\ncontestants[").nth(a_contestant + 1).unwrap();
    assert!(
        block.contains("    Judge (candidate A): correctness true (1.00), quality good (1.00)\n      correctness: works\n      quality: tidy\n"),
        "{shown}"
    );
    assert!(
        shown.contains("Judge (candidate B): correctness false (0.00), quality poor (0.00)"),
        "{shown}"
    );
}

#[test]
fn run_show_reports_a_judge_that_did_not_score_and_unscored_criteria() {
    let failed = go(
        "",
        JUDGE,
        &backend(out(&codex_reply("no json here"), "", 0)),
    );
    let shown = run_show::render(
        &failed.env.config_dir(),
        &failed.summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();
    assert!(
        shown.contains("Judge (candidate A): not scored: the judge's reply has no JSON object"),
        "{shown}"
    );

    let partial =
        json!({"A": {"correctness": true}, "B": {"correctness": false, "quality": "poor"}})
            .to_string();
    let ran = go("", JUDGE, &backend(out(&codex_reply(&partial), "", 0)));
    let shown = run_show::render(
        &ran.env.config_dir(),
        &ran.summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();
    assert!(
        shown.contains("(candidate A): correctness true (1.00); unscored: quality"),
        "{shown}"
    );
}
