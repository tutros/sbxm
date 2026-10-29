//! M2a slice 6: `sbxm run <config>` end to end with the fake backend
//! (decisions 6, 16, 95, 98, 107). Checks first, then kits, then sandboxes.

mod common;

use std::path::PathBuf;

use assert_cmd::Command;
use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::run;
use sbxm::run::id;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TASK: &str = "[task]\nprompt = \"Reply with exactly: PONG\"\n\n";
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n";

fn pong() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic", "openai"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    })
}

struct Ran {
    result: anyhow::Result<run::Summary>,
    out: String,
    warn: String,
}

fn go(env: &Env, body: &str, backend: &FakeBackend) -> Ran {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run::run(
        &env.config_dir(),
        &path,
        &Default::default(),
        backend,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn nothing_written(env: &Env) {
    assert_eq!(std::fs::read_dir(env.base_dir()).unwrap().count(), 0);
}

#[test]
fn a_run_prints_its_id_and_one_line_per_contestant() {
    let env = Env::new();
    let backend = pong();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let summary = ran.result.unwrap();
    assert!(id::is_valid(&summary.run_id));
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines[0], format!("Run {}", summary.run_id));
    assert!(
        lines[1].starts_with("contestants[0] claude/one: completed"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("contestants[1] claude/two: completed"),
        "{}",
        lines[2]
    );
    assert!(
        lines[1].contains("27651 in") && lines[1].contains("44 out"),
        "{}",
        lines[1]
    );
    // The run ID, one line per contestant, and where the results are.
    assert_eq!(lines.len(), 4, "{}", ran.out);
    assert!(lines[3].starts_with("Results: "), "{}", lines[3]);
    assert_eq!(ran.warn, "");
}

#[test]
fn kits_are_validated_before_the_first_sandbox_and_every_sandbox_is_removed() {
    let env = Env::new();
    let backend = pong();

    let summary = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend)
        .result
        .unwrap();

    let log = backend.log();
    let last_validate = log
        .iter()
        .rposition(|l| l.starts_with("validate "))
        .unwrap();
    let first_create = log.iter().position(|l| l.starts_with("create ")).unwrap();
    assert!(last_validate < first_create, "{log:?}");
    assert_eq!(backend.creates().len(), 2);
    let mut created: Vec<_> = backend.creates().into_iter().map(|c| c.name).collect();
    let mut removed = backend.removes();
    created.sort();
    removed.sort();
    assert_eq!(created, removed);
    assert!(created[0].starts_with(&format!("sbxm-run-{}-0-0", summary.run_id)));
}

#[test]
fn results_land_in_the_two_run_roots_and_workspaces_stay() {
    let env = Env::new();

    let summary = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong())
        .result
        .unwrap();

    let base = env.base_dir();
    let meta = base.join(".sbxm").join("runs").join(&summary.run_id);
    assert!(meta.join("kits").is_dir());
    for i in 0..2 {
        let workspace = base
            .join("runs")
            .join(&summary.run_id)
            .join(i.to_string())
            .join("0");
        assert!(workspace.is_dir(), "{}", workspace.display());
    }
}

#[test]
fn repeats_are_reported_per_repeat() {
    let env = Env::new();

    let ran = go(
        &env,
        &format!("{TASK}[run]\nrepeat = 2\n\n{TWO_CLAUDES}"),
        &pong(),
    );

    ran.result.unwrap();
    assert!(
        ran.out
            .contains("contestants[0] claude/one repeat 1/2: completed"),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("contestants[1] claude/two repeat 2/2: completed"),
        "{}",
        ran.out
    );
}

fn scripted(stdout: &str, stderr: &str, exit_code: i32) -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(exit_code),
    })
}

#[test]
fn a_failed_command_is_reported_with_its_reason_and_the_run_still_succeeds() {
    let env = Env::new();
    let backend = scripted("", "boom\n", 1);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    ran.result.unwrap();
    assert!(
        ran.out.contains("contestants[0] claude/one: failed:"),
        "{}",
        ran.out
    );
    assert!(ran.out.contains("boom"), "{}", ran.out);
    assert_eq!(backend.removes().len(), 2);
}

#[test]
fn a_timeout_is_labelled_and_keeps_the_partial_output_note() {
    let env = Env::new();
    let backend = scripted(
        CLAUDE_PONG,
        "timeout: sending signal TERM to command 'claude'\n",
        124,
    );

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    ran.result.unwrap();
    assert!(
        ran.out
            .contains("contestants[0] claude/one: timed out (partial output kept)"),
        "{}",
        ran.out
    );
}

