//! M2a slice 8: what a run leaves on disk under `<base>/.sbxm/runs/<id>/`
//! (decisions 16, 25, 28, 46, 55, 111, 112): `run.json` written before any
//! sandbox exists, each pair's files written as soon as that pair finishes,
//! and `completed_at` only when everything finished and was saved.

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend};
use sbxm::commands::run;
use sbxm::run::results;
use serde_json::{Value, json};

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TASK: &str = "[task]\nprompt = \"Reply with exactly: PONG\"\n\n";
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n";

fn pong() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    })
}

struct Ran {
    summary: anyhow::Result<run::Summary>,
    out: String,
    warn: String,
    config_text: String,
}

fn go(env: &Env, body: &str, backend: &FakeBackend) -> Ran {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let summary = run::run(&env.config_dir(), &path, backend, &mut out, &mut warn);
    Ran {
        summary,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        config_text: body.to_owned(),
    }
}

fn meta(env: &Env, run_id: &str) -> PathBuf {
    env.base_dir().join(".sbxm").join("runs").join(run_id)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn run_json(env: &Env, run_id: &str) -> Value {
    read_json(&meta(env, run_id).join("run.json"))
}

// ---- files and locations ---------------------------------------------------

#[test]
fn a_run_writes_every_pairs_files_under_contestant_and_repeat() {
    let env = Env::new();
    let body = format!("{TASK}[run]\nrepeat = 2\n\n{TWO_CLAUDES}");

    let ran = go(&env, &body, &pong());

    let id = ran.summary.unwrap().run_id;
    for contestant in 0..2 {
        for repeat in 0..2 {
            let dir = meta(&env, &id)
                .join(contestant.to_string())
                .join(repeat.to_string());
            for file in ["answer.md", "diff.patch", "result.json", "transcript.jsonl"] {
                assert!(dir.join(file).is_file(), "{}", dir.join(file).display());
            }
        }
    }
    assert!(meta(&env, &id).join("kits").is_dir());
    assert!(!meta(&env, &id).join("work").exists());
}

#[test]
fn the_run_config_is_copied_verbatim_with_its_rubric() {
    let env = Env::new();
    let body = format!(
        "{TASK}{TWO_CLAUDES}\n[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n"
    );

    let ran = go(&env, &body, &pong());

    let id = ran.summary.unwrap().run_id;
    let copy = std::fs::read_to_string(meta(&env, &id).join("run-config.toml")).unwrap();
    assert_eq!(copy, ran.config_text);
    assert!(copy.contains("correctness"));
}

#[test]
fn answer_transcript_and_diff_hold_the_pairs_output() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    let id = ran.summary.unwrap().run_id;
    let dir = meta(&env, &id).join("0").join("0");
    assert_eq!(
        std::fs::read_to_string(dir.join("answer.md")).unwrap(),
        "PONG"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("transcript.jsonl")).unwrap(),
        CLAUDE_PONG
    );
    // Unseeded and unchanged by the fake agent: an empty patch, still written.
    assert_eq!(std::fs::read_to_string(dir.join("diff.patch")).unwrap(), "");
}

#[test]
fn a_changed_workspace_shows_up_in_diff_patch() {
    let env = Env::new();
    // The fake agent writes into the only pair workspace that exists when it runs.
    let base = env.base_dir();
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox.ends_with("-0-0") {
            let id = sandbox
                .trim_start_matches("sbxm-run-")
                .trim_end_matches("-0-0");
            let ws = base.join("runs").join(id).join("0").join("0");
            std::fs::write(ws.join("made.txt"), "by the agent\n").unwrap();
        }
    });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let id = ran.summary.unwrap().run_id;
    let patch = std::fs::read_to_string(meta(&env, &id).join("0/0/diff.patch")).unwrap();
    assert!(
        patch.contains("diff --git a/made.txt b/made.txt"),
        "{patch}"
    );
}

// ---- result.json -----------------------------------------------------------

