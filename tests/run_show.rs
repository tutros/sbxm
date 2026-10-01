//! M2a slice 9: `sbxm run show <run-id>` prints a saved run and nothing else
//! (decision 100): it validates the id first, then only reads files.

mod common;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::{run, run_show};
use serde_json::{Value, json};

const ID: &str = "2026-09-30-abc123";

const PATCH: &str = "diff --git a/a.txt b/a.txt\n\
index 1111111..2222222 100644\n\
--- a/a.txt\n\
+++ b/a.txt\n\
@@ -1 +1 @@\n\
-alpha\n\
+alpha edited\n\
diff --git a/new.txt b/new.txt\n\
new file mode 100644\n\
index 0000000..3333333\n\
--- /dev/null\n\
+++ b/new.txt\n\
@@ -0,0 +1,2 @@\n\
+one\n\
+two\n\
diff --git a/old.txt b/old.txt\n\
deleted file mode 100644\n\
index 4444444..0000000\n\
--- a/old.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-gone\n\
diff --git a/blob.bin b/blob.bin\n\
new file mode 100644\n\
index 0000000..5555555\n\
Binary files /dev/null and b/blob.bin differ\n";

fn meta(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("runs").join(ID)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write_json(path: &Path, value: &Value) {
    write(path, &serde_json::to_string_pretty(value).unwrap());
}

/// A pair's folder as `sbxm run` leaves it; `None` files are left out.
fn pair(env: &Env, i: usize, r: u32, result: Value, answer: Option<&str>, patch: Option<&str>) {
    let dir = meta(env).join(i.to_string()).join(r.to_string());
    write_json(&dir.join("result.json"), &result);
    if let Some(text) = answer {
        write(&dir.join("answer.md"), text);
    }
    if let Some(text) = patch {
        write(&dir.join("diff.patch"), text);
    }
}

fn run_json(env: &Env, completed: Option<&str>) {
    write_json(
        &meta(env).join("run.json"),
        &json!({
            "run_id": ID,
            "started_at": "2026-09-30T18:00:00Z",
            "completed_at": completed,
            "sbxm_version": "0.1.0",
            "sbx_version": "0.43.0",
            "skills": null,
            "profile": "default",
            "resources": {"cpus": 4, "memory": "8g"},
            "repeat": 2,
            "contestants": [
                {"index": 0, "harness": "claude", "model": "model-a", "profile": "default"},
                {"index": 1, "harness": "codex", "model": "model-b", "profile": "default"},
            ],
            "harnesses": [],
        }),
    );
}

fn result(status: &str, error: Value, usage: Value, diff: Value) -> Value {
    json!({"status": status, "error": error, "usage": usage, "diff": diff, "remove_error": null})
}

/// Four pairs with every status, two contestants, two repeats.
fn fixture() -> Env {
    let env = Env::new();
    run_json(&env, Some("2026-09-30T18:01:10Z"));
    pair(
        &env,
        0,
        0,
        result(
            "completed",
            Value::Null,
            json!({"input_tokens": 100, "output_tokens": 20, "cost_usd": 0.01}),
            json!({"status": "ok", "skipped": []}),
        ),
        Some("PONG"),
        Some(PATCH),
    );
    pair(
        &env,
        0,
        1,
        result(
            "timed_out",
            Value::Null,
            json!({"input_tokens": 50, "output_tokens": 5, "cost_usd": null}),
            json!({"status": "ok", "skipped": ["linked"]}),
        ),
        Some("half of the\nanswer"),
        Some(""),
    );
    pair(
        &env,
        1,
        0,
        result(
            "failed",
            json!("exit code 1: boom"),
            json!({"input_tokens": 0, "output_tokens": 0, "cost_usd": null}),
            json!({"status": "error", "error": "cannot capture the diff: gone"}),
        ),
        None,
        None,
    );
    pair(
        &env,
        1,
        1,
        result(
            "error",
            json!("cannot create sandbox x: nope"),
            Value::Null,
            json!({"status": "none"}),
        ),
        None,
        None,
    );
    env
}

fn show(env: &Env, id: &str, full_diff: bool) -> anyhow::Result<String> {
    run_show::render(&env.config_dir(), id, &run_show::Options { full_diff })
}

/// The output with the temp folder replaced, so it can be compared exactly.
fn shown(env: &Env) -> String {
    show(env, ID, false)
        .unwrap()
        .replace(&meta(env).display().to_string(), "<run>")
        // Windows separators, so the expected text is the same everywhere.
        .replace(char::from(92), "/")
}

#[test]
fn a_complete_run_is_printed_with_every_status() {
    let env = fixture();

    let expected = "\
Run 2026-09-30-abc123
Started 2026-09-30T18:00:00Z, completed 2026-09-30T18:01:10Z
Profile default, sbx 0.43.0, 2 contestants, repeat 2

contestants[0] claude/model-a
  repeat 1/2: completed (100 in, 20 out tokens, $0.0100)
    Answer:
      PONG
    Diff: 4 files changed (+3 -2)
      M a.txt (+1 -1)
      A new.txt (+2)
      D old.txt (-1)
      A blob.bin (binary)
    Patch: <run>/0/0/diff.patch
  repeat 2/2: timed out (partial output kept)
    Answer:
      half of the
      answer
    Diff: no changes
    Left out (links): linked

contestants[1] codex/model-b
  repeat 1/2: failed: exit code 1: boom
    Answer: (none)
    Diff: could not be captured: cannot capture the diff: gone
  repeat 2/2: error: cannot create sandbox x: nope
    Answer: (none)
    Diff: none (the pair never got a sandbox)
";
    assert_eq!(shown(&env), expected);
}

#[test]
fn full_diff_prints_every_patch_indented_under_its_summary() {
    let env = fixture();

    let out = show(&env, ID, true).unwrap();

    assert!(
        out.contains("      diff --git a/a.txt b/a.txt\n      index 1111111..2222222 100644\n"),
        "{out}"
    );
    assert!(out.contains("      +alpha edited\n"), "{out}");
    assert!(
        out.contains("      Binary files /dev/null and b/blob.bin differ\n"),
        "{out}"
    );
    // Without the flag no patch lines are printed.
    assert!(!show(&env, ID, false).unwrap().contains("+alpha edited"));
}

#[test]
fn a_run_still_going_or_interrupted_says_so() {
    let env = Env::new();
    run_json(&env, None);
    pair(
        &env,
        0,
        0,
        result(
            "completed",
            Value::Null,
            json!({"input_tokens": 1, "output_tokens": 1, "cost_usd": null}),
            json!({"status": "ok", "skipped": []}),
        ),
        Some("done"),
        Some(""),
    );

    let out = show(&env, ID, false).unwrap();

    assert!(
        out.contains("Started 2026-09-30T18:00:00Z, not completed (still running, or interrupted)"),
        "{out}"
    );
    assert!(
        out.contains("repeat 1/2: completed (1 in, 1 out tokens)"),
        "{out}"
    );
    // Pairs with no result.json yet.
    assert!(
        out.contains("repeat 2/2: no results saved (the run is still going, or was interrupted)"),
        "{out}"
    );
    assert!(
        out.contains("contestants[1] codex/model-b\n  repeat 1/2: no results saved"),
        "{out}"
    );
}

#[test]
fn ids_are_validated_before_anything_on_disk_is_touched() {
    // A config folder that doesn't exist: a filesystem read would fail with a different error.
    let missing = Path::new("E:/no/such/config-dir-for-sbxm-tests");
    for bad in [
        "",
        "..",
        "../2026-09-30-abc123",
        "latest",
        "2026-09-30-ABC123",
        "2026-09-30-abc123/x",
    ] {
        let err = run_show::render(missing, bad, &run_show::Options { full_diff: false })
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("isn't a run id") && err.contains("2026-09-30-a1b2c3"),
            "{bad:?}: {err}"
        );
    }
}

