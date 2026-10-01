//! M2a slice 10: executable checks (P6). Each `[[eval.checks]]` command runs
//! inside the contestant's own sandbox after the agent finishes, under the same
//! in-sandbox `timeout` (decision 114), and is scored pass/fail by exit code.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::Env;
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend, Stdin};
use sbxm::commands::{run, run_show};
use sbxm::run::checks::{self, CheckOutcome};
use sbxm::run::config::{Check, RunConfig};
use serde_json::Value;

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const TERM: &str = "timeout: sending signal TERM to command 'sh'\n";
const KILL: &str =
    "timeout: sending signal TERM to command 'sh'\ntimeout: sending signal KILL to command 'sh'\n";

fn check(id: &str, command: &str, timeout: Option<u64>) -> Check {
    Check {
        id: id.into(),
        command: command.into(),
        timeout: timeout.map(Duration::from_secs),
    }
}

fn out(stdout: &str, stderr: &str, code: i32) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(code),
    }
}

fn run_checks(backend: &FakeBackend, list: &[Check]) -> Vec<CheckOutcome> {
    checks::run_checks(
        backend,
        "sb",
        Path::new("/work"),
        list,
        Duration::from_secs(600),
    )
}

// ---- running one check ------------------------------------------------------

#[test]
fn a_check_is_a_shell_command_under_the_in_sandbox_timeout() {
    let backend = FakeBackend::default();

    run_checks(&backend, &[check("tests", "cargo test --all", None)]);

    let calls = backend.execs();
    assert_eq!(calls.len(), 1);
    let (sandbox, spec) = &calls[0];
    assert_eq!(sandbox, "sb");
    assert_eq!(
        spec.argv,
        [
            "timeout",
            "-v",
            "--kill-after=10",
            "600",
            "sh",
            "-c",
            "cargo test --all"
        ]
    );
    assert_eq!(spec.workdir, Some(PathBuf::from("/work")));
    assert_eq!(spec.stdin, Stdin::Closed);
}

#[test]
fn a_checks_own_timeout_overrides_the_default() {
    let backend = FakeBackend::default();

    run_checks(
        &backend,
        &[
            check("quick", "true", Some(30)),
            check("dflt", "true", None),
        ],
    );

    let calls = backend.execs();
    assert_eq!(calls[0].1.argv[3], "30");
    assert_eq!(calls[1].1.argv[3], "600");
}

#[test]
fn exit_zero_passes_and_anything_else_fails_with_its_code() {
    let backend = FakeBackend::default()
        .with_exec_output_matching("ok-cmd", out("fine\n", "", 0))
        .with_exec_output_matching("bad-cmd", out("", "assertion failed\n", 1))
        .with_exec_output_matching("worse-cmd", out("", "", 2));

    let outcomes = run_checks(
        &backend,
        &[
            check("a", "ok-cmd", None),
            check("b", "bad-cmd", None),
            check("c", "worse-cmd", None),
        ],
    );

    assert_eq!(
        outcomes.iter().map(|o| o.passed).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(
        outcomes.iter().map(|o| o.exit_code).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2)]
    );
    assert!(outcomes.iter().all(|o| !o.timed_out && o.error.is_none()));
    assert_eq!(outcomes[1].id, "b");
    assert_eq!(outcomes[1].command, "bad-cmd");
    assert_eq!(outcomes[1].timeout_secs, 600);
    assert!(outcomes[1].output_tail.contains("assertion failed"));
}

