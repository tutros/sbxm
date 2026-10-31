//! Decision 170, part 3 (issue 89): `sbxm run` copies what it prints to `run.log` in the run
//! folder, `<base>/.sbxm/runs/<id>/`, with part 1's header and line format, and no secret values.

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use regex::Regex;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::run;
use sbxm::task::runlog::LineSink;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TASK: &str = "[task]\nprompt = \"Reply with exactly: PONG\"\n\n";
const TWO_CLAUDES: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n";
const CLAUDE_AND_CODEX: &str = "[run]\nbudget_usd = 1.0\n\n\
                                [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
                                [[contestants]]\nharness = \"codex\"\nmodel = \"b\"\n";

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

/// The one run folder under `<base>/.sbxm/runs/`.
fn run_folder(env: &Env) -> PathBuf {
    let runs = env.base_dir().join(".sbxm").join("runs");
    let mut all: Vec<PathBuf> = std::fs::read_dir(&runs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

fn log_of(env: &Env) -> String {
    std::fs::read_to_string(run_folder(env).join("run.log")).unwrap()
}

/// `(stage, text)` of every stamped line, in order; the header lines are left out.
fn stamped(log: &str) -> Vec<(String, String)> {
    let line = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z \[([^\]]+)\] (.*)$").unwrap();
    log.lines()
        .filter(|l| !l.starts_with("# "))
        .map(|l| {
            let caps = line
                .captures(l)
                .unwrap_or_else(|| panic!("not a stamped line: {l:?}"));
            (caps[1].to_owned(), caps[2].to_owned())
        })
        .collect()
}

fn texts(log: &str) -> Vec<String> {
    stamped(log).into_iter().map(|(_, text)| text).collect()
}

#[test]
fn a_run_log_starts_with_the_header_then_has_every_line_the_screen_had() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    let summary = ran.result.unwrap();
    let log = log_of(&env);
    let first = log.lines().next().unwrap();
    assert!(first.starts_with("# "), "{first}");
    assert!(first.contains(" | exe "), "{first}");
    assert!(
        Regex::new(r" \| sha256 [0-9a-f]{64} \| commit \S+ \| pid \d+$")
            .unwrap()
            .is_match(first),
        "{first}"
    );
    assert!(
        first.ends_with(&format!("| pid {}", std::process::id())),
        "{first}"
    );
    assert_eq!(log.lines().filter(|l| l.starts_with("# ")).count(), 1);
    let screen: Vec<String> = ran.out.lines().map(str::to_owned).collect();
    assert_eq!(texts(&log), screen);
    assert_eq!(screen[0], format!("Run {}", summary.run_id));
}

#[test]
fn warnings_printed_before_the_run_folder_exists_are_in_the_log_too() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{CLAUDE_AND_CODEX}"), &pong());

    ran.result.unwrap();
    let warning = ran.warn.lines().next().unwrap();
    assert!(
        warning.starts_with("warning: run.budget_usd is set"),
        "{warning}"
    );
    let lines = stamped(&log_of(&env));
    assert_eq!(lines[0], ("preflight".to_owned(), warning.to_owned()));
    let all: Vec<String> = ran
        .warn
        .lines()
        .chain(ran.out.lines())
        .map(str::to_owned)
        .collect();
    let mut logged = texts(&log_of(&env));
    let mut screen = all;
    logged.sort();
    screen.sort();
    assert_eq!(logged, screen);
}

#[test]
fn each_line_carries_the_stage_of_the_run_it_was_printed_in() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    ran.result.unwrap();
    let lines = stamped(&log_of(&env));
    let stage_of = |prefix: &str| {
        lines
            .iter()
            .find(|(_, text)| text.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line {prefix}: {lines:?}"))
            .0
            .clone()
    };
    assert_eq!(stage_of("Run "), "pairs");
    assert_eq!(stage_of("contestants[0]"), "pairs");
    assert_eq!(stage_of("Results: "), "results");
}

#[test]
fn the_screen_output_is_unchanged_by_the_log() {
    let env = Env::new();

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong());

    let summary = ran.result.unwrap();
    let meta = run_folder(&env);
    assert_eq!(
        ran.out,
        format!(
            "Run {}\n{}\n{}\nResults: {}\n",
            summary.run_id,
            ran.out.lines().nth(1).unwrap(),
            ran.out.lines().nth(2).unwrap(),
            meta.display()
        )
    );
    assert!(
        ran.out
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("contestants[0] claude (one): completed")
    );
    assert!(!ran.out.contains("# ") && !ran.out.contains("[pairs]"));
    assert_eq!(ran.warn, "");
}

#[test]
fn secret_values_on_the_screen_never_reach_the_log() {
    let env = Env::new();
    let backend =
        FakeBackend::with_secrets(&["anthropic", "openai"]).with_default_exec_output(ExecOutput {
            stdout: String::new(),
            stderr: "token sk-ant-FAKE-NOT-A-REAL-KEY rejected".into(),
            exit_code: Some(1),
        });

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    ran.result.unwrap();
    assert!(
        ran.out.contains("sk-ant-FAKE-NOT-A-REAL-KEY"),
        "{}",
        ran.out
    );
    let log = log_of(&env);
    assert!(!log.contains("sk-ant-FAKE-NOT-A-REAL-KEY"), "{log}");
    assert!(log.contains("token [redacted] rejected"), "{log}");
}

