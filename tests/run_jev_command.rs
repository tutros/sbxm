//! Issue 285 (decision 241), end to end with the fake backend: the Jev evaluator's scoped
//! secret, its sandbox, one call per contestant per repeat (plus chunks), retries, errors that
//! never fail the run, and what lands in `evals.json` and `run.json`. A fake `curl` answers in
//! the evaluator's workspace, like the real one would.

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Env;
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend, SecretSource};
use sbxm::commands::run;
use sbxm::eval::jev;
use sbxm::run::config::{Criterion, CriterionKind};
use serde_json::{Value, json};

const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const CONTESTANTS: &str = "[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
                           [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n\n";
const RUBRIC: &str = "[[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                      [[eval.rubric]]\nid = \"quality\"\nkind = \"scale\"\nlevels = [\"poor\", \"good\"]\nweight = 0.5\n\n";
const AUTH_BODY: &str = r#"{"detail":{"error_type":"authentication_error","message":"Cannot authenticate with the server. Please check your API key and try again."}}"#;

fn out(stdout: &str, stderr: &str, code: i32) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(code),
    }
}

/// A Claude stream whose final answer is `text`.
fn claude_answer(text: &str) -> String {
    let result = json!({
        "type": "result", "subtype": "success", "is_error": false, "result": text,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    format!("{result}\n")
}

/// A 200 body answering both criteria, from `model`.
fn answer(model: &str, noul: f64) -> String {
    json!({
        "model": model,
        "answers": {
            "correct": {"type": "noul", "noul": noul},
            "quality": {"type": "score", "score": 0.8, "confidence": 0.7, "legend": {}, "probabilities": {}},
        },
        "usage": {"input_tokens": 400, "output_tokens": 30},
    })
    .to_string()
}

/// `base/runs/<id>/jev/<repeat>/call` for the evaluator sandbox `sbxm-run-<id>-jev-<repeat>`.
fn call_dir(base: &Path, sandbox: &str) -> PathBuf {
    let rest = sandbox.strip_prefix("sbxm-run-").unwrap();
    let (id, repeat) = rest.rsplit_once("-jev-").unwrap();
    base.join("runs")
        .join(id)
        .join("jev")
        .join(repeat)
        .join("call")
}

/// `base/runs/<id>/<contestant>/<repeat>` for a contestant's sandbox.
fn workspace_of(base: &Path, sandbox: &str) -> PathBuf {
    let rest = sandbox.strip_prefix("sbxm-run-").unwrap();
    let mut parts = rest.rsplitn(3, '-');
    let repeat = parts.next().unwrap();
    let contestant = parts.next().unwrap();
    let id = parts.next().unwrap();
    base.join("runs").join(id).join(contestant).join(repeat)
}

fn is_jev(spec: &ExecSpec) -> bool {
    spec.argv.iter().any(|a| a.contains("api.typesafe.ai"))
}

/// The fake Jev: answers each call from `replies` in order, then with `default`, and keeps
/// every request it was sent.
#[derive(Clone)]
struct FakeJev {
    requests: Arc<Mutex<Vec<Value>>>,
    replies: Arc<Mutex<VecDeque<(u16, String)>>>,
    default: (u16, String),
}

impl FakeJev {
    fn new(replies: Vec<(u16, String)>) -> Self {
        FakeJev {
            requests: Arc::default(),
            replies: Arc::new(Mutex::new(replies.into())),
            default: (200, answer("jev-1.13.0", 0.9)),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// A backend whose contestants answer PONG and whose evaluator calls reach this fake Jev.
    fn backend(&self, env: &Env) -> FakeBackend {
        self.backend_answering(env, CLAUDE_PONG)
    }

    fn backend_answering(&self, env: &Env, contestant_output: &str) -> FakeBackend {
        let base = env.base_dir();
        let jev = self.clone();
        FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_version("0.46.0")
            .with_default_exec_output(out(contestant_output, "", 0))
            .with_exec_responder(move |sandbox, spec| {
                if !is_jev(spec) {
                    return None;
                }
                let dir = call_dir(&base, sandbox);
                let request = std::fs::read_to_string(dir.join("request.json")).unwrap();
                jev.requests
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&request).unwrap());
                let (code, body) = jev
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| jev.default.clone());
                std::fs::write(dir.join("response.json"), body).unwrap();
                Some(out(&code.to_string(), "", 0))
            })
    }
}

fn helper(env: &Env) -> PathBuf {
    let path = env.tmp.path().join("tools").join("jev-token");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "#!/bin/sh\necho CANARY-TOKEN-5f1e\n").unwrap();
    path
}

fn jev_table(env: &Env) -> String {
    let helper = toml::Value::String(helper(env).to_str().unwrap().to_owned());
    format!("[eval.jev]\ntoken_command = {helper}\n")
}

struct Ran {
    summary: run::Summary,
    out: String,
    warn: String,
    sleeps: Vec<Duration>,
}

fn go(env: &Env, body: &str, backend: &FakeBackend) -> Ran {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let sleeps = Mutex::new(Vec::new());
    let summary = run::run_with_sleep(
        &env.config_dir(),
        &path,
        &Default::default(),
        backend,
        &mut out,
        &mut warn,
        &|d| sleeps.lock().unwrap().push(d),
    )
    .unwrap();
    Ran {
        summary,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
        sleeps: sleeps.into_inner().unwrap(),
    }
}

fn config(env: &Env, extra: &str) -> String {
    format!(
        "[task]\nprompt = \"Implement feature X\"\n\n{extra}{CONTESTANTS}{RUBRIC}{}",
        jev_table(env)
    )
}

fn meta(env: &Env, ran: &Ran) -> PathBuf {
    env.base_dir()
        .join(".sbxm")
        .join("runs")
        .join(&ran.summary.run_id)
}