#[test]
fn an_unknown_run_names_where_it_looked() {
    let env = Env::new();

    let err = show(&env, ID, false).unwrap_err().to_string();

    assert!(err.contains(ID) && err.contains(".sbxm"), "{err}");
    assert!(err.contains("sbxm run"), "{err}");
}

#[test]
fn a_folder_without_run_json_is_not_a_run() {
    let env = Env::new();
    std::fs::create_dir_all(meta(&env)).unwrap();

    let err = show(&env, ID, false).unwrap_err().to_string();

    assert!(err.contains("run.json"), "{err}");
}

#[test]
fn a_damaged_run_json_is_reported_not_guessed_at() {
    let env = Env::new();
    write(&meta(&env).join("run.json"), "{ not json");

    let err = format!("{:#}", show(&env, ID, false).unwrap_err());

    assert!(
        err.contains("run.json") && err.contains("not valid JSON"),
        "{err}"
    );
}

#[test]
fn showing_a_run_changes_nothing() {
    let env = fixture();
    let count = || walk(&meta(&env));
    let before = count();

    show(&env, ID, true).unwrap();

    assert_eq!(before, count());
}

fn walk(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            all.extend(walk(&path));
        } else {
            all.push((path, std::fs::metadata(entry.path()).unwrap().len()));
        }
    }
    all.sort();
    all
}

