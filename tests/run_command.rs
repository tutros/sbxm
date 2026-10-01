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
    let result = run::run(&env.config_dir(), &path, backend, &mut out, &mut warn);
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

    ran.result.unwrap();
    assert!(
        ran.out
            .contains("contestants[0] claude/one: error: cannot create sandbox"),
        "{}",
        ran.out
    );
    // The half-made sandboxes are removed anyway.
    assert_eq!(backend.removes().len(), 2);
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
fn cosine_is_refused_before_any_write_or_backend_call() {
    let env = Env::new();
    let backend = pong();

    let ran = go(
        &env,
        &format!("{TASK}{TWO_CLAUDES}[eval.cosine]\n"),
        &backend,
    );

    assert!(
        ran.result
            .unwrap_err()
            .to_string()
            .contains("[eval.cosine] isn't implemented yet")
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