#[test]
fn a_check_that_hits_its_timeout_fails_and_is_marked_timed_out() {
    let backend = FakeBackend::default()
        .with_exec_output_matching("term-cmd", out("partial\n", TERM, 124))
        .with_exec_output_matching("kill-cmd", out("", KILL, 137))
        .with_exec_output_matching("oom-cmd", out("", "Killed\n", 137));

    let outcomes = run_checks(
        &backend,
        &[
            check("t", "term-cmd", None),
            check("k", "kill-cmd", None),
            check("o", "oom-cmd", None),
        ],
    );

    assert!(!outcomes[0].passed && outcomes[0].timed_out);
    assert!(!outcomes[1].passed && outcomes[1].timed_out);
    // A bare 137 (an OOM kill) is a failure, not a timeout.
    assert!(!outcomes[2].passed && !outcomes[2].timed_out);
    assert_eq!(outcomes[2].exit_code, Some(137));
}

#[test]
fn a_backend_error_fails_that_check_and_the_rest_still_run() {
    let backend = FakeBackend::default().with_failing_exec_matching("boom-cmd");

    let outcomes = run_checks(
        &backend,
        &[check("a", "boom-cmd", None), check("b", "true", None)],
    );

    assert!(!outcomes[0].passed && outcomes[0].exit_code.is_none());
    assert!(
        outcomes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("fake exec failure")
    );
    assert!(outcomes[1].passed);
    assert_eq!(backend.execs().len(), 2);
}

#[test]
fn only_the_end_of_a_long_output_is_kept_and_it_includes_stderr() {
    let long = "x".repeat(50_000);
    let backend = FakeBackend::default().with_exec_output_matching(
        "noisy",
        out(&format!("{long}END-OF-STDOUT\n"), "the-error\n", 1),
    );

    let outcomes = run_checks(&backend, &[check("n", "noisy", None)]);

    let tail = &outcomes[0].output_tail;
    assert!(tail.len() <= 8 * 1024 + 4, "{}", tail.len());
    assert!(tail.contains("END-OF-STDOUT") && tail.contains("the-error"));
}

#[test]
fn no_checks_means_no_exec_calls() {
    let backend = FakeBackend::default();

    assert!(run_checks(&backend, &[]).is_empty());
    assert!(backend.execs().is_empty());
}

// ---- inside a run ------------------------------------------------------------

struct Ran {
    env: Env,
    summary: run::Summary,
    out: String,
    warn: String,
}

/// Two Claude contestants and the given `[[eval.checks]]` text.
fn go(checks_toml: &str, backend: &FakeBackend) -> Ran {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"p\"\n\n[run]\ntimeout = \"5m\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n{checks_toml}"
        ),
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

/// The host workspace of a run sandbox `sbxm-run-<id>-<contestant>-<repeat>`.
fn workspace_of(base: &Path, sandbox: &str) -> PathBuf {
    let rest = sandbox.strip_prefix("sbxm-run-").unwrap();
    let mut parts = rest.rsplitn(3, '-');
    let repeat = parts.next().unwrap();
    let contestant = parts.next().unwrap();
    let id = parts.next().unwrap();
    base.join("runs").join(id).join(contestant).join(repeat)
}

fn pong() -> FakeBackend {
    FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(out(CLAUDE_PONG, "", 0))
}

const TWO_CHECKS: &str = "[[eval.checks]]\nid = \"first\"\ncommand = \"check-one\"\n\n\
                          [[eval.checks]]\nid = \"second\"\ncommand = \"check-two\"\ntimeout = \"45s\"\n";