#[test]
fn the_run_folder_gains_only_run_log() {
    let env = Env::new();

    go(&env, &format!("{TASK}{TWO_CLAUDES}"), &pong())
        .result
        .unwrap();

    let mut names: Vec<String> = std::fs::read_dir(run_folder(&env))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["0", "1", "kits", "run-config.toml", "run.json", "run.log"]
    );
}

#[test]
fn a_run_that_fails_after_its_folder_exists_logs_the_error() {
    let env = Env::new();
    let backend = FakeBackend::with_invalid_kit("bad").and_secrets(&["anthropic"]);

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let err = ran.result.unwrap_err();
    let log = log_of(&env);
    let expected: Vec<String> = format!("Error: {err:?}")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(texts(&log), expected);
}

#[test]
fn a_run_with_pairs_in_error_logs_the_error_after_its_output() {
    let env = Env::new();
    let backend = pong().with_failing_create_matching("-1-0");

    let ran = go(&env, &format!("{TASK}{TWO_CLAUDES}"), &backend);

    let err = ran.result.unwrap_err();
    let logged = texts(&log_of(&env));
    assert_eq!(
        logged.last().unwrap(),
        &format!("Error: {err:?}").lines().last().unwrap().to_owned()
    );
    assert!(logged.iter().any(|l| l.starts_with("Results: ")));
}

#[test]
fn a_run_refused_before_its_folder_exists_writes_no_log() {
    let env = Env::new();

    let ran = go(&env, "[task]\n", &pong());

    assert!(ran.result.is_err());
    assert!(!env.base_dir().join(".sbxm").exists());
}

/// A log that can't be appended to warns once, naming the file, not once per line.
#[test]
fn an_append_failure_warns_once_naming_the_path() {
    use std::sync::{Arc, Mutex};

    let dir = tempfile::TempDir::new().unwrap();
    let gone: &Path = &dir.path().join("gone");
    let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&warnings);
    let mut log = sbxm::run::log::RunFolderLog::new(
        "# header".to_owned(),
        Box::new(|| 0),
        Box::new(move |msg| seen.lock().unwrap().push(msg.to_owned())),
    );

    log.attach(gone);
    log.line("one");
    log.line("two");

    let warnings = warnings.lock().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&gone.join("run.log").display().to_string()),
        "{warnings:?}"
    );
}

/// A `RunFolderLog` with header `# header`, the clock at 0, and the warnings it gave.
fn folder_log() -> (
    sbxm::run::log::RunFolderLog,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use std::sync::{Arc, Mutex};

    let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&warnings);
    let log = sbxm::run::log::RunFolderLog::new(
        "# header".to_owned(),
        Box::new(|| 0),
        Box::new(move |msg| seen.lock().unwrap().push(msg.to_owned())),
    );
    (log, warnings)
}

/// Issue 353: the header and the preflight lines wait for the first append that succeeds.
#[test]
fn a_recovered_append_still_gets_the_header_and_the_preflight_lines() {
    let dir = tempfile::TempDir::new().unwrap();
    // `run.log` is a directory, so the first append fails deterministically.
    std::fs::create_dir(dir.path().join("run.log")).unwrap();
    let (mut log, warnings) = folder_log();

    log.line("preflight warning");
    log.attach(dir.path());
    std::fs::remove_dir(dir.path().join("run.log")).unwrap();
    log.line("Run example");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("run.log")).unwrap(),
        "# header\n\
         1970-01-01T00:00:00Z [preflight] preflight warning\n\
         1970-01-01T00:00:00Z [preflight] Run example\n"
    );
    assert_eq!(warnings.lock().unwrap().len(), 1);
}

#[test]
fn two_failed_appends_then_one_that_succeeds_write_the_header_once_and_warn_once() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("run.log")).unwrap();
    let (mut log, warnings) = folder_log();

    log.line("preflight warning");
    log.attach(dir.path());
    log.line("lost");
    std::fs::remove_dir(dir.path().join("run.log")).unwrap();
    log.line("Run example");
    log.line("after");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("run.log")).unwrap(),
        "# header\n\
         1970-01-01T00:00:00Z [preflight] preflight warning\n\
         1970-01-01T00:00:00Z [preflight] Run example\n\
         1970-01-01T00:00:00Z [preflight] after\n"
    );
    let warnings = warnings.lock().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&dir.path().join("run.log").display().to_string()),
        "{warnings:?}"
    );
}

#[test]
fn without_a_failure_the_header_is_written_once_at_attach() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut log, warnings) = folder_log();

    log.line("preflight warning");
    log.attach(dir.path());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("run.log")).unwrap(),
        "# header\n1970-01-01T00:00:00Z [preflight] preflight warning\n"
    );
    log.line("Run example");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("run.log")).unwrap(),
        "# header\n\
         1970-01-01T00:00:00Z [preflight] preflight warning\n\
         1970-01-01T00:00:00Z [preflight] Run example\n"
    );
    assert!(warnings.lock().unwrap().is_empty());
}

#[test]
fn nothing_buffered_then_a_failed_attach_recovers_with_the_header_and_the_new_line() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("run.log")).unwrap();
    let (mut log, warnings) = folder_log();

    log.attach(dir.path());
    std::fs::remove_dir(dir.path().join("run.log")).unwrap();
    log.line("Run example");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("run.log")).unwrap(),
        "# header\n1970-01-01T00:00:00Z [preflight] Run example\n"
    );
    assert_eq!(warnings.lock().unwrap().len(), 1);
}