#[test]
fn a_backend_error_is_reported_per_pair_and_cleaned_up() {
    let env = Env::new();
    let backend = FakeBackend::failing_create().and_secrets(&["anthropic"]);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    assert!(
        ran.out
            .contains("contestants[0] claude/one: error: cannot create sandbox"),
        "{}",
        ran.out
    );
    // The half-made sandboxes are removed anyway.
    assert_eq!(backend.removes().len(), 2);
    // Every pair errored, so the run itself fails (decision 181).
    let err = ran.result.unwrap_err();
    assert_eq!(err.to_string(), "2 of 2 contestant runs ended in error");
    let errored = err.downcast::<run::PairsErrored>().unwrap();
    assert_eq!(errored.summary.outcomes.len(), 2);
}

#[test]
fn one_errored_pair_is_counted_against_the_total_the_other_pair_still_completing() {
    let env = Env::new();
    // Contestant "two"'s headless command has `--model two` in its argv, so only its exec fails.
    let backend = pong().with_failing_exec_matching("two");

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    assert!(
        ran.out.contains("contestants[0] claude/one: completed"),
        "{}",
        ran.out
    );
    assert!(
        ran.out.contains("contestants[1] claude/two: error:"),
        "{}",
        ran.out
    );
    // Everything still got saved and printed (decision 16): the run isn't aborted by the error.
    let last = ran.out.lines().last().unwrap();
    assert!(last.starts_with("Results: "), "{}", ran.out);
    // Only then does the run fail, which is what gives the process its exit status (decision 181).
    let err = ran.result.unwrap_err();
    assert_eq!(err.to_string(), "1 of 2 contestant runs ended in error");
    let errored = err.downcast::<run::PairsErrored>().unwrap();
    assert_eq!(errored.summary.errored_pairs(), 1);
    assert_eq!(errored.summary.outcomes.len(), 2);
}

#[test]
fn a_timed_out_or_harness_failed_pair_alone_reports_no_errors() {
    let env = Env::new();

    let timed_out = go(
        &env,
        &format!("{TASK}{TWO_CLAUDES}"),
        &scripted(
            CLAUDE_PONG,
            "timeout: sending signal TERM to command 'claude'\n",
            124,
        ),
    );
    assert_eq!(timed_out.result.unwrap().errored_pairs(), 0);
    assert!(
        !timed_out.warn.contains("ended in error"),
        "{}",
        timed_out.warn
    );

    let env = Env::new();
    let failed = go(
        &env,
        &format!("{TASK}{TWO_CLAUDES}"),
        &scripted("", "boom\n", 1),
    );
    assert_eq!(failed.result.unwrap().errored_pairs(), 0);
    assert!(!failed.warn.contains("ended in error"), "{}", failed.warn);
}

#[test]
fn a_sandbox_that_cannot_be_removed_is_a_warning_with_the_fix() {
    let env = Env::new();
    let backend = FakeBackend::failing_remove()
        .and_secrets(&["anthropic"])
        .with_default_exec_output(ExecOutput {
            stdout: CLAUDE_PONG.into(),
            stderr: String::new(),
            exit_code: Some(0),
        });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    ran.result.unwrap();
    assert!(
        ran.warn
            .contains("warning: cannot remove sandbox sbxm-run-"),
        "{}",
        ran.warn
    );
    assert!(ran.warn.contains("sbx rm -f"), "{}", ran.warn);
}

#[test]
fn a_missing_secret_stops_everything_before_anything_is_written() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    });
    let body = format!(
        "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"b\"\n"
    );

    let ran = go(&env, &body, &backend);

    assert!(
        ran.result
            .unwrap_err()
            .to_string()
            .contains("secret 'openai'")
    );
    nothing_written(&env);
    assert!(backend.log().is_empty());
    assert_eq!(ran.out, "");
}

