//! M2a slice 11, the runner: a throwaway judge sandbox per repeat index that
//! sees only that repeat's answers and diffs under blind labels (P3, decisions
//! 16, 21, 95, 114), and the provider-sharing warning.

mod common;

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend, SkillsStore};
use sbxm::eval::judge::{self, JudgeRun};
use sbxm::harness::Harness;
use sbxm::run::config::RunConfig;
use sbxm::run::id::RunRoots;
use sbxm::run::kits::{self, RunKits};
use sbxm::run::orchestrate::{self, PairOutcome};
use sbxm::run::preflight;
use serde_json::json;

const RUN_ID: &str = "2026-09-30-abc123";
const RUBRIC: &str = "[[eval.rubric]]\nid = \"correctness\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                      [[eval.rubric]]\nid = \"quality\"\nkind = \"scale\"\nlevels = [\"poor\", \"good\"]\nweight = 0.5\n\n";

/// A Claude `stream-json` run whose final answer is `text`.
fn claude_reply(text: &str) -> String {
    let result = json!({
        "type": "result", "subtype": "success", "is_error": false,
        "result": text, "total_cost_usd": 0.01,
        "usage": {"input_tokens": 10, "output_tokens": 5,
                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
    });
    format!("{result}\n")
}

fn out(stdout: &str, stderr: &str, code: i32) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: stderr.into(),
        exit_code: Some(code),
    }
}

struct Setup {
    env: Env,
    config: RunConfig,
    kits: RunKits,
    roots: RunRoots,
}

fn contestant(harness: &str, model: &str) -> String {
    format!("[[contestants]]\nharness = \"{harness}\"\nmodel = \"{model}\"\n\n")
}

/// Contestants claude/one and claude/two, a claude judge unless `judge` says otherwise.
fn setup(run_table: &str, contestants: &str, judge: &str) -> Setup {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"Implement feature X\"\n\n{run_table}{contestants}{RUBRIC}{judge}"
        ),
    )
    .unwrap();
    let config = RunConfig::load(&path).unwrap();
    let meta = env.tmp.path().join("meta").join(RUN_ID);
    let workspaces = env.tmp.path().join("runs").join(RUN_ID);
    std::fs::create_dir_all(&meta).unwrap();
    std::fs::create_dir_all(&workspaces).unwrap();
    let kits = kits::build(
        &env.config_dir(),
        &config,
        &meta.join("kits"),
        &FakeBackend::default(),
    )
    .unwrap();
    Setup {
        env,
        config,
        kits,
        roots: RunRoots {
            id: RUN_ID.into(),
            meta,
            workspaces,
        },
    }
}

const CLAUDE_JUDGE: &str = "[eval.judge]\nharness = \"claude\"\nmodel = \"judge-model\"\n";

fn two_claudes() -> String {
    format!(
        "{}{}",
        contestant("claude", "one"),
        contestant("claude", "two")
    )
}

fn pair_name(contestant: usize, repeat: u32) -> String {
    format!("sbxm-run-{RUN_ID}-{contestant}-{repeat}")
}

fn judge_name(repeat: u32) -> String {
    format!("sbxm-run-{RUN_ID}-judge-{repeat}")
}

fn contestants_backend() -> FakeBackend {
    // Each pair answers with its own coordinates so prompts can be checked.
    let mut backend = FakeBackend::default();
    for contestant in 0..2 {
        for repeat in 0..2 {
            backend = backend.with_exec_output_for(
                &pair_name(contestant, repeat),
                out(
                    &claude_reply(&format!("answer-{contestant}-{repeat}")),
                    "",
                    0,
                ),
            );
        }
    }
    backend
}

fn run_pairs(s: &Setup, backend: &FakeBackend) -> Vec<PairOutcome> {
    orchestrate::execute(backend, &s.config, &s.kits, &s.roots)
}

fn judge_repeat(
    s: &Setup,
    backend: &FakeBackend,
    outcomes: &[PairOutcome],
    repeat: u32,
) -> Option<JudgeRun> {
    let of_repeat: Vec<&PairOutcome> = outcomes.iter().filter(|o| o.repeat == repeat).collect();
    judge::judge_repeat(backend, &s.config, &s.kits, &s.roots, repeat, &of_repeat)
}

fn verdict() -> String {
    json!({
        "A": {"correctness": {"value": true, "reason": "ok"}, "quality": {"value": "good", "reason": "ok"}},
        "B": {"correctness": {"value": false, "reason": "no"}, "quality": {"value": "poor", "reason": "no"}},
    })
    .to_string()
}

fn prompt_file(s: &Setup, repeat: u32) -> String {
    std::fs::read_to_string(
        s.roots
            .workspaces
            .join("judge")
            .join(repeat.to_string())
            .join("judge-prompt.md"),
    )
    .unwrap()
}