fn read(path: PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn evals(env: &Env, ran: &Ran, contestant: usize, repeat: u32) -> Value {
    read(
        meta(env, ran)
            .join(contestant.to_string())
            .join(repeat.to_string())
            .join("evals.json"),
    )
}

fn jev_execs(backend: &FakeBackend) -> usize {
    backend.execs().iter().filter(|(_, s)| is_jev(s)).count()
}

/// The index of `line` in `log`, which must hold it.
fn at(log: &[String], line: &str) -> usize {
    log.iter()
        .position(|l| l == line)
        .unwrap_or_else(|| panic!("{line:?} not in {log:#?}"))
}

/// Asserts the evaluator's secret was set before its sandbox and removed after it.
fn assert_secret_around_sandbox(backend: &FakeBackend, run_id: &str, repeat: u32) {
    let log = backend.log();
    let sandbox = format!("sbxm-run-{run_id}-jev-{repeat}");
    let placeholder = format!("sbxm-jev-{run_id}-{repeat}");
    let set = at(&log, &format!("secret-set {sandbox} {placeholder}"));
    let create = at(&log, &format!("create {sandbox}"));
    let rm = at(&log, &format!("rm {sandbox}"));
    let secret_rm = at(&log, &format!("secret-rm {sandbox} {placeholder}"));
    assert!(set < create && create < rm && rm < secret_rm, "{log:#?}");
    for (i, line) in log.iter().enumerate() {
        if line == &format!("exec {sandbox}") {
            assert!(create < i && i < rm, "{log:#?}");
        }
    }
    assert!(backend.current_custom_secrets().is_empty());
}

// ---- order and calls ----------------------------------------------------------------------

#[test]
fn the_secret_comes_before_the_sandbox_and_goes_after_it_one_call_per_pair_per_repeat() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, "[run]\nrepeat = 2\n\n"), &backend);

    let id = &ran.summary.run_id;
    for repeat in 0..2 {
        assert_secret_around_sandbox(&backend, id, repeat);
        let sandbox = format!("sbxm-run-{id}-jev-{repeat}");
        let calls = backend
            .execs()
            .iter()
            .filter(|(s, spec)| *s == sandbox && is_jev(spec))
            .count();
        assert_eq!(calls, 2, "one call per contestant in repeat {repeat}");
    }
    assert_eq!(jev_execs(&backend), 4);
    // After every contestant's sandbox is gone.
    let log = backend.log();
    let last_pair_rm = log
        .iter()
        .rposition(|l| l.starts_with("rm ") && !l.contains("-jev-"))
        .unwrap();
    assert!(
        last_pair_rm
            < at(
                &log,
                &format!("secret-set sbxm-run-{id}-jev-0 sbxm-jev-{id}-0")
            )
    );
    // The secret names the helper, the host and the env, never a value.
    let set = &backend.custom_secret_sets()[0];
    assert_eq!(set.host, "api.typesafe.ai");
    assert_eq!(set.env, "JEV_API_KEY");
    assert_eq!(set.source, SecretSource::Command(helper(&env)));
    // The evaluator is a plain shell with its own kit, never a contestant's.
    let create = backend
        .creates()
        .into_iter()
        .find(|c| c.name.contains("-jev-"))
        .unwrap();
    assert_eq!(create.agent, "shell");
    assert_eq!(create.kits.len(), 1);
    let spec = std::fs::read_to_string(create.kits[0].join("spec.yaml")).unwrap();
    assert!(spec.contains("api.typesafe.ai"), "{spec}");
    assert!(
        ran.out
            .contains("Jev jev-latest: repeat 1/2 scored 2 of 2 contestants"),
        "{}",
        ran.out
    );
}

#[test]
fn the_call_is_the_fixed_curl_with_the_header_in_double_quotes() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = jev.backend(&env);
    go(&env, &config(&env, ""), &backend);
    let (_, spec) = backend
        .execs()
        .into_iter()
        .find(|(_, s)| is_jev(s))
        .unwrap();
    let command = spec.argv.last().unwrap();
    assert!(
        command.contains("-H \"Authorization: Bearer $JEV_API_KEY\""),
        "{command}"
    );
    assert!(command.contains("--data @request.json -o response.json"));
    assert_eq!(spec.argv[0], "timeout");
    // Every request names the model; `jev-latest` by default.
    for request in jev.requests() {
        assert_eq!(request["model"], "jev-latest");
    }
}

// ---- results ------------------------------------------------------------------------------

#[test]
fn evals_json_gets_a_jev_key_next_to_checks_and_judge_and_run_json_the_models() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let judge_reply = {
        let verdict = json!({
            "A": {"correct": true, "quality": "good"},
            "B": {"correct": false, "quality": "poor"},
        })
        .to_string();
        let item = json!({"type": "item.completed", "item": {"id": "i", "type": "agent_message", "text": verdict}});
        format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}\n{item}\n{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":2}}}}\n"
        )
    };
    let backend = jev
        .backend(&env)
        .with_exec_output_matching("judge-prompt.md", out(&judge_reply, "", 0));
    let body = format!(
        "{}[[eval.checks]]\nid = \"builds\"\ncommand = \"true\"\n\n\
         [eval.judge]\nharness = \"codex\"\nmodel = \"judge-model\"\n",
        config(&env, "")
    );
    let ran = go(&env, &body, &backend);

    let pair = evals(&env, &ran, 0, 0);
    assert!(
        pair["checks"].is_array() && pair["judge"]["status"] == "ok",
        "{pair:#}"
    );
    assert_eq!(
        pair["jev"],
        json!({
            "status": "ok",
            "model": "jev-1.13.0",
            "chunks": 1,
            "criteria": {
                "correct": {"score": 0.9},
                "quality": {"score": 0.8, "confidence": 0.7},
            },
            "unscored": [],
            "usage": {"input_tokens": 400, "output_tokens": 30},
        })
    );
    let record = read(meta(&env, &ran).join("run.json"));
    assert!(record["completed_at"].is_string());
    assert_eq!(record["jev_models"], json!(["jev-1.13.0"]));
}