#[test]
fn result_json_records_status_usage_and_identity() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    let id = ran.summary.unwrap().run_id;
    let result = read_json(&meta(&env, &id).join("1/0/result.json"));
    assert_eq!(result["status"], "completed");
    assert_eq!(result["contestant"], 1);
    assert_eq!(result["repeat"], 0);
    assert_eq!(result["harness"], "claude");
    assert_eq!(result["model"], "two");
    assert_eq!(result["sandbox"], format!("sbxm-run-{id}-1-0"));
    assert_eq!(result["usage"]["input_tokens"], 27651);
    assert_eq!(result["usage"]["output_tokens"], 44);
    assert!(result["usage"]["cost_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(result["diff"]["status"], "ok");
    assert!(result["error"].is_null());
}

#[test]
fn a_timeout_is_marked_timed_out_and_keeps_its_partial_files() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
        exit_code: Some(124),
    });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let id = ran.summary.unwrap().run_id;
    let dir = meta(&env, &id).join("0/0");
    assert_eq!(read_json(&dir.join("result.json"))["status"], "timed_out");
    assert!(dir.join("answer.md").is_file() && dir.join("transcript.jsonl").is_file());
}

#[test]
fn a_failed_command_records_why() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: String::new(),
        stderr: "boom\n".into(),
        exit_code: Some(1),
    });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let id = ran.summary.unwrap().run_id;
    let result = read_json(&meta(&env, &id).join("0/0/result.json"));
    assert_eq!(result["status"], "failed");
    assert_eq!(result["error"], "exit code 1: boom");
}

#[test]
fn a_pair_that_never_ran_has_a_result_json_but_no_output_files() {
    let env = Env::new();
    let backend = FakeBackend::failing_create().and_secrets(&["anthropic"]);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let id = ran.summary.unwrap().run_id;
    let dir = meta(&env, &id).join("0/0");
    let result = read_json(&dir.join("result.json"));
    assert_eq!(result["status"], "error");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("cannot create sandbox")
    );
    assert_eq!(result["diff"]["status"], "none");
    for file in ["answer.md", "diff.patch", "transcript.jsonl"] {
        assert!(!dir.join(file).exists(), "{file}");
    }
}

// ---- run.json --------------------------------------------------------------