fn calls_for(backend: &FakeBackend, sandbox: &str) -> Vec<String> {
    backend
        .log()
        .into_iter()
        .filter(|l| l.ends_with(sandbox))
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect()
}

// ---- the judge sandbox ---------------------------------------------------------

#[test]
fn the_judge_gets_its_own_sandbox_that_is_created_used_and_removed() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    assert_eq!(run.sandbox, judge_name(0));
    assert_eq!(
        calls_for(&backend, &judge_name(0)),
        ["create", "exec", "rm"]
    );
    let create = backend
        .creates()
        .into_iter()
        .find(|c| c.name == judge_name(0))
        .unwrap();
    assert_eq!(create.agent, Harness::Claude.agent_arg());
    assert_eq!(create.kits, s.kits.get(Harness::Claude).unwrap().dirs);
    assert_eq!(create.skills, SkillsStore::ReadOnly);
    assert_eq!(create.workspace, s.roots.workspaces.join("judge").join("0"));
    assert!(run.remove_error.is_none());
}

#[test]
fn a_valid_reply_becomes_scores_for_each_label() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    let parsed = run.result.as_ref().unwrap();
    assert_eq!(parsed.candidates[&'A'].criteria["correctness"].score, 1.0);
    assert_eq!(parsed.candidates[&'B'].criteria["quality"].score, 0.0);
    // Each contestant has exactly one of the two labels.
    let mut labels: Vec<char> = run.labels.iter().map(|(_, l)| *l).collect();
    labels.sort();
    assert_eq!(labels, ['A', 'B']);
    assert_eq!(run.answer.as_deref(), Some(verdict().as_str()));
}

#[test]
fn the_prompt_is_a_file_in_the_judges_workspace_not_on_the_command_line() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    let judge_exec = backend
        .execs()
        .into_iter()
        .find(|(n, _)| *n == judge_name(0))
        .unwrap()
        .1;
    let command_line = judge_exec.argv.join(" ");
    assert!(command_line.contains("judge-prompt.md"));
    assert!(
        !command_line.contains("answer-0-0"),
        "answers must not ride on the command line"
    );
    assert!(command_line.contains("judge-model"));
    assert!(command_line.contains("claude"));
    // The file holds the task, the rubric and both answers.
    let prompt = prompt_file(&s, 0);
    assert!(prompt.contains("Implement feature X") && prompt.contains("correctness"));
    assert!(prompt.contains("answer-0-0") && prompt.contains("answer-1-0"));
}

#[test]
fn labels_in_the_prompt_match_the_returned_mapping() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    let prompt = prompt_file(&s, 0);
    for (contestant, label) in &run.labels {
        let heading = format!("### Candidate {label}\n\n#### Answer\n\nanswer-{contestant}-0");
        assert!(prompt.contains(&heading), "{heading}\n{prompt}");
    }
}

#[test]
fn with_repeat_two_each_index_is_judged_alone() {
    let s = setup("[run]\nrepeat = 2\n\n", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    let first = judge_repeat(&s, &backend, &outcomes, 0).unwrap();
    let second = judge_repeat(&s, &backend, &outcomes, 1).unwrap();

    assert_eq!(
        (first.sandbox.as_str(), second.sandbox.as_str()),
        (judge_name(0).as_str(), judge_name(1).as_str())
    );
    let (p0, p1) = (prompt_file(&s, 0), prompt_file(&s, 1));
    assert!(p0.contains("answer-0-0") && p0.contains("answer-1-0"));
    assert!(!p0.contains("-1\n") && !p0.contains("answer-0-1") && !p0.contains("answer-1-1"));
    assert!(p1.contains("answer-0-1") && p1.contains("answer-1-1"));
    assert!(!p1.contains("answer-0-0") && !p1.contains("answer-1-0"));
    // Two judge calls, one per index.
    assert_eq!(
        backend
            .creates()
            .iter()
            .filter(|c| c.name.contains("-judge-"))
            .count(),
        2
    );
}

#[test]
fn a_timed_out_or_failed_contestant_is_judged_on_its_partial_output_without_its_status() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_exec_output_for(
            &pair_name(1, 0),
            out(
                &claude_reply("partial-answer"),
                "timeout: sending signal TERM to command 'claude'\n",
                124,
            ),
        )
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);

    judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    let prompt = prompt_file(&s, 0).to_lowercase();
    assert!(prompt.contains("partial-answer"));
    assert!(!prompt.contains("timed out") && !prompt.contains("timeout"));
}