#[test]
fn a_run_without_jev_has_no_jev_models() {
    let env = Env::new();
    let backend =
        FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(out(CLAUDE_PONG, "", 0));
    let ran = go(
        &env,
        &format!("[task]\nprompt = \"p\"\n\n{CONTESTANTS}"),
        &backend,
    );
    let record = read(meta(&env, &ran).join("run.json"));
    assert!(record["completed_at"].is_string());
    assert!(record.get("jev_models").is_none(), "{record:#}");
    assert!(!backend.log().iter().any(|l| l.contains("secret-")));
}

#[test]
fn the_state_holds_the_answer_and_diff_never_the_transcript() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let stream = format!(
        "{{\"type\":\"system\",\"subtype\":\"init\",\"note\":\"TRANSCRIPT-SENTINEL\"}}\n{}",
        claude_answer("the final answer")
    );
    let base = env.base_dir();
    let backend = jev
        .backend_answering(&env, &stream)
        .with_exec_hook(move |sandbox, spec| {
            if !is_jev(spec) && !sandbox.contains("-jev-") {
                std::fs::write(
                    workspace_of(&base, sandbox).join("new.rs"),
                    "fn added() {}\n",
                )
                .unwrap();
            }
        });
    let ran = go(&env, &config(&env, ""), &backend);
    // The transcript was saved, so the sentinel was there to leak.
    let transcript = std::fs::read_to_string(
        meta(&env, &ran)
            .join("0")
            .join("0")
            .join("transcript.jsonl"),
    )
    .unwrap();
    assert!(transcript.contains("TRANSCRIPT-SENTINEL"));
    let requests = jev.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let text = request.to_string();
        assert!(!text.contains("TRANSCRIPT-SENTINEL"), "{text}");
        let state = request["state"].as_str().unwrap();
        assert!(
            state.contains("the final answer") && state.contains("fn added()"),
            "{state}"
        );
    }
}

// ---- failures still remove the secret -----------------------------------------------------

#[test]
fn a_failed_exec_is_the_pairs_error_and_the_secret_is_still_removed() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = jev
        .backend(&env)
        .with_failing_exec_matching("api.typesafe.ai");
    let ran = go(&env, &config(&env, ""), &backend);
    assert_secret_around_sandbox(&backend, &ran.summary.run_id, 0);
    let entry = &evals(&env, &ran, 0, 0)["jev"];
    assert_eq!(entry["status"], "error");
    assert!(entry["error"].as_str().unwrap().contains("exec"), "{entry}");
    assert!(
        ran.warn
            .contains("warning: Jev could not score contestants[0]"),
        "{}",
        ran.warn
    );
}