#[test]
fn cosine_with_an_incomplete_model_dir_is_refused_before_any_write_or_backend_call() {
    let env = Env::new();
    let backend = pong();
    let model_dir = env.tmp.path().join("models");
    std::fs::create_dir_all(&model_dir).unwrap();
    // Every file but model.onnx, the first one checked.
    for name in [
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ] {
        std::fs::write(model_dir.join(name), "{}").unwrap();
    }
    let body = format!(
        "{TASK}{TWO_CLAUDES}[eval.cosine]\nmodel_dir = {}\n",
        toml::Value::String(model_dir.to_str().unwrap().to_owned())
    );

    let ran = go(&env, &body, &backend);

    let err = ran.result.unwrap_err().to_string();
    assert!(err.contains("model.onnx"), "{err}");
    assert!(err.contains(model_dir.to_str().unwrap()), "{err}");
    assert!(
        err.contains("https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/"),
        "{err}"
    );
    nothing_written(&env);
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
    assert_eq!(ran.out, "");
}

#[test]
fn an_invalid_kit_creates_no_sandbox() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad").and_secrets(&["anthropic"]);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    assert!(
        ran.result
            .unwrap_err()
            .to_string()
            .contains("is invalid: bad")
    );
    assert!(backend.creates().is_empty() && backend.execs().is_empty());
}

#[test]
fn a_seeded_run_gives_every_contestant_its_own_repo_with_the_seed() {
    let env = Env::new();
    let seed = env.seed();
    let body = format!(
        "[task]\nprompt = \"p\"\nseed = {}\n\n{TWO_CLAUDES}",
        toml::Value::String(seed.to_str().unwrap().to_owned())
    );

    let ran = go(&env, &body, &pong());

    let summary = ran.result.unwrap();
    assert_eq!(ran.warn, "");
    for i in 0..2 {
        let ws = env
            .base_dir()
            .join("runs")
            .join(&summary.run_id)
            .join(i.to_string())
            .join("0");
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "a");
        assert!(ws.join(".git").is_dir());
        // The diff was captured, empty because the fake agent changed nothing.
        assert_eq!(
            summary.outcomes[i]
                .diff
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .patch,
            ""
        );
    }
}

#[test]
fn a_seed_that_fails_preflight_writes_nothing() {
    let env = Env::new();
    let body = format!(
        "[task]\nprompt = \"p\"\nseed = {}\n\n{TWO_CLAUDES}",
        toml::Value::String(
            env.tmp
                .path()
                .join("no-such-seed")
                .to_str()
                .unwrap()
                .to_owned()
        )
    );

    let ran = go(&env, &body, &pong());

    assert!(
        ran.result
            .unwrap_err()
            .to_string()
            .contains("is not a directory")
    );
    nothing_written(&env);
}

#[test]
fn preflight_warnings_reach_the_warning_stream() {
    let env = Env::new();

    let ran = go(
        &env,
        &format!("{TASK}[run]\nbudget_usd = 1.0\n\n{TWO_CLAUDES}"),
        &pong(),
    );

    ran.result.unwrap();
    assert_eq!(ran.warn, "");
    let env = Env::new();
    let body = format!(
        "{TASK}[run]\nbudget_usd = 1.0\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
         [[contestants]]\nharness = \"codex\"\nmodel = \"b\"\n"
    );
    let ran = go(&env, &body, &pong());
    ran.result.unwrap();
    assert!(
        ran.warn
            .starts_with("warning: run.budget_usd is set, but contestants[1] (codex)"),
        "{}",
        ran.warn
    );
}

// ---- the CLI ---------------------------------------------------------------

fn cli(dir: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir)
        .env("SBXM_CONFIG_DIR", dir.join("no-such-config"))
        .args(args)
        .assert()
}

#[test]
fn run_without_a_config_asks_for_one() {
    let tmp = tempfile::TempDir::new().unwrap();

    let output = cli(tmp.path(), &["run"]).failure().get_output().clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.to_lowercase().contains("config"), "{stderr}");
}

#[test]
fn run_with_a_missing_file_names_it_and_points_at_run_init() {
    let tmp = tempfile::TempDir::new().unwrap();

    let output = cli(tmp.path(), &["run", "nope.toml"])
        .failure()
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nope.toml") && stderr.contains("sbxm run init"),
        "{stderr}"
    );
}

#[test]
fn run_init_still_works_next_to_run_config() {
    let tmp = tempfile::TempDir::new().unwrap();

    cli(tmp.path(), &["run", "init"]).success();

    assert!(PathBuf::from(tmp.path()).join("run.toml").is_file());
}

// Issue 210: a low sandbox drive refuses before the run is reserved or any sandbox exists.

