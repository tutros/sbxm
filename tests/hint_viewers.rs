//! Issue 574, review M-2 and M-3: the commands that show stored content (a run's answers and
//! patches, a task's `run.log`, the dashboard's LOG column) show it as stored, and only the hints
//! they generate themselves name the running executable; the dashboard carries the block that says
//! where to run them in every frame that has such a hint.

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::Env;
use sbxm::backend::FakeBackend;
use sbxm::commands::status;
use sbxm::hint::{self, Shell};
use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record};
use serde_json::json;

const T0: u64 = 1_790_000_000;
const RUN: &str = "2026-11-11-a1b2c3";

fn sbxm() -> Command {
    Command::cargo_bin("sbxm").unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A saved run whose one answer and patch are command-shaped text.
fn saved_run(env: &Env) {
    let meta = env.base_dir().join(".sbxm").join("runs").join(RUN);
    let run = json!({
        "run_id": RUN,
        "started_at": "2026-11-11T18:00:00Z",
        "completed_at": "2026-11-11T18:01:00Z",
        "sbxm_version": "0.1.0",
        "sbx_version": "0.43.0",
        "skills": null,
        "profile": "default",
        "resources": {"cpus": 4, "memory": "8g"},
        "repeat": 1,
        "contestants": [
            {"index": 0, "harness": "claude", "model": "model-a", "profile": "default"},
        ],
        "harnesses": [
            {"profile": "default", "harness": "claude", "config_hash": "abc", "kits": []},
        ],
    });
    write(&meta.join("run.json"), &run.to_string());
    let pair = meta.join("0").join("0");
    let result = json!({
        "status": "completed", "error": null, "remove_error": null,
        "usage": {"input_tokens": 1, "output_tokens": 1, "cost_usd": null},
        "diff": {"status": "ok", "skipped": []},
    });
    write(&pair.join("result.json"), &result.to_string());
    write(&pair.join("answer.md"), "sbxm task status");
    write(
        &pair.join("diff.patch"),
        "diff --git a/a.md b/a.md\n--- a/a.md\n+++ b/a.md\n@@ -1 +1 @@\n-x\n+run `sbxm task status`\n",
    );
}

#[test]
fn run_show_keeps_a_saved_answer_and_patch_as_saved() {
    let env = Env::new();
    saved_run(&env);
    for args in [
        &["run", "show", RUN][..],
        &["run", "show", RUN, "--diff"][..],
    ] {
        let output = sbxm()
            .env("SBXM_CONFIG_DIR", env.config_dir())
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout.clone()).unwrap();
        assert!(
            stdout.contains("      sbxm task status\n"),
            "{args:?}: {stdout}"
        );
        if args.len() == 4 {
            assert!(stdout.contains("+run `sbxm task status`"), "{stdout}");
        }
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    }
}

fn save_task(base: &Path) -> std::path::PathBuf {
    let task = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    );
    let dir = record::task_dir(base, &task.id);
    record::write(&dir, &task).unwrap();
    dir
}

const LOGGED: &str = "2026-11-11T00:00:00Z [prepared] next: sbxm task resume --issue 41";

#[test]
fn task_logs_shows_a_stored_log_as_stored() {
    let env = Env::new();
    let dir = save_task(&env.base_dir());
    write(&dir.join("run.log"), &format!("{LOGGED}\n"));
    let elsewhere = env.tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let output = sbxm()
        .current_dir(&elsewhere)
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["task", "logs", "--issue", "41"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(stdout.ends_with(&format!("{LOGGED}\n")), "{stdout}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

struct Probe;

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn status_env(terminal: bool, iterations: Option<usize>) -> status::Env {
    status::Env {
        now: Box::new(|| T0),
        sleep: Box::new(|_| {}),
        terminal,
        width: 400,
        iterations,
        monotonic: Box::new(|| std::time::Duration::ZERO),
        slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(std::path::PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    }
}

fn dashboard(base: &Path, terminal: bool, iterations: Option<usize>) -> String {
    let mut out = Vec::new();
    status::run_with(
        base,
        &FakeBackend::default(),
        &Probe,
        &status_env(terminal, iterations),
        iterations.map(|_| std::time::Duration::from_secs(1)),
        None,
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

fn program() -> String {
    Shell::HOST.program(&std::env::current_exe().unwrap().to_string_lossy())
}

// The test below turns on the binary's hint mode for this test process, as `main` does.

#[test]
fn the_dashboard_log_column_shows_the_stored_line() {
    let env = Env::new();
    let dir = save_task(&env.base_dir());
    write(&dir.join("run.log"), &format!("{LOGGED}\n"));
    let output = sbxm()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(["status"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        stdout.contains("next: sbxm task resume --issue 41"),
        "{stdout}"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn every_dashboard_frame_with_a_hint_carries_the_block() {
    hint::enable();
    let env = Env::new();
    let shown = dashboard(&env.base_dir(), true, Some(2));
    let hint = format!(
        "No tasks yet; start one with `{} task start --issue <n>`.",
        program()
    );
    assert_eq!(shown.matches(&hint).count(), 2, "{shown}");
    assert_eq!(
        shown
            .matches("to run the commands above as this one ran, first:")
            .count(),
        2,
        "{shown}"
    );
}
