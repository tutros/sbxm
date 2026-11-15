//! Issue 419: `task.json` keeps every run of a task's worker, fix rounds and reviewer in order
//! (`runs`: role, round, the model the harness reported or `unknown`, status, usage, duration),
//! next to the latest-run fields `worker.run` and `reviewer.run`, which stay as they were. A record
//! written before `runs` existed still loads.

use crate::common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Play, ctx, fixture_with, ok, play_reviews, play_tasks, source,
    worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline;
use sbxm::task::record::{self, SandboxRole};

/// The model `CLAUDE_DONE` reports.
const CLAUDE_MODEL: &str = "claude-haiku-4-5-20251001";

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str =
    "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 does the wrong thing.\n";

/// A Claude worker with one fix round and a Codex reviewer: the first review finds one must-fix
/// finding, the narrow second one none, and the full third one none.
fn reviewed() -> (common::task_fixture::Fixture, record::Record) {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    );
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        vec![ONE.into(), CLEAN.into(), CLEAN.into()],
    );
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        });
    let mut prepared = worked_task(&f, &backend);
    let github = FakeGitHub::default();
    let source = source(&f);
    pipeline::review_issue(&ctx(&f, &source, &backend, &github).env(), &mut prepared).unwrap();
    let task = record::read(&task_json(&f)).unwrap();
    (f, task)
}

fn task_json(f: &common::task_fixture::Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")
}

#[test]
fn task_runs_keeps_every_run_in_order_with_its_role_and_round() {
    let (_f, task) = reviewed();

    let order: Vec<(SandboxRole, u32)> = task.runs.iter().map(|r| (r.role, r.round)).collect();
    assert_eq!(
        order,
        [
            (SandboxRole::Worker, 0),
            (SandboxRole::Reviewer, 1),
            (SandboxRole::Fix, 1),
            (SandboxRole::Reviewer, 2),
            (SandboxRole::Reviewer, 3),
        ]
    );
    assert!(
        task.runs.iter().all(|r| r.status == "completed"),
        "{:?}",
        task.runs
    );
}

#[test]
fn task_runs_records_the_model_the_harness_reported_or_unknown() {
    let (_f, task) = reviewed();

    let models: Vec<&str> = task.runs.iter().map(|r| r.model.as_str()).collect();
    // Claude names its model; Codex's output names none.
    assert_eq!(
        models,
        [CLAUDE_MODEL, "unknown", CLAUDE_MODEL, "unknown", "unknown"]
    );
}

#[test]
fn task_runs_records_each_runs_usage_and_keeps_the_latest_run_fields() {
    let (_f, task) = reviewed();

    let worker = task.worker.as_ref().unwrap().run.as_ref().unwrap();
    let reviewer = task.reviewer.as_ref().unwrap().run.as_ref().unwrap();
    assert_eq!(task.runs[0].usage, worker.usage);
    assert_eq!(task.runs[4].usage, reviewer.usage);
    assert_eq!(task.runs[2].usage, worker.usage, "the same fixture output");
    assert!(task.runs[0].usage["output_tokens"].as_u64().unwrap() > 0);
    for run in &task.runs {
        for key in ["input_tokens", "output_tokens", "cost_usd"] {
            assert!(run.usage.get(key).is_some(), "{key} in {:?}", run.usage);
        }
    }
}

#[test]
fn task_runs_an_old_record_without_runs_still_loads() {
    let (f, _) = reviewed();
    let path = task_json(&f);
    let mut json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("runs").unwrap();
    fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();

    let task = record::read(&path).unwrap();

    assert!(task.runs.is_empty());
    assert!(task.worker.unwrap().run.is_some());
}

#[test]
fn task_runs_round_trips_through_task_json() {
    let (f, task) = reviewed();
    let path = task_json(&f);
    record::write(path.parent().unwrap(), &task).unwrap();

    assert_eq!(record::read(&path).unwrap().runs, task.runs);
}