#[test]
fn run_json_holds_the_runs_identity() {
    let env = Env::new();
    let backend = pong()
        .with_version("0.99.1")
        .with_skills(json!({"store": "/x", "skills": []}));

    let ran = go(
        &env,
        &format!("{TASK}[run]\ncpus = 2\n\n{TWO_CLAUDES}"),
        &backend,
    );

    let id = ran.summary.unwrap().run_id;
    let record = run_json(&env, &id);
    assert_eq!(record["run_id"], id);
    assert_eq!(record["sbx_version"], "0.99.1");
    assert_eq!(record["sbxm_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(record["skills"], json!({"store": "/x", "skills": []}));
    assert_eq!(record["profile"], "default");
    assert_eq!(record["resources"], json!({"cpus": 2, "memory": "8g"}));
    assert_eq!(record["repeat"], 1);
    assert_eq!(
        record["contestants"][1],
        json!({"index": 1, "harness": "claude", "model": "two", "profile": "default"})
    );
    // One entry per harness in use, with the hash and the kit folders.
    let harnesses = record["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 1);
    assert_eq!(harnesses[0]["harness"], "claude");
    assert_eq!(harnesses[0]["config_hash"].as_str().unwrap().len(), 64);
    assert_eq!(harnesses[0]["kits"].as_array().unwrap().len(), 2);
    assert!(record["started_at"].as_str().unwrap().ends_with('Z'));
    assert!(record["completed_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn completed_at_is_only_set_after_every_pair_finished() {
    let env = Env::new();
    let backend = pong();
    let (backend, gate) = backend.with_exec_gate();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, format!("{TASK}{TWO_CLAUDES}")).unwrap();

    std::thread::scope(|scope| {
        let running = scope.spawn(|| {
            run::run(
                &env.config_dir(),
                &path,
                &backend,
                &mut Vec::new(),
                &mut Vec::new(),
            )
        });
        gate.wait_for_blocked(2);

        // Both pairs are mid-run: the run's identity is already on disk, the
        // results and `completed_at` are not.
        let runs = env.base_dir().join(".sbxm").join("runs");
        let id = std::fs::read_dir(&runs)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        let dir = runs.join(&id);
        let record = read_json(&dir.join("run.json"));
        assert!(record["started_at"].is_string());
        assert!(record.get("completed_at").is_none() || record["completed_at"].is_null());
        assert!(dir.join("run-config.toml").is_file());
        assert!(!dir.join("0").exists() && !dir.join("1").exists());

        gate.open();
        running.join().unwrap().unwrap();
        assert!(read_json(&dir.join("run.json"))["completed_at"].is_string());
    });
}

#[test]
fn one_pair_stuck_mid_run_leaves_the_others_results_already_on_disk() {
    let env = Env::new();
    let (backend, gate) = pong().with_exec_gate_for("-1-0");
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, format!("{TASK}{TWO_CLAUDES}")).unwrap();

    std::thread::scope(|scope| {
        let running = scope.spawn(|| {
            run::run(
                &env.config_dir(),
                &path,
                &backend,
                &mut Vec::new(),
                &mut Vec::new(),
            )
        });
        gate.wait_for_blocked(1);

        // Contestant 0 finished; its files are written although contestant 1 hasn't returned.
        let runs = env.base_dir().join(".sbxm").join("runs");
        let dir = runs.join(
            std::fs::read_dir(&runs)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dir.join("0/0/result.json").is_file() {
            assert!(
                std::time::Instant::now() < deadline,
                "contestant 0's results never appeared"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(dir.join("0/0/answer.md").is_file());
        assert!(!dir.join("1/0").exists());
        let record = read_json(&dir.join("run.json"));
        assert!(record.get("completed_at").is_none() || record["completed_at"].is_null());

        gate.open();
        running.join().unwrap().unwrap();
        assert!(dir.join("1/0/result.json").is_file());
    });
}

#[test]
fn a_failed_save_is_a_warning_and_leaves_completed_at_unset() {
    let env = Env::new();
    let base = env.base_dir();
    // While contestant 0 runs, put a file where its results folder must go.
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox.ends_with("-0-0") {
            let runs = base.join(".sbxm").join("runs");
            let dir = runs.join(
                std::fs::read_dir(&runs)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .file_name(),
            );
            std::fs::write(dir.join("0"), "in the way").unwrap();
        }
    });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let id = ran.summary.unwrap().run_id;
    assert!(
        ran.warn
            .contains("cannot save the results of contestants[0]"),
        "{}",
        ran.warn
    );
    // The other contestant is saved, and the run is not marked complete.
    assert!(meta(&env, &id).join("1/0/result.json").is_file());
    let record = run_json(&env, &id);
    assert!(record.get("completed_at").is_none() || record["completed_at"].is_null());
}

#[test]
fn no_temp_files_are_left_next_to_run_json() {
    let env = Env::new();

    let id = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong())
        .summary
        .unwrap()
        .run_id;

    let names: Vec<String> = std::fs::read_dir(meta(&env, &id))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
}

#[test]
fn the_run_prints_where_the_results_are() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    let id = ran.summary.unwrap().run_id;
    let last = ran.out.lines().last().unwrap();
    assert_eq!(last, format!("Results: {}", meta(&env, &id).display()));
}

#[test]
fn a_run_that_is_refused_up_front_writes_no_run_json() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad").and_secrets(&["anthropic"]);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    assert!(ran.summary.is_err());
    let runs = env.base_dir().join(".sbxm").join("runs");
    for entry in std::fs::read_dir(&runs).unwrap() {
        assert!(!entry.unwrap().path().join("run.json").exists());
    }
}

// ---- timestamps ------------------------------------------------------------

#[test]
fn timestamps_are_utc_rfc3339() {
    assert_eq!(results::format_timestamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(
        results::format_timestamp(1_790_000_000),
        "2026-09-21T14:13:20Z"
    );
    assert_eq!(
        results::format_timestamp(20_726 * 86_400 + 3_661),
        "2026-09-30T01:01:01Z"
    );
}
