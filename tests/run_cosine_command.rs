//! Issue #63 (M2a slice 12): `sbxm run` end to end with `[eval.cosine]`.
//! No real model file is used: a model dir with garbage bytes makes
//! `FastEmbedder::from_dir` fail for real, exercising the "never fails the
//! run" path without any network or a real `.onnx` file. The happy path
//! (real similarities) is covered by the pure `cosine_repeat` unit tests and
//! the `#[ignore]`d real-model test.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{run, run_show};
use sbxm::eval::cosine::CosineEntry;
use sbxm::run::results;
use serde_json::Value;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TASK: &str = "[task]\nprompt = \"Reply with exactly: PONG\"\n\n";
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n";
const RUBRIC_AND_JUDGE: &str = "[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                                [eval.judge]\nharness = \"claude\"\nmodel = \"judge-model\"\n";

fn pong() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    })
}

/// A model dir with every required file present but not a real model: real
/// enough to pass preflight's existence check, guaranteed to fail to load.
fn garbage_model_dir(env: &Env) -> PathBuf {
    let dir = env.tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    for name in [
        "model.onnx",
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ] {
        std::fs::write(dir.join(name), "not a real model file").unwrap();
    }
    dir
}

struct Ran {
    env: Env,
    summary: run::Summary,
    out: String,
    warn: String,
}

fn go(body: &str, backend: &FakeBackend) -> Ran {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
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

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn cosine_of(ran: &Ran, contestant: usize, repeat: u32) -> Value {
    read_json(
        &meta(ran)
            .join(contestant.to_string())
            .join(repeat.to_string())
            .join("evals.json"),
    )["cosine"]
        .clone()
}

#[test]
fn a_model_that_fails_to_load_is_one_warning_and_an_error_entry_per_pair_without_failing_the_run() {
    let env = Env::new();
    let model_dir = garbage_model_dir(&env);
    let body = format!(
        "{TASK}{TWO_CLAUDES}[eval.cosine]\nmodel_dir = {}\n[run]\nrepeat = 2\n",
        toml::Value::String(model_dir.to_str().unwrap().to_owned())
    );
    let ran = go(&body, &pong());

    assert_eq!(
        ran.warn.matches("cannot load the cosine model").count(),
        1,
        "{}",
        ran.warn
    );
    for repeat in 0..2 {
        for contestant in 0..2 {
            let entry = cosine_of(&ran, contestant, repeat);
            assert!(entry["error"].is_string(), "{entry}");
        }
    }
    // The run still completes normally.
    let record = read_json(&meta(&ran).join("run.json"));
    assert!(record["completed_at"].is_string());
    assert!(ran.out.contains("contestants[0] claude/one"));
}

#[test]
fn a_cosine_model_failure_leaves_the_judges_ranking_unaffected() {
    let env = Env::new();
    let model_dir = garbage_model_dir(&env);
    let backend = pong().with_exec_output_matching(
        "judge-prompt.md",
        ExecOutput {
            stdout: {
                let result = serde_json::json!({
                    "type": "result", "subtype": "success", "is_error": false,
                    "result": serde_json::json!({
                        "A": {"correctness": {"value": true, "reason": "ok"}},
                        "B": {"correctness": {"value": false, "reason": "no"}},
                    }).to_string(),
                    "usage": {"input_tokens": 1, "output_tokens": 1}
                });
                format!("{result}\n")
            },
            stderr: String::new(),
            exit_code: Some(0),
        },
    );
    let body = format!(
        "{TASK}{TWO_CLAUDES}{RUBRIC_AND_JUDGE}[eval.cosine]\nmodel_dir = {}\n",
        toml::Value::String(model_dir.to_str().unwrap().to_owned())
    );
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, &body).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(&env.config_dir(), &path, &backend, &mut out, &mut warn).unwrap();
    let out = String::from_utf8(out).unwrap();
    let warn_text = String::from_utf8(warn).unwrap();

    assert!(
        warn_text.contains("cannot load the cosine model"),
        "{warn_text}"
    );
    assert!(
        out.contains("Ranking (judge scores 0-1, repeats averaged)"),
        "{out}"
    );
    assert!(out.contains("1. contestants["), "{out}");
    let _ = summary;
}

#[test]
fn run_show_prints_the_similarity_lines_from_saved_files() {
    let ran = go(&format!("{TASK}{TWO_CLAUDES}"), &pong());
    let mut peers = BTreeMap::new();
    peers.insert(1usize, 0.87_f32);
    let mut entries = BTreeMap::new();
    entries.insert(0usize, CosineEntry::Peers(peers));
    entries.insert(
        1usize,
        CosineEntry::Peers(BTreeMap::from([(0usize, 0.87_f32)])),
    );
    results::write_cosine(&meta(&ran), 0, &entries).unwrap();

    let text = run_show::render(
        &ran.env.config_dir(),
        &ran.summary.run_id,
        &run_show::Options::default(),
    )
    .unwrap();

    assert!(text.contains("Similarity repeat 1/1: 0-1 0.87"), "{text}");
}