#[test]
fn a_401_is_never_retried_names_the_config_key_and_the_secret_is_still_removed() {
    let env = Env::new();
    let jev = FakeJev::new(vec![(401, AUTH_BODY.into()), (401, AUTH_BODY.into())]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert_secret_around_sandbox(&backend, &ran.summary.run_id, 0);
    assert_eq!(jev_execs(&backend), 2, "one call per pair, no retry");
    assert!(ran.sleeps.is_empty());
    for contestant in 0..2 {
        let error = evals(&env, &ran, contestant, 0)["jev"]["error"].clone();
        let error = error.as_str().unwrap();
        assert!(
            error.contains("401") && error.contains("eval.jev.token_command"),
            "{error}"
        );
    }
}

#[test]
fn a_timeout_is_the_pairs_error_and_the_secret_is_still_removed() {
    let env = Env::new();
    // No fake Jev: the in-sandbox `timeout` killed the call.
    let backend = FakeBackend::with_secrets(&["anthropic"])
        .with_version("0.46.0")
        .with_default_exec_output(out(CLAUDE_PONG, "", 0))
        .with_exec_output_matching(
            "api.typesafe.ai",
            out("", "timeout: sending signal TERM to command 'sh'\n", 124),
        );
    let ran = go(&env, &config(&env, ""), &backend);
    assert_secret_around_sandbox(&backend, &ran.summary.run_id, 0);
    let error = evals(&env, &ran, 1, 0)["jev"]["error"].clone();
    assert!(error.as_str().unwrap().contains("timed out"), "{error}");
}

#[test]
fn a_failed_sandbox_create_is_every_pairs_error_and_the_secret_is_still_removed() {
    let env = Env::new();
    let backend = FakeJev::new(vec![])
        .backend(&env)
        .with_failing_create_matching("-jev-");
    let ran = go(&env, &config(&env, ""), &backend);
    let id = &ran.summary.run_id;
    let log = backend.log();
    let sandbox = format!("sbxm-run-{id}-jev-0");
    let placeholder = format!("sbxm-jev-{id}-0");
    let create = at(&log, &format!("create {sandbox}"));
    assert!(at(&log, &format!("secret-set {sandbox} {placeholder}")) < create);
    assert!(create < at(&log, &format!("secret-rm {sandbox} {placeholder}")));
    assert_eq!(jev_execs(&backend), 0);
    assert!(backend.current_custom_secrets().is_empty());
    for contestant in 0..2 {
        let error = evals(&env, &ran, contestant, 0)["jev"]["error"].clone();
        assert!(
            error
                .as_str()
                .unwrap()
                .contains("cannot create the Jev sandbox"),
            "{error}"
        );
    }
}

#[test]
fn a_failed_secret_store_makes_no_sandbox_and_still_tries_the_removal() {
    let env = Env::new();
    let backend = FakeJev::new(vec![])
        .backend(&env)
        .failing_set_custom_secret();
    let ran = go(&env, &config(&env, ""), &backend);
    let log = backend.log();
    assert!(
        !log.iter()
            .any(|l| l.starts_with("create ") && l.contains("-jev-")),
        "{log:#?}"
    );
    assert!(log.iter().any(|l| l.starts_with("secret-rm ")), "{log:#?}");
    let error = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(error.as_str().unwrap().contains("scoped secret"), "{error}");
}

#[test]
fn a_secret_that_cannot_be_removed_is_a_warning_with_the_command_and_the_results_stay() {
    let env = Env::new();
    let backend = FakeJev::new(vec![])
        .backend(&env)
        .failing_remove_custom_secret();
    let ran = go(&env, &config(&env, ""), &backend);
    let id = &ran.summary.run_id;
    assert!(
        ran.warn.contains(&format!(
            "sbx secret rm --placeholder sbxm-jev-{id}-0 --sandbox sbxm-run-{id}-jev-0 -f"
        )),
        "{}",
        ran.warn
    );
    assert_eq!(evals(&env, &ran, 0, 0)["jev"]["status"], "ok");
}

// ---- retries ------------------------------------------------------------------------------

#[test]
fn rate_limits_and_overload_retry_after_2_4_and_8_seconds() {
    let env = Env::new();
    let jev = FakeJev::new(vec![
        (429, String::new()),
        (529, "{}".into()),
        (429, String::new()),
    ]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert_eq!(
        ran.sleeps,
        [2, 4, 8].map(Duration::from_secs),
        "{}",
        ran.warn
    );
    // Four calls for the first pair (each retry a fresh exec in the same sandbox), one for the second.
    assert_eq!(jev_execs(&backend), 5);
    assert_eq!(evals(&env, &ran, 0, 0)["jev"]["status"], "ok");
}

#[test]
fn it_gives_up_after_three_retries() {
    let env = Env::new();
    let jev = FakeJev::new(vec![(429, String::new()); 4]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert_eq!(ran.sleeps, [2, 4, 8].map(Duration::from_secs));
    assert_eq!(jev_execs(&backend), 5);
    let error = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(
        error.as_str().unwrap().contains("gave up after 3 retries"),
        "{error}"
    );
    assert_eq!(evals(&env, &ran, 1, 0)["jev"]["status"], "ok");
}

#[test]
fn a_400_and_a_422_are_not_retried() {
    let env = Env::new();
    let jev = FakeJev::new(vec![
        (400, r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#.into()),
        (
            422,
            r#"{"detail":[{"type":"missing","loc":["body","model"],"msg":"Field required","input":{}}]}"#
                .into(),
        ),
    ]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert_eq!(jev_execs(&backend), 2);
    assert!(ran.sleeps.is_empty());
    let first = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(
        first.as_str().unwrap().contains("max_tokens_exceeded"),
        "{first}"
    );
    let second = evals(&env, &ran, 1, 0)["jev"]["error"].clone();
    assert!(
        second
            .as_str()
            .unwrap()
            .contains("body.model: Field required"),
        "{second}"
    );
}

#[test]
fn a_422_echoing_the_request_leaves_its_text_in_no_result_log_or_warning() {
    let env = Env::new();
    let sentinel = "ECHOED-REQUEST-SENTINEL-77";
    let echo = json!({"detail": [{
        "type": "dict_type", "loc": ["body", "questions"], "msg": "Input should be a valid dictionary",
        "input": {"model": "jev-latest", "state": sentinel},
    }]})
    .to_string();
    let jev = FakeJev::new(vec![(422, echo.clone()), (422, echo)]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    let error = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(
        error
            .as_str()
            .unwrap()
            .contains("body.questions: Input should be a valid dictionary"),
        "{error}"
    );
    assert!(!ran.out.contains(sentinel) && !ran.warn.contains(sentinel));
    for path in files(&env.base_dir()) {
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        assert!(
            !text.contains(sentinel),
            "{} holds the echoed body",
            path.display()
        );
    }
}

/// A backend whose every Jev call runs `write` in the call folder (a fake `curl` writing its
/// response) and then ends with `output`.
fn jev_ending(
    env: &Env,
    write: impl Fn(&Path) + Send + Sync + 'static,
    output: ExecOutput,
) -> FakeBackend {
    let base = env.base_dir();
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_version("0.46.0")
        .with_default_exec_output(out(CLAUDE_PONG, "", 0))
        .with_exec_responder(move |sandbox, spec| {
            if !is_jev(spec) {
                return None;
            }
            write(&call_dir(&base, sandbox));
            Some(output.clone())
        })
}

/// Runs `backend` and checks that the pair's Jev result is an error and that `sentinel` is in no
/// file under the base dir, nor in the run's output or warnings (PR 289 review M-1).
fn assert_response_gone(env: &Env, backend: &FakeBackend, sentinel: &str) {
    let ran = go(env, &config(env, ""), backend);
    let error = evals(env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(error.is_string(), "{error}");
    assert!(!error.as_str().unwrap().contains(sentinel), "{error}");
    assert!(!ran.out.contains(sentinel) && !ran.warn.contains(sentinel));
    for path in files(&env.base_dir()) {
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        assert!(
            !text.contains(sentinel),
            "{} holds the raw response",
            path.display()
        );
    }
}

#[test]
fn a_curl_with_no_usable_status_leaves_no_raw_response() {
    let env = Env::new();
    let sentinel = "RAW-RESPONSE-SENTINEL-NOSTATUS";
    let backend = jev_ending(
        &env,
        move |dir| std::fs::write(dir.join("response.json"), sentinel).unwrap(),
        out("", "curl: (18) transfer closed", 18),
    );
    assert_response_gone(&env, &backend, sentinel);
}

#[test]
fn a_timed_out_call_leaves_no_raw_response() {
    let env = Env::new();
    let sentinel = "RAW-RESPONSE-SENTINEL-TIMEOUT";
    let backend = jev_ending(
        &env,
        move |dir| std::fs::write(dir.join("response.json"), sentinel).unwrap(),
        out("", "timeout: sending signal TERM to command 'curl'", 124),
    );
    assert_response_gone(&env, &backend, sentinel);
}

#[test]
fn an_oversized_response_is_removed() {
    let env = Env::new();
    let sentinel = "RAW-RESPONSE-SENTINEL-OVERSIZE";
    let backend = jev_ending(
        &env,
        move |dir| {
            let body = format!("{sentinel}{}", "x".repeat(5 * 1024 * 1024));
            std::fs::write(dir.join("response.json"), body).unwrap()
        },
        out("200", "", 0),
    );
    assert_response_gone(&env, &backend, sentinel);
}

#[cfg(unix)]
#[test]
fn a_response_that_is_a_link_is_removed_and_its_target_never_read() {
    // The link's target is outside the base dir, so a link left behind is found by the scan
    // (which reads through it) and the target itself is never the scan's business.
    let env = Env::new();
    let sentinel = "RAW-RESPONSE-SENTINEL-LINK";
    let target = env.tmp.path().join("outside.json");
    std::fs::write(&target, sentinel).unwrap();
    let backend = jev_ending(
        &env,
        move |dir| common::file_link(&dir.join("response.json"), &target),
        out("200", "", 0),
    );
    assert_response_gone(&env, &backend, sentinel);
}

// ---- budget and chunks --------------------------------------------------------------------

#[test]
fn a_task_over_the_budget_makes_no_call_and_names_the_task() {
    let env = Env::new();
    let backend = FakeJev::new(vec![]).backend(&env);
    let body = format!(
        "[task]\nprompt = \"{}\"\n\n{CONTESTANTS}{RUBRIC}{}",
        "t".repeat(100_000),
        jev_table(&env)
    );
    let ran = go(&env, &body, &backend);
    assert_eq!(jev_execs(&backend), 0);
    assert!(
        !backend.log().iter().any(|l| l.contains("-jev-")),
        "{:?}",
        backend.log()
    );
    let error = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    assert!(error.as_str().unwrap().contains("task"), "{error}");
}

#[test]
fn an_answer_over_the_budget_makes_no_call_for_that_pair_and_names_the_answer() {
    let env = Env::new();
    let backend =
        FakeJev::new(vec![]).backend_answering(&env, &claude_answer(&"a".repeat(100_000)));
    let ran = go(&env, &config(&env, ""), &backend);
    assert_eq!(jev_execs(&backend), 0);
    let error = evals(&env, &ran, 1, 0)["jev"]["error"].clone();
    assert!(error.as_str().unwrap().contains("answer"), "{error}");
}

/// Contestants write one file per entry of `sizes`, each that many characters (45k is about 15k
/// tokens, so no two such files fit one request).
fn big_files_backend(env: &Env, jev: &FakeJev, sizes: &'static [usize]) -> FakeBackend {
    with_files(env, jev.backend(env), sizes)
}

/// `backend` with contestants that also write one file per entry of `sizes`.
fn with_files(env: &Env, backend: FakeBackend, sizes: &'static [usize]) -> FakeBackend {
    let base = env.base_dir();
    backend.with_exec_hook(move |sandbox, spec| {
        if !is_jev(spec) && !sandbox.contains("-jev-") {
            for (i, size) in sizes.iter().enumerate() {
                std::fs::write(
                    workspace_of(&base, sandbox).join(format!("file{i}.txt")),
                    "x".repeat(*size),
                )
                .unwrap();
            }
        }
    })
}

#[test]
fn a_chunked_pair_sends_the_first_responses_model_with_every_later_chunk() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = big_files_backend(&env, &jev, &[45_000, 45_000, 45_000]);
    let ran = go(&env, &config(&env, ""), &backend);
    let requests = jev.requests();
    assert_eq!(requests.len(), 6, "three chunks per pair");
    for pair in requests.chunks(3) {
        assert_eq!(pair[0]["model"], "jev-latest");
        assert_eq!(pair[1]["model"], "jev-1.13.0");
        assert_eq!(pair[2]["model"], "jev-1.13.0");
    }
    let entry = &evals(&env, &ran, 0, 0)["jev"];
    assert_eq!(entry["chunks"], 3);
    assert_eq!(entry["model"], "jev-1.13.0");
    assert!(entry.get("truncated").is_none());
}

#[test]
fn a_later_chunk_from_another_model_makes_the_pair_an_error() {
    let env = Env::new();
    let jev = FakeJev::new(vec![
        (200, answer("jev-1.13.0", 0.9)),
        (200, answer("jev-1.14.0", 0.9)),
    ]);
    let backend = big_files_backend(&env, &jev, &[45_000, 45_000, 45_000]);
    let ran = go(&env, &config(&env, ""), &backend);
    let error = evals(&env, &ran, 0, 0)["jev"]["error"].clone();
    let error = error.as_str().unwrap();
    assert!(
        error.contains("jev-1.14.0") && error.contains("one model"),
        "{error}"
    );
    // The other pair is unaffected.
    assert_eq!(evals(&env, &ran, 1, 0)["jev"]["status"], "ok");
    assert_eq!(
        read(meta(&env, &ran).join("run.json"))["jev_models"],
        json!(["jev-1.13.0"])
    );
}

#[test]
fn chunk_scores_combine_by_size() {
    let env = Env::new();
    // Contestant 0's two chunks answer 1.0 and 0.0; weights are the chunks' sizes.
    let jev = FakeJev::new(vec![
        (200, answer("jev-1.13.0", 1.0)),
        (200, answer("jev-1.13.0", 0.0)),
    ]);
    let backend = big_files_backend(&env, &jev, &[60_000, 30_000]);
    let ran = go(&env, &config(&env, ""), &backend);
    let requests = jev.requests();
    let weight = |r: &Value| {
        let state = r["state"].as_str().unwrap();
        state[state.find("diff --git").unwrap()..].trim_end().len() as f64
    };
    let (w1, w2) = (weight(&requests[0]), weight(&requests[1]));
    let entry = &evals(&env, &ran, 0, 0)["jev"];
    assert_eq!(entry["chunks"], 2);
    let score = entry["criteria"]["correct"]["score"].as_f64().unwrap();
    assert!(
        (score - w1 / (w1 + w2)).abs() < 0.001,
        "{score} vs {}",
        w1 / (w1 + w2)
    );
}

#[test]
fn a_single_file_over_the_budget_is_truncated_and_recorded() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = big_files_backend(&env, &jev, &[200_000]);
    let ran = go(&env, &config(&env, ""), &backend);
    let entry = &evals(&env, &ran, 0, 0)["jev"];
    assert_eq!(entry["truncated"], true);
    for request in jev.requests() {
        assert!(request["state"].as_str().unwrap().len() < 100_000);
    }
    // A one-file diff only: the near-limit multi-file fallback has its own tests below.
    let shown = show(&env, &ran);
    assert!(shown.contains("the diff was cut"), "{shown}");
    assert!(!shown.contains("a file was cut"), "{shown}");
}

/// `config`'s task and rubric as the planner sees them.
fn planned_rubric() -> Vec<Criterion> {
    let criterion = |id: &str, kind, levels: &[&str], weight| Criterion {
        id: id.into(),
        kind,
        levels: levels.iter().map(|l| l.to_string()).collect(),
        weight,
        notes: None,
    };
    vec![
        criterion("correct", CriterionKind::PassFail, &[], 1.0),
        criterion("quality", CriterionKind::Scale, &["poor", "good"], 0.5),
    ]
}

/// The longest answer Jev's planner accepts for `config`'s task with no diff, found without any
/// diff so the boundary can't move with the diff under test.
fn longest_answer_accepted_alone() -> usize {
    let rubric = planned_rubric();
    let accepted =
        |n: usize| jev::plan("Implement feature X", &rubric, &[], &"a".repeat(n), None).is_ok();
    let (mut low, mut high) = (0, 3 * jev::TOKEN_CAP);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if accepted(mid) {
            low = mid
        } else {
            high = mid - 1
        }
    }
    assert!(accepted(low) && !accepted(low + 1), "{low}");
    low
}

/// Runs both contestants answering `answer_len` characters with two 45k-character files, and
/// returns the run, Jev's requests and `run show`'s text.
fn two_files_with_answer(answer_len: usize) -> (Env, Ran, Vec<Value>, String) {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let answering = jev.backend_answering(&env, &claude_answer(&"a".repeat(answer_len)));
    let backend = with_files(&env, answering, &[45_000, 45_000]);
    let ran = go(&env, &config(&env, ""), &backend);
    let shown = show(&env, &ran);
    (env, ran, jev.requests(), shown)
}

/// Both pairs were scored from one unsplit request each with the diff cut, and saved and shown
/// as cut (PR 289 review round 3, S-1).
fn assert_one_cut_request_each(env: &Env, ran: &Ran, requests: &[Value], shown: &str) {
    assert_eq!(requests.len(), 2, "one request per pair");
    for request in requests {
        let state = request["state"].as_str().unwrap();
        assert!(state.contains("\n## Diff\n\n"), "{state}");
        assert!(!state.contains("split per file"), "{state}");
        assert!(
            state.matches("diff --git").count() <= 1,
            "both files sent: {state}"
        );
    }
    for contestant in 0..2 {
        let entry = &evals(env, ran, contestant, 0)["jev"];
        assert_eq!(entry["status"], "ok", "{entry}");
        assert_eq!(entry["truncated"], true, "{entry}");
        assert!(entry.get("chunks").is_none_or(|c| c == 1), "{entry}");
    }
    assert_eq!(shown.matches("the diff was cut").count(), 2, "{shown}");
    assert!(!shown.contains("a file was cut"), "{shown}");
}

#[test]
fn the_longest_answer_accepted_alone_with_two_oversized_files_gets_the_diff_cut_to_nothing() {
    let (env, ran, requests, shown) = two_files_with_answer(longest_answer_accepted_alone());
    assert_one_cut_request_each(&env, &ran, &requests, &shown);
    for request in &requests {
        let state = request["state"].as_str().unwrap();
        assert!(
            state.ends_with("\n## Diff\n\n\n"),
            "nothing of the diff left"
        );
    }
}

#[test]
fn an_answer_15_short_of_the_boundary_gets_one_request_with_the_diff_cut() {
    // Under the 30 characters a split's longer heading adds, so still one unsplit request.
    let (env, ran, requests, shown) = two_files_with_answer(longest_answer_accepted_alone() - 15);
    assert_one_cut_request_each(&env, &ran, &requests, &shown);
}

#[test]
fn a_short_answer_with_two_oversized_files_is_split_per_file_and_not_cut() {
    let (env, ran, requests, shown) = two_files_with_answer(10);
    assert_eq!(requests.len(), 4, "two parts per pair");
    for request in &requests {
        let state = request["state"].as_str().unwrap();
        assert!(state.contains(", split per file)"), "{state}");
        assert!(!state.contains("[file truncated: "), "{state}");
    }
    for contestant in 0..2 {
        let entry = &evals(&env, &ran, contestant, 0)["jev"];
        assert_eq!(entry["chunks"], 2, "{entry}");
        assert!(entry.get("truncated").is_none(), "{entry}");
    }
    assert!(shown.contains("2 chunks"), "{shown}");
    assert!(!shown.contains("was cut"), "{shown}");
}

// ---- the token ----------------------------------------------------------------------------

/// Every file under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.extend(files(&path));
        } else {
            all.push(path);
        }
    }
    all
}

#[test]
fn the_canary_token_never_appears_in_the_run_folders_errors_or_warnings() {
    for replies in [vec![], vec![(401, AUTH_BODY.to_owned()); 2]] {
        let env = Env::new();
        let backend = FakeJev::new(replies).backend(&env);
        let ran = go(&env, &config(&env, ""), &backend);
        assert!(!ran.out.contains("CANARY") && !ran.warn.contains("CANARY"));
        let roots = [
            meta(&env, &ran),
            env.base_dir().join("runs").join(&ran.summary.run_id),
        ];
        for root in roots {
            let all = files(&root);
            assert!(!all.is_empty());
            for path in all {
                let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
                assert!(
                    !text.contains("CANARY"),
                    "{} holds the token",
                    path.display()
                );
            }
        }
    }
}

// ---- rankings -----------------------------------------------------------------------------

fn codex_verdict(a_correct: bool) -> String {
    let verdict = json!({
        "A": {"correct": a_correct, "quality": "good"},
        "B": {"correct": !a_correct, "quality": "poor"},
    })
    .to_string();
    let item = json!({"type": "item.completed", "item": {"id": "i", "type": "agent_message", "text": verdict}});
    format!(
        "{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}\n{item}\n{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":2}}}}\n"
    )
}