fn evals(ran: &Ran, contestant: usize, repeat: u32) -> Value {
    let path = ran
        .env
        .base_dir()
        .join(".sbxm")
        .join("runs")
        .join(&ran.summary.run_id)
        .join(contestant.to_string())
        .join(repeat.to_string())
        .join("evals.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn evals_path(ran: &Ran, contestant: usize, repeat: u32) -> PathBuf {
    ran.env
        .base_dir()
        .join(".sbxm")
        .join("runs")
        .join(&ran.summary.run_id)
        .join(contestant.to_string())
        .join(repeat.to_string())
        .join("evals.json")
}

#[test]
fn checks_run_in_the_pairs_own_sandbox_after_the_agent_and_before_removal() {
    let backend = pong();

    let ran = go(TWO_CHECKS, &backend);

    let id = &ran.summary.run_id;
    for i in 0..2 {
        let sandbox = format!("sbxm-run-{id}-{i}-0");
        let calls: Vec<String> = backend
            .log()
            .into_iter()
            .filter(|l| l.ends_with(&sandbox))
            .map(|l| l.split(' ').next().unwrap().to_owned())
            .collect();
        // agent, then both checks, then the sandbox goes.
        assert_eq!(calls, ["create", "exec", "exec", "exec", "rm"], "{sandbox}");
    }
    let execs = backend.execs();
    let commands: Vec<&str> = execs
        .iter()
        .filter(|(s, _)| s.ends_with("-0-0"))
        .map(|(_, spec)| spec.argv.last().unwrap().as_str())
        .collect();
    assert_eq!(commands[1..], ["check-one", "check-two"]);
}

#[test]
fn a_check_sees_the_agents_files_but_its_side_effects_stay_out_of_the_diff() {
    let env = Env::new();
    let base = env.base_dir();
    // The check writes a build artifact into the workspace, like `cargo test`'s target/.
    let backend = pong().with_exec_hook(move |sandbox: &str, spec: &ExecSpec| {
        if spec.argv.get(4).map(String::as_str) == Some("sh") {
            std::fs::write(workspace_of(&base, sandbox).join("artifact.o"), "built\n").unwrap();
        }
    });
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        "[task]\nprompt = \"p\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n\
         [[eval.checks]]\nid = \"build\"\ncommand = \"make\"\n",
    )
    .unwrap();

    let summary = run::run(
        &env.config_dir(),
        &path,
        &backend,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();

    let outcome = &summary.outcomes[0];
    // The artifact exists in the workspace afterwards, but the diff was captured before the check ran.
    assert!(outcome.workspace.join("artifact.o").is_file());
    let patch = &outcome.diff.as_ref().unwrap().as_ref().unwrap().patch;
    assert!(!patch.contains("artifact.o"), "{patch}");
}

#[test]
fn evals_json_records_each_check_next_to_the_pairs_results() {
    let backend = pong()
        .with_exec_output_matching("check-one", out("all good\n", "", 0))
        .with_exec_output_matching("check-two", out("", "2 tests failed\n", 1));

    let ran = go(TWO_CHECKS, &backend);

    let doc = evals(&ran, 0, 0);
    let list = doc["checks"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], "first");
    assert_eq!(list[0]["command"], "check-one");
    assert_eq!(list[0]["passed"], true);
    assert_eq!(list[0]["exit_code"], 0);
    assert_eq!(list[0]["timed_out"], false);
    assert_eq!(list[0]["timeout_secs"], 300);
    assert!(list[0]["error"].is_null());
    assert_eq!(list[1]["id"], "second");
    assert_eq!(list[1]["passed"], false);
    assert_eq!(list[1]["exit_code"], 1);
    assert_eq!(list[1]["timeout_secs"], 45);
    assert!(
        list[1]["output_tail"]
            .as_str()
            .unwrap()
            .contains("2 tests failed")
    );
    // The other contestant has its own file.
    assert!(evals_path(&ran, 1, 0).is_file());
}

#[test]
fn without_checks_there_is_no_evals_json() {
    let ran = go("", &pong());

    assert!(!evals_path(&ran, 0, 0).exists());
    assert!(!ran.out.contains("checks"), "{}", ran.out);
}