#[test]
fn a_contestant_that_never_ran_is_left_out_and_nothing_to_judge_means_no_call() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let broken = FakeBackend::failing_create();
    let outcomes = run_pairs(&s, &broken);
    assert!(outcomes.iter().all(|o| o.result.is_err()));

    assert!(judge_repeat(&s, &broken, &outcomes, 0).is_none());

    // One pair ran, one didn't: only the one that ran is a candidate.
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend()
        .with_failing_create_for(&pair_name(1, 0))
        .with_exec_output_matching("judge-prompt.md", out(&claude_reply(&verdict()), "", 0));
    let outcomes = run_pairs(&s, &backend);
    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();
    assert_eq!(run.labels.len(), 1);
    assert_eq!(run.labels[0].0, 0);
    assert!(!prompt_file(&s, 0).contains("answer-1-0"));
}

#[test]
fn no_judge_configured_means_no_judge_call() {
    let s = setup("", &two_claudes(), "");
    let backend = contestants_backend();
    let outcomes = run_pairs(&s, &backend);

    assert!(judge_repeat(&s, &backend, &outcomes, 0).is_none());
    assert!(!backend.creates().iter().any(|c| c.name.contains("-judge-")));
}

// ---- when the judge misbehaves: always an error string, always cleaned up --------

fn failing_judge(reply: ExecOutput) -> (Setup, FakeBackend, JudgeRun) {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend().with_exec_output_matching("judge-prompt.md", reply);
    let outcomes = run_pairs(&s, &backend);
    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();
    (s, backend, run)
}

#[test]
fn a_reply_that_is_not_json_is_a_parse_error_and_the_sandbox_is_removed() {
    let (_, backend, run) = failing_judge(out(&claude_reply("I like A better."), "", 0));

    assert!(run.result.unwrap_err().contains("no JSON object"));
    assert_eq!(
        calls_for(&backend, &judge_name(0)),
        ["create", "exec", "rm"]
    );
}

#[test]
fn a_judge_timeout_is_an_error_and_the_sandbox_is_removed() {
    let (_, backend, run) = failing_judge(out(
        &claude_reply(""),
        "timeout: sending signal TERM to command 'claude'\n",
        124,
    ));

    assert!(run.result.unwrap_err().contains("timed out"));
    assert_eq!(
        calls_for(&backend, &judge_name(0)),
        ["create", "exec", "rm"]
    );
}

#[test]
fn a_judge_command_failure_is_an_error_with_its_reason() {
    let (_, _, run) = failing_judge(out("", "boom\n", 1));

    let err = run.result.unwrap_err();
    assert!(
        err.contains("the judge failed") && err.contains("boom"),
        "{err}"
    );
}

#[test]
fn a_backend_error_running_the_judge_is_an_error_and_the_sandbox_is_removed() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend().with_failing_exec_matching("judge-prompt.md");
    let outcomes = run_pairs(&s, &backend);

    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    assert!(run.result.unwrap_err().contains("exec"));
    assert_eq!(
        calls_for(&backend, &judge_name(0)),
        ["create", "exec", "rm"]
    );
}

#[test]
fn a_failed_judge_create_is_an_error_and_is_still_cleaned_up() {
    let s = setup("", &two_claudes(), CLAUDE_JUDGE);
    let backend = contestants_backend().with_failing_create_for(&judge_name(0));
    let outcomes = run_pairs(&s, &backend);

    let run = judge_repeat(&s, &backend, &outcomes, 0).unwrap();

    assert!(
        run.result
            .unwrap_err()
            .contains("cannot create the judge sandbox")
    );
    assert_eq!(calls_for(&backend, &judge_name(0)), ["create", "rm"]);
}

// ---- provider sharing ------------------------------------------------------------

fn warnings(contestants: &str, judge: &str) -> Vec<String> {
    let s = setup("", contestants, judge);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai", "google"]);
    preflight::check(&s.env.config_dir(), &s.config, &backend)
        .unwrap()
        .warnings
}

#[test]
fn a_judge_from_a_contestants_provider_is_warned_about() {
    let w = warnings(
        &format!("{}{}", contestant("claude", "a"), contestant("codex", "b")),
        CLAUDE_JUDGE,
    );

    assert_eq!(
        w,
        [
            "the judge (claude) uses the same provider (anthropic) as contestants[0] (claude); \
          it may favour its own model family"
        ]
    );
}

#[test]
fn every_contestant_sharing_the_judges_provider_is_named() {
    let w = warnings(&two_claudes(), CLAUDE_JUDGE);

    assert!(
        w[0].contains("contestants[0] (claude), contestants[1] (claude)"),
        "{w:?}"
    );
}

#[test]
fn a_judge_from_another_provider_gets_no_warning() {
    let w = warnings(
        &two_claudes(),
        "[eval.judge]\nharness = \"codex\"\nmodel = \"judge-model\"\n",
    );

    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn without_a_judge_there_is_nothing_to_warn_about() {
    assert!(warnings(&two_claudes(), "").is_empty());
}