fn show(env: &Env, ran: &Ran) -> String {
    sbxm::commands::run_show::render(&env.config_dir(), &ran.summary.run_id, &Default::default())
        .unwrap()
}

#[test]
fn with_the_judge_and_jev_the_judges_ranking_leads_and_jevs_follows_never_blended() {
    let env = Env::new();
    // Jev prefers contestant 0: 0.9 and 0.2 on `correct`, 0.8 on quality for both.
    let jev = FakeJev::new(vec![
        (200, answer("jev-1.13.0", 0.9)),
        (200, answer("jev-1.13.0", 0.2)),
    ]);
    let backend = jev
        .backend(&env)
        .with_exec_output_matching("judge-prompt.md", out(&codex_verdict(true), "", 0));
    let body = format!(
        "{}[eval.judge]\nharness = \"codex\"\nmodel = \"judge-model\"\n",
        config(&env, "")
    );
    let ran = go(&env, &body, &backend);

    for text in [ran.out.clone(), show(&env, &ran)] {
        let judge = text.find("Ranking (judge scores 0-1").expect(&text);
        let jev_at = text
            .find("Jev ranking (jev-1.13.0; scores 0-1")
            .expect(&text);
        assert!(judge < jev_at, "{text}");
        let jev_block = &text[jev_at..];
        // (0.9 * 1 + 0.8 * 0.5) / 1.5 and (0.2 * 1 + 0.8 * 0.5) / 1.5.
        assert!(
            jev_block.contains("1. contestants[0] claude (one): 0.87"),
            "{jev_block}"
        );
        assert!(
            jev_block.contains("2. contestants[1] claude (two): 0.40"),
            "{jev_block}"
        );
    }
    let shown = show(&env, &ran);
    assert!(
        shown.contains("Jev (jev-1.13.0): correct 0.90, quality 0.80 (confidence 0.70)"),
        "{shown}"
    );
}

