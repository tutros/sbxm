//! Issue 285 (decision 241): `[eval.jev]` in the run-config. Exactly one token source, an
//! absolute helper path, a rubric to score, no unknown keys, and `model` defaulting to
//! `jev-latest`.

mod common;

use std::path::PathBuf;

use common::Env;
use sbxm::backend::SecretSource;
use sbxm::run::config::RunConfig;

const BASE: &str = "[task]\nprompt = \"Do it\"\n\n\
                    [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
                    [[contestants]]\nharness = \"codex\"\nmodel = \"b\"\n\n";
const RUBRIC: &str = "[[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n";

fn helper() -> String {
    if cfg!(windows) {
        "E:/tools/jev-token.cmd".into()
    } else {
        "/opt/tools/jev-token".into()
    }
}

fn write(env: &Env, body: &str) -> PathBuf {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn load_ok(body: &str) -> RunConfig {
    let env = Env::new();
    RunConfig::load(&write(&env, body)).unwrap()
}

/// The error, which must be one line naming the file.
fn load_err(body: &str) -> String {
    let env = Env::new();
    let err = RunConfig::load(&write(&env, body)).unwrap_err();
    let text = format!("{err:#}");
    assert!(!text.contains('\n'), "not one line: {text}");
    assert!(text.contains("run.toml"), "doesn't name the file: {text}");
    text
}

#[test]
fn loads_with_a_token_command_and_defaults_the_model() {
    let config = load_ok(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_command = \"{}\"\n",
        helper()
    ));
    let jev = config.eval.jev.unwrap();
    assert_eq!(jev.token, SecretSource::Command(PathBuf::from(helper())));
    assert_eq!(jev.model, "jev-latest");
}

#[test]
fn loads_with_a_token_ref_and_an_explicit_model() {
    let config = load_ok(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_ref = \"op://vault/jev/token\"\nmodel = \"jev-1.13.0\"\n"
    ));
    let jev = config.eval.jev.unwrap();
    assert_eq!(jev.token, SecretSource::Ref("op://vault/jev/token".into()));
    assert_eq!(jev.model, "jev-1.13.0");
}

#[test]
fn an_arn_is_a_token_ref_too() {
    let config = load_ok(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_ref = \"arn:aws:secretsmanager:eu-west-1:1:secret:jev\"\n"
    ));
    assert!(matches!(
        config.eval.jev.unwrap().token,
        SecretSource::Ref(_)
    ));
}

#[test]
fn both_sources_are_refused() {
    let err = load_err(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_command = \"{}\"\ntoken_ref = \"op://v/j/t\"\n",
        helper()
    ));
    assert!(
        err.contains("token_command") && err.contains("token_ref") && err.contains("one"),
        "{err}"
    );
}

#[test]
fn neither_source_is_refused() {
    let err = load_err(&format!("{BASE}{RUBRIC}[eval.jev]\n"));
    assert!(
        err.contains("eval.jev") && err.contains("token_command") && err.contains("token_ref"),
        "{err}"
    );
}

#[test]
fn a_relative_token_command_is_refused() {
    let err = load_err(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_command = \"bin/jev-token\"\n"
    ));
    assert!(
        err.contains("eval.jev.token_command") && err.contains("absolute"),
        "{err}"
    );
}

#[test]
fn a_token_ref_that_is_no_reference_is_refused() {
    let err = load_err(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_ref = \"sk-live-123\"\n"
    ));
    assert!(
        err.contains("eval.jev.token_ref") && err.contains("op://"),
        "{err}"
    );
    // A value put there by mistake is never repeated in the message (decision 36).
    assert!(!err.contains("sk-live-123"), "{err}");
}

#[test]
fn an_unknown_key_is_refused() {
    let err = load_err(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_ref = \"op://v/j/t\"\ntoken = \"x\"\n"
    ));
    assert!(err.contains("token"), "{err}");
}

#[test]
fn an_empty_rubric_is_refused() {
    let err = load_err(&format!("{BASE}[eval.jev]\ntoken_ref = \"op://v/j/t\"\n"));
    assert!(
        err.contains("eval.jev") && err.contains("[[eval.rubric]]"),
        "{err}"
    );
}

#[test]
fn an_empty_model_is_refused() {
    let err = load_err(&format!(
        "{BASE}{RUBRIC}[eval.jev]\ntoken_ref = \"op://v/j/t\"\nmodel = \" \"\n"
    ));
    assert!(err.contains("eval.jev.model"), "{err}");
}

#[test]
fn without_eval_jev_there_is_none() {
    assert!(load_ok(&format!("{BASE}{RUBRIC}")).eval.jev.is_none());
}