#[test]
fn a_real_run_can_be_shown_right_after_it_finishes() {
    let env = Env::new();
    let backend = FakeBackend::with_secrets(&["anthropic"]).with_default_exec_output(ExecOutput {
        stdout: include_str!("../src/fixtures/claude-stream-json-pong.jsonl").into(),
        stderr: String::new(),
        exit_code: Some(0),
    });
    let config = env.tmp.path().join("run.toml");
    std::fs::write(
        &config,
        "[task]\nprompt = \"p\"\n\n[[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
         [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n",
    )
    .unwrap();
    let summary = run::run(
        &env.config_dir(),
        &config,
        &backend,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();

    let out = show(&env, &summary.run_id, false).unwrap();

    assert!(
        out.starts_with(&format!("Run {}\nStarted ", summary.run_id)),
        "{out}"
    );
    assert!(
        out.contains(
            "contestants[0] claude/one\n  repeat 1/1: completed (27651 in, 44 out tokens, $0.0295)"
        ),
        "{out}"
    );
    assert!(out.contains("Answer:\n      PONG"), "{out}");
    assert!(out.contains("Diff: no changes"), "{out}");
}

// ---- the CLI ---------------------------------------------------------------

fn cli(env: &Env, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sbxm")
        .unwrap()
        .env("SBXM_CONFIG_DIR", env.config_dir())
        .args(args)
        .assert()
}

#[test]
fn the_cli_prints_a_saved_run() {
    let env = fixture();

    let output = cli(&env, &["run", "show", ID])
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("Run 2026-09-30-abc123\n"), "{stdout}");
    assert!(stdout.contains("contestants[1] codex/model-b"), "{stdout}");
    assert!(!stdout.contains("+alpha edited"));
}

#[test]
fn the_cli_diff_flag_prints_the_patches() {
    let env = fixture();

    let output = cli(&env, &["run", "show", ID, "--diff"])
        .success()
        .get_output()
        .clone();

    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("+alpha edited")
    );
}

#[test]
fn the_cli_refuses_a_bad_id_and_a_missing_id() {
    let env = Env::new();

    let bad = cli(&env, &["run", "show", "../x"])
        .failure()
        .get_output()
        .clone();
    assert!(
        String::from_utf8(bad.stderr)
            .unwrap()
            .contains("isn't a run id")
    );

    let none = cli(&env, &["run", "show"]).failure().get_output().clone();
    assert!(String::from_utf8(none.stderr).unwrap().contains("<RUN_ID>"));
}