#[test]
fn with_jev_only_run_and_run_show_still_print_a_ranking() {
    let env = Env::new();
    let backend = FakeJev::new(vec![]).backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert!(!ran.out.contains("Ranking (judge"), "{}", ran.out);
    assert!(ran.out.contains("Jev ranking (jev-1.13.0;"), "{}", ran.out);
    let shown = show(&env, &ran);
    assert!(!shown.contains("Ranking (judge"), "{shown}");
    assert!(shown.contains("Jev ranking (jev-1.13.0;"), "{shown}");
    // Both tie: 0.9 on correct and 0.8 on quality each.
    assert!(
        shown.contains("1. contestants[0] claude (one): 0.87"),
        "{shown}"
    );
    assert!(
        shown.contains("1. contestants[1] claude (two): 0.87"),
        "{shown}"
    );
}

#[test]
fn a_pair_jev_could_not_score_is_left_out_of_jevs_ranking_and_shown_with_its_error() {
    let env = Env::new();
    let backend = FakeJev::new(vec![(401, AUTH_BODY.into())]).backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    let shown = show(&env, &ran);
    assert!(
        shown.contains("contestants[0] claude (one): not scored (Jev scored none of its repeats)"),
        "{shown}"
    );
    assert!(
        shown.contains("Jev: not scored: Jev rejected the token (401"),
        "{shown}"
    );
}