#[test]
fn timed_out_and_failed_pairs_still_run_their_checks() {
    // Every agent exec times out; the checks (sh -c) are matched first and pass.
    let backend = FakeBackend::with_secrets(&["anthropic"])
        .with_exec_output_matching("check-one", out("", "", 0))
        .with_exec_output_matching("check-two", out("", "", 0))
        .with_default_exec_output(out(
            CLAUDE_PONG,
            "timeout: sending signal TERM to command 'claude'\n",
            124,
        ));

    let ran = go(TWO_CHECKS, &backend);

    for i in 0..2 {
        let doc = evals(&ran, i, 0);
        assert_eq!(doc["checks"].as_array().unwrap().len(), 2);
        assert_eq!(doc["checks"][0]["passed"], true);
    }
    // The pairs really did time out.
    assert!(ran.out.contains("timed out"), "{}", ran.out);

    let failing = FakeBackend::with_secrets(&["anthropic"])
        .with_exec_output_matching("check-one", out("", "", 0))
        .with_exec_output_matching("check-two", out("", "", 0))
        .with_default_exec_output(out("", "boom\n", 1));
    let ran = go(TWO_CHECKS, &failing);
    assert!(ran.out.contains("failed:"), "{}", ran.out);
    assert_eq!(evals(&ran, 0, 0)["checks"].as_array().unwrap().len(), 2);
}

#[test]
fn a_hanging_check_is_recorded_and_the_run_still_saves_and_cleans_up() {
    let backend = pong()
        .with_exec_output_matching("check-one", out("", TERM, 124))
        .with_exec_output_matching("check-two", out("", "", 0));

    let ran = go(TWO_CHECKS, &backend);

    let doc = evals(&ran, 0, 0);
    assert_eq!(doc["checks"][0]["passed"], false);
    assert_eq!(doc["checks"][0]["timed_out"], true);
    // The next check still ran, the pair's own results were written, the sandbox was removed.
    assert_eq!(doc["checks"][1]["passed"], true);
    let dir = evals_path(&ran, 0, 0).parent().unwrap().to_path_buf();
    assert_eq!(read(&dir.join("result.json"))["status"], "completed");
    assert!(dir.join("answer.md").is_file());
    assert_eq!(backend.removes().len(), 2);
    assert_eq!(ran.warn, "");
}

#[test]
fn a_pair_that_never_got_a_sandbox_runs_no_checks() {
    let backend = FakeBackend::failing_create().and_secrets(&["anthropic"]);

    let ran = go(TWO_CHECKS, &backend);

    assert!(backend.execs().is_empty());
    assert!(!evals_path(&ran, 0, 0).exists());
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_run_summary_counts_passed_checks_per_pair() {
    let backend = pong()
        .with_exec_output_matching("check-one", out("", "", 0))
        .with_exec_output_matching("check-two", out("", "", 3));

    let ran = go(TWO_CHECKS, &backend);

    assert!(
        ran.out.contains("contestants[0] claude/one: completed"),
        "{}",
        ran.out
    );
    assert!(ran.out.contains("; checks 1/2 passed"), "{}", ran.out);
}

#[test]
fn run_show_lists_each_check_with_its_verdict() {
    let backend = pong()
        .with_exec_output_matching("check-one", out("", "", 0))
        .with_exec_output_matching("check-two", out("", "", 1));
    let ran = go(TWO_CHECKS, &backend);

    let shown = run_show::render(
        &ran.env.config_dir(),
        &ran.summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();

    assert!(
        shown.contains(
            "    Checks: 1/2 passed\n      passed first\n      failed second (exit code 1)\n"
        ),
        "{shown}"
    );
}

#[test]
fn run_show_marks_a_timed_out_check() {
    let backend = pong().with_exec_output_matching("check-two", out("", TERM, 124));
    let ran = go(TWO_CHECKS, &backend);

    let shown = run_show::render(
        &ran.env.config_dir(),
        &ran.summary.run_id,
        &run_show::Options { full_diff: false },
    )
    .unwrap();

    assert!(
        shown.contains("failed second (timed out after 45s)"),
        "{shown}"
    );
}

#[test]
fn the_configs_check_list_reaches_the_runner_in_order() {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"p\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"b\"\n\n{TWO_CHECKS}"
        ),
    )
    .unwrap();

    let config = RunConfig::load(&path).unwrap();

    assert_eq!(
        config
            .eval
            .checks
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}