#[test]
fn a_low_sandbox_drive_refuses_before_anything_is_written() {
    let env = Env::new();
    let backend = pong();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, format!("{TASK}{TWO_CLAUDES}")).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());

    let result = run::run(
        &env.config_dir(),
        &path,
        &common::low_disk(&env),
        &backend,
        &mut out,
        &mut warn,
    );

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.starts_with(common::LOW_DISK), "{err}");
    assert!(err.contains("pass --ignore-disk"), "{err}");
    assert!(backend.log().is_empty(), "{:?}", backend.log());
    assert_eq!(backend.secret_service_calls(), 0);
    nothing_written(&env);
}

#[test]
fn ignore_disk_runs_anyway() {
    let env = Env::new();
    let backend = pong();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, format!("{TASK}{TWO_CLAUDES}")).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let disk = sbxm::disk::Guard {
        ignore: true,
        ..common::low_disk(&env)
    };

    run::run(
        &env.config_dir(),
        &path,
        &disk,
        &backend,
        &mut out,
        &mut warn,
    )
    .unwrap();
}

#[test]
fn run_takes_ignore_disk() {
    let env = Env::new();
    let assert = Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["run", "--ignore-disk", "missing.toml"])
        .assert()
        .failure();
    // Past argument parsing: the run-config itself is what's missing.
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("missing.toml"), "{stderr}");
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
}

// ---- Antigravity's default model (issue 314) -----------------------------

const AGY_PONG: &str = include_str!("../src/fixtures/antigravity-stream-json-pong.jsonl");
const DEFAULT_AGY_MODEL: &str = "gemini-3.8-flash-medium";

/// A Claude contestant and an Antigravity one whose `model` is `agy_model` (omitted when `None`),
/// judged by Antigravity with the same `model`; the judge's reply has no verdict.
fn claude_and_agy(agy_model: Option<&str>) -> String {
    let model = agy_model.map_or(String::new(), |m| format!("model = \"{m}\"\n"));
    format!(
        "{TASK}[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
         [[contestants]]\nharness = \"antigravity\"\n{model}\n\
         [[eval.rubric]]\nid = \"a\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
         [eval.judge]\nharness = \"antigravity\"\n{model}"
    )
}

fn agy_backend() -> FakeBackend {
    pong()
        .and_secrets(&["anthropic", "google"])
        .with_exec_output_matching(
            "agy",
            ExecOutput {
                stdout: AGY_PONG.into(),
                stderr: String::new(),
                exit_code: Some(0),
            },
        )
}

/// The `--model` of every `agy` command the backend ran.
fn agy_models(backend: &FakeBackend) -> Vec<String> {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a == "agy"))
        .map(|(_, spec)| {
            let at = spec.argv.iter().position(|a| a == "--model").unwrap();
            spec.argv[at + 1].clone()
        })
        .collect()
}

fn read_json(path: PathBuf) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The model every record names for the Antigravity contestant (`run.json`, its `result.json`)
/// and for the judge (`judge.json`, the pair's `evals.json`).
fn saved_agy_models(env: &Env, run_id: &str) -> Vec<serde_json::Value> {
    let meta = env.base_dir().join(".sbxm").join("runs").join(run_id);
    let pair = meta.join("1").join("0");
    vec![
        read_json(meta.join("run.json"))["contestants"][1]["model"].clone(),
        read_json(pair.join("result.json"))["model"].clone(),
        read_json(meta.join("judge").join("0").join("judge.json"))["judge"]["model"].clone(),
        read_json(pair.join("evals.json"))["judge"]["judge"]["model"].clone(),
    ]
}

#[test]
fn an_antigravity_contestant_and_judge_without_a_model_run_and_save_the_default() {
    let env = Env::new();
    let backend = agy_backend();

    let ran = go(&env, &claude_and_agy(None), &backend);

    let summary = ran.result.unwrap();
    assert_eq!(agy_models(&backend), [DEFAULT_AGY_MODEL, DEFAULT_AGY_MODEL]);
    assert_eq!(
        saved_agy_models(&env, &summary.run_id),
        [DEFAULT_AGY_MODEL; 4]
    );
}

#[test]
fn an_explicit_antigravity_model_is_run_and_saved_instead_of_the_default() {
    let env = Env::new();
    let backend = agy_backend();

    let ran = go(&env, &claude_and_agy(Some("gemini-4-argon-high")), &backend);

    let summary = ran.result.unwrap();
    assert_eq!(
        agy_models(&backend),
        ["gemini-4-argon-high", "gemini-4-argon-high"]
    );
    assert_eq!(
        saved_agy_models(&env, &summary.run_id),
        ["gemini-4-argon-high"; 4]
    );
}