// ---- raw questions (issue 343) ------------------------------------------------------------

const QUESTIONS: &str = "[[eval.jev.questions]]\nid = \"tests\"\nkind = \"noul\"\n\
                         question = \"Did it add tests?\"\n\n\
                         [[eval.jev.questions]]\nid = \"approach\"\nkind = \"choice\"\n\
                         question = \"Which approach did the contestant take?\"\n\
                         options = { refactor = \"Restructured the existing code\", rewrite = \"Replaced it\" }\n\n\
                         [[eval.jev.questions]]\nid = \"depth\"\nkind = \"score\"\n\
                         question = \"How fully are edge cases handled?\"\n\
                         levels = [\"none\", \"partial\", \"full\"]\n";

/// A 200 body answering both criteria (`correct` at `noul`) and the three raw questions.
fn answer_with_questions(noul: f64, tests: f64, refactor: f64) -> String {
    let mut body: Value = serde_json::from_str(&answer("jev-1.14.0", noul)).unwrap();
    let answers = body["answers"].as_object_mut().unwrap();
    answers.insert("tests".into(), json!({"type": "noul", "noul": tests}));
    let choice = if refactor >= 0.5 {
        "refactor"
    } else {
        "rewrite"
    };
    answers.insert(
        "approach".into(),
        json!({"type": "choice", "choice": choice, "confidence": 0.9,
               "probabilities": {"refactor": refactor, "rewrite": 1.0 - refactor}}),
    );
    answers.insert(
        "depth".into(),
        json!({"type": "score", "score": 1.18, "confidence": 0.73, "legend": {}, "probabilities": {}}),
    );
    body.to_string()
}

fn config_with_questions(env: &Env) -> String {
    format!("{}{QUESTIONS}", config(env, ""))
}

#[test]
fn raw_questions_are_asked_in_the_same_call_as_the_rubric() {
    let env = Env::new();
    let jev = FakeJev::new(vec![]);
    let backend = jev.backend(&env);
    go(&env, &config_with_questions(&env), &backend);
    let requests = jev.requests();
    // Still one call per contestant per repeat.
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let questions = &request["questions"];
        assert_eq!(questions["correct"]["type"], "noul");
        assert_eq!(questions["tests"]["type"], "noul");
        assert_eq!(questions["approach"]["type"], "choice");
        assert_eq!(
            questions["approach"]["criteria"],
            json!({"refactor": "Restructured the existing code", "rewrite": "Replaced it"})
        );
        assert_eq!(
            questions["depth"]["criteria"],
            json!(["none", "partial", "full"])
        );
    }
}

#[test]
fn evals_json_keeps_the_raw_answers_with_a_choices_whole_distribution() {
    let env = Env::new();
    let jev = FakeJev::new(vec![
        (200, answer_with_questions(0.9, 0.95, 0.72)),
        (200, answer_with_questions(0.2, 0.1, 0.25)),
    ]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config_with_questions(&env), &backend);
    let first = &evals(&env, &ran, 0, 0)["jev"];
    assert_eq!(
        first["questions"],
        json!({
            "tests": {"kind": "noul", "yes": 0.95},
            "approach": {
                "kind": "choice",
                "choice": "refactor",
                "probability": 0.72,
                "probabilities": {"refactor": 0.72, "rewrite": 1.0 - 0.72},
            },
            "depth": {"kind": "score", "score": 1.18, "confidence": 0.73},
        })
    );
    assert_eq!(first["unanswered"], json!([]));
    // The criteria are what they were without raw questions.
    assert_eq!(
        first["criteria"],
        json!({"correct": {"score": 0.9}, "quality": {"score": 0.8, "confidence": 0.7}})
    );
}

#[test]
fn raw_answers_are_shown_one_line_per_contestant_and_never_change_the_ranking() {
    let env = Env::new();
    // Contestant 1's raw answers are better in every way; the ranking must not notice.
    let jev = FakeJev::new(vec![
        (200, answer_with_questions(0.9, 0.05, 0.72)),
        (200, answer_with_questions(0.2, 0.99, 0.25)),
    ]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config_with_questions(&env), &backend);
    for text in [ran.out.clone(), show(&env, &ran)] {
        let ranking = text.find("Jev ranking (jev-1.14.0;").expect(&text);
        let block_at = text
            .find("Jev questions (shown only, never ranked)")
            .expect(&text);
        assert!(ranking < block_at, "{text}");
        let ranking_block = &text[ranking..block_at];
        assert!(
            ranking_block.contains("1. contestants[0] claude (one): 0.87"),
            "{ranking_block}"
        );
        assert!(
            ranking_block.contains("2. contestants[1] claude (two): 0.40"),
            "{ranking_block}"
        );
        assert!(
            text.contains(
                "  contestants[0] claude (one): tests: no (0.95); approach: refactor (0.72); \
                 depth: partial (1.18 on 0-2, confidence 0.73)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "  contestants[1] claude (two): tests: yes (0.99); approach: rewrite (0.75); \
                 depth: partial (1.18 on 0-2, confidence 0.73)\n"
            ),
            "{text}"
        );
        // The distribution is saved, never printed.
        assert!(!text.contains("0.28") && !text.contains("0.25"), "{text}");
    }
}

#[test]
fn an_unanswered_raw_question_and_a_pair_jev_could_not_score_are_said_so() {
    let env = Env::new();
    let mut partial: Value = serde_json::from_str(&answer_with_questions(0.9, 0.95, 0.72)).unwrap();
    partial["answers"].as_object_mut().unwrap().remove("depth");
    let jev = FakeJev::new(vec![(200, partial.to_string()), (401, AUTH_BODY.into())]);
    let backend = jev.backend(&env);
    let ran = go(&env, &config_with_questions(&env), &backend);
    assert_eq!(
        evals(&env, &ran, 0, 0)["jev"]["unanswered"],
        json!(["depth"])
    );
    for text in [ran.out.clone(), show(&env, &ran)] {
        assert!(
            text.contains(
                "  contestants[0] claude (one): tests: yes (0.95); approach: refactor (0.72); \
                 unanswered: depth\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("  contestants[1] claude (two): not answered (Jev could not score it)\n"),
            "{text}"
        );
    }
}

#[test]
fn every_repeat_gets_its_own_line() {
    let env = Env::new();
    let backend = FakeJev::new(vec![]).backend(&env);
    let body = format!("{}{QUESTIONS}", config(&env, "[run]\nrepeat = 2\n\n"));
    let ran = go(&env, &body, &backend);
    for text in [ran.out.clone(), show(&env, &ran)] {
        for line in [
            "  contestants[0] claude (one), repeat 1/2: unanswered: tests, approach, depth\n",
            "  contestants[0] claude (one), repeat 2/2: unanswered: tests, approach, depth\n",
            "  contestants[1] claude (two), repeat 2/2: unanswered: tests, approach, depth\n",
        ] {
            assert!(text.contains(line), "{line:?} in {text}");
        }
    }
}

#[test]
fn without_raw_questions_there_is_no_questions_block_or_key() {
    let env = Env::new();
    let backend = FakeJev::new(vec![]).backend(&env);
    let ran = go(&env, &config(&env, ""), &backend);
    assert!(!ran.out.contains("Jev questions"), "{}", ran.out);
    assert!(!show(&env, &ran).contains("Jev questions"));
    let jev = &evals(&env, &ran, 0, 0)["jev"];
    assert!(
        jev.get("questions").is_none() && jev.get("unanswered").is_none(),
        "{jev:#}"
    );
}
