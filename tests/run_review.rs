//! Issue 347: `sbxm run review <run-id>` lays a saved run out for a person to score under blind
//! labels of its own, and `--submit` checks `scores.toml` and saves the scores in `evals.json`
//! under `human` (decisions 18, 21, 22).

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use common::Env;
use sbxm::commands::{run_review, run_show};
use sbxm::confirm::Confirm;
use sbxm::eval::{human, judge};
use serde_json::{Value, json};

const ID: &str = "2026-09-30-abc123";

const PATCH: &str =
    "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-alpha\n+beta\n";

const CONFIG: &str = "[task]\nprompt = \"Say PONG\"\n\n[run]\nrepeat = 2\n\n\
     [[contestants]]\nharness = \"claude\"\nmodel = \"model-a\"\n\n\
     [[contestants]]\nharness = \"codex\"\nmodel = \"model-b\"\n\n\
     [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\nnotes = \"It says PONG.\"\n\n\
     [[eval.rubric]]\nid = \"clarity\"\nkind = \"scale\"\nlevels = [\"poor\", \"fair\", \"good\"]\nweight = 0.5\n";

/// Answers every prompt with `answer`, remembering the prompts.
struct FakeConfirm {
    interactive: bool,
    answer: bool,
    prompts: RefCell<Vec<String>>,
}

impl FakeConfirm {
    fn new(interactive: bool, answer: bool) -> Self {
        FakeConfirm {
            interactive,
            answer,
            prompts: RefCell::new(Vec::new()),
        }
    }
}

impl Confirm for FakeConfirm {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn confirm(&self, prompt: &str) -> anyhow::Result<bool> {
        self.prompts.borrow_mut().push(prompt.to_owned());
        Ok(self.answer)
    }
}

fn meta(env: &Env) -> PathBuf {
    env.base_dir().join(".sbxm").join("runs").join(ID)
}

fn review_dir(env: &Env) -> PathBuf {
    meta(env).join("review")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn pair(env: &Env, i: usize, r: u32, status: &str, answer: Option<&str>, patch: Option<&str>) {
    let dir = meta(env).join(i.to_string()).join(r.to_string());
    let diff = if patch.is_some() {
        json!({"status": "ok", "skipped": []})
    } else {
        json!({"status": "none"})
    };
    write(
        &dir.join("result.json"),
        &json!({"status": status, "error": null, "usage": null, "diff": diff}).to_string(),
    );
    if let Some(text) = answer {
        write(&dir.join("answer.md"), text);
    }
    if let Some(text) = patch {
        write(&dir.join("diff.patch"), text);
    }
}

fn run_json(env: &Env, completed: Option<&str>) {
    write(
        &meta(env).join("run.json"),
        &json!({"run_id": ID, "started_at": "2026-09-30T18:00:00Z", "completed_at": completed,
                "sbx_version": "0.43.0", "profile": "default", "repeat": 2,
                "contestants": [
                    {"index": 0, "harness": "claude", "model": "model-a", "profile": "default"},
                    {"index": 1, "harness": "codex", "model": "model-b", "profile": "default"}]})
        .to_string(),
    );
}

/// A completed run, two contestants, two repeats; contestant 1's second repeat couldn't run.
fn fixture() -> Env {
    let env = Env::new();
    run_json(&env, Some("2026-09-30T18:01:10Z"));
    write(&meta(&env).join("run-config.toml"), CONFIG);
    pair(&env, 0, 0, "completed", Some("PONG from A0"), Some(PATCH));
    pair(&env, 0, 1, "completed", Some("PONG from A1"), Some(""));
    pair(&env, 1, 0, "timed_out", Some("half a PONG"), Some(PATCH));
    pair(&env, 1, 1, "error", None, None);
    // The judge's entry stays next to the human one.
    write(
        &meta(&env).join("0").join("0").join("evals.json"),
        &json!({"judge": {"label": "A", "status": "ok"}}).to_string(),
    );
    env
}

fn prepare(env: &Env) -> anyhow::Result<PathBuf> {
    run_review::prepare(
        &env.config_dir(),
        ID,
        false,
        &FakeConfirm::new(false, false),
    )
}

fn submit(env: &Env) -> anyhow::Result<String> {
    run_review::submit(&env.config_dir(), ID)
}

/// The label contestant `c` has in repeat `r` (0-based) of the human review.
fn label_of(c: usize, r: u32) -> char {
    human::assign_labels(ID, r, &[0, 1])
        .into_iter()
        .find(|(contestant, _)| *contestant == c)
        .unwrap()
        .1
}

fn scores_path(env: &Env, r: u32) -> PathBuf {
    review_dir(env)
        .join(format!("repeat-{}", r + 1))
        .join("scores.toml")
}

/// Every file under `dir`, as relative paths with `/`.
fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            out.extend(files(&path).into_iter().map(|f| format!("{name}/{f}")));
        } else {
            out.push(name);
        }
    }
    out.sort();
    out
}

fn evals(env: &Env, c: usize, r: u32) -> Value {
    let path = meta(env)
        .join(c.to_string())
        .join(r.to_string())
        .join("evals.json");
    std::fs::read_to_string(path)
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap_or(Value::Null)
}

/// Writes a filled-in `scores.toml` for repeat `r`: `values[label]` is that label's lines.
fn fill(env: &Env, r: u32, a: &str, b: &str) {
    write(&scores_path(env, r), &format!("[A]\n{a}\n\n[B]\n{b}\n"));
}

/// Both repeats fully scored, with every first and last value.
fn fill_all(env: &Env) {
    fill(
        env,
        0,
        "correct = true\nclarity = \"good\"",
        "correct = false\nclarity = \"poor\"",
    );
    fill(
        env,
        1,
        "correct = true\nclarity = \"fair\"",
        "correct = false\nclarity = \"poor\"",
    );
}

// ---- run review: the folder ------------------------------------------------------------

#[test]
fn review_lays_out_one_folder_and_one_scores_file_per_repeat() {
    let env = fixture();

    let dir = prepare(&env).unwrap();

    assert_eq!(dir, review_dir(&env));
    let mut expected = vec!["task.md".to_owned()];
    for r in 0..2 {
        expected.push(format!("repeat-{}/scores.toml", r + 1));
    }
    let a0 = label_of(0, 0);
    let b0 = label_of(1, 0);
    let a1 = label_of(0, 1);
    let b1 = label_of(1, 1);
    expected.extend([
        format!("repeat-1/{a0}/answer.md"),
        format!("repeat-1/{a0}/diff.patch"),
        format!("repeat-1/{b0}/answer.md"),
        format!("repeat-1/{b0}/diff.patch"),
        format!("repeat-2/{a1}/answer.md"),
        format!("repeat-2/{a1}/diff.patch"),
        format!("repeat-2/{b1}/no-output.md"),
    ]);
    expected.sort();
    assert_eq!(files(&dir), expected);
    assert_eq!(
        read(&dir.join("repeat-1").join(a0.to_string()).join("answer.md")),
        "PONG from A0"
    );
    assert_eq!(
        read(&dir.join("repeat-1").join(a0.to_string()).join("diff.patch")),
        PATCH
    );
    assert_eq!(
        read(&dir.join("repeat-1").join(b0.to_string()).join("answer.md")),
        "half a PONG"
    );
    assert!(read(&dir.join("task.md")).contains("Say PONG"));
}

#[test]
fn a_contestant_that_could_not_run_gets_a_note_and_no_answer_or_diff() {
    let env = fixture();
    let dir = prepare(&env).unwrap();

    let folder = dir.join("repeat-2").join(label_of(1, 1).to_string());
    let note = read(&folder.join("no-output.md"));
    assert!(note.contains("no answer and no diff"), "{note}");
    assert!(!folder.join("answer.md").exists());
    assert!(!folder.join("diff.patch").exists());
}

#[test]
fn the_review_folder_never_names_a_contestant_harness_or_model() {
    let env = fixture();
    let dir = prepare(&env).unwrap();

    for file in files(&dir) {
        let text = read(&dir.join(&file));
        for name in ["claude", "codex", "model-a", "model-b", "contestants["] {
            assert!(!text.contains(name), "{file} names {name}:\n{text}");
        }
    }
}

#[test]
fn scores_toml_lists_every_criterion_per_label_blank_with_its_allowed_values() {
    let env = fixture();
    prepare(&env).unwrap();

    let text = read(&scores_path(&env, 0));
    let parsed: toml::Table = toml::from_str(&text).unwrap();
    for label in ["A", "B"] {
        let table = parsed[label].as_table().unwrap();
        assert_eq!(table["correct"].as_str(), Some(""), "{text}");
        assert_eq!(table["clarity"].as_str(), Some(""), "{text}");
    }
    assert!(text.contains("true or false"), "{text}");
    assert!(text.contains("\"poor\", \"fair\", \"good\""), "{text}");
    assert!(text.contains("It says PONG."), "{text}");
    assert!(
        text.contains(&format!("sbxm run review {ID} --submit")),
        "{text}"
    );
    assert!(text.contains("repeat 1 of 2"), "{text}");
}

#[test]
fn the_labels_are_shuffled_with_their_own_seed_not_the_judges() {
    // The same run, repeat and contestants: the judge's shuffle and the human one disagree.
    let contestants = [0, 1, 2];
    assert_ne!(
        human::assign_labels(ID, 0, &contestants),
        judge::assign_labels(ID, 0, &contestants)
    );
    // And the human labels are reproducible, so --submit finds the same mapping.
    assert_eq!(
        human::assign_labels(ID, 0, &contestants),
        human::assign_labels(ID, 0, &contestants)
    );
}

// ---- run review: refusals --------------------------------------------------------------

#[test]
fn a_run_with_no_rubric_is_refused_and_nothing_is_written() {
    let env = fixture();
    let config = CONFIG.split("[[eval.rubric]]").next().unwrap();
    write(&meta(&env).join("run-config.toml"), config);

    let err = prepare(&env).unwrap_err().to_string();

    assert!(err.contains("no rubric"), "{err}");
    assert!(!review_dir(&env).exists());
}

#[test]
fn a_run_still_going_is_refused_and_nothing_is_written() {
    let env = fixture();
    run_json(&env, None);

    let err = prepare(&env).unwrap_err().to_string();

    assert!(err.contains("not completed"), "{err}");
    assert!(!review_dir(&env).exists());
}

#[test]
fn a_bad_or_unknown_run_id_is_refused() {
    let env = fixture();
    let bad = run_review::prepare(
        &env.config_dir(),
        "../x",
        false,
        &FakeConfirm::new(false, false),
    )
    .unwrap_err()
    .to_string();
    assert!(bad.contains("isn't a run id"), "{bad}");
    let missing = run_review::prepare(
        &env.config_dir(),
        "2026-09-30-ffffff",
        false,
        &FakeConfirm::new(false, false),
    )
    .unwrap_err()
    .to_string();
    assert!(missing.contains("no run 2026-09-30-ffffff"), "{missing}");
}

#[test]
fn a_second_review_refuses_without_restart_and_keeps_the_scores_file() {
    let env = fixture();
    prepare(&env).unwrap();
    write(&scores_path(&env, 0), "[A]\ncorrect = true\n");

    let err = prepare(&env).unwrap_err().to_string();

    assert!(err.contains("--restart"), "{err}");
    assert!(err.contains("--submit"), "{err}");
    assert_eq!(read(&scores_path(&env, 0)), "[A]\ncorrect = true\n");
}

#[test]
fn restart_asks_with_the_path_and_rewrites_the_folder() {
    let env = fixture();
    prepare(&env).unwrap();
    write(&scores_path(&env, 0), "[A]\ncorrect = true\n");
    let confirm = FakeConfirm::new(true, true);

    run_review::prepare(&env.config_dir(), ID, true, &confirm).unwrap();

    let prompts = confirm.prompts.borrow();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains(&review_dir(&env).display().to_string()),
        "{}",
        prompts[0]
    );
    assert!(read(&scores_path(&env, 0)).contains("correct = \"\""));
}

#[test]
fn restart_answered_no_or_without_a_terminal_changes_nothing() {
    let env = fixture();
    prepare(&env).unwrap();
    write(&scores_path(&env, 0), "[A]\ncorrect = true\n");

    let no = run_review::prepare(&env.config_dir(), ID, true, &FakeConfirm::new(true, false))
        .unwrap_err()
        .to_string();
    assert!(no.contains("nothing was changed"), "{no}");
    let headless = run_review::prepare(&env.config_dir(), ID, true, &FakeConfirm::new(false, true))
        .unwrap_err()
        .to_string();
    assert!(headless.contains("terminal"), "{headless}");

    assert_eq!(read(&scores_path(&env, 0)), "[A]\ncorrect = true\n");
}

#[test]
fn restart_refuses_before_asking_when_the_run_has_no_rubric_any_more() {
    let env = fixture();
    prepare(&env).unwrap();
    write(&scores_path(&env, 0), "[A]\ncorrect = true\n");
    let config = CONFIG.split("[[eval.rubric]]").next().unwrap();
    write(&meta(&env).join("run-config.toml"), config);
    let confirm = FakeConfirm::new(true, true);

    assert!(run_review::prepare(&env.config_dir(), ID, true, &confirm).is_err());

    assert!(confirm.prompts.borrow().is_empty());
    assert_eq!(read(&scores_path(&env, 0)), "[A]\ncorrect = true\n");
}

#[test]
fn restart_refuses_a_review_folder_that_is_a_link() {
    let env = fixture();
    let elsewhere = env.tmp.path().join("elsewhere");
    write(&elsewhere.join("keep.txt"), "mine");
    common::dir_link(&review_dir(&env), &elsewhere);

    let err = run_review::prepare(&env.config_dir(), ID, true, &FakeConfirm::new(true, true))
        .unwrap_err()
        .to_string();

    assert!(err.contains("link"), "{err}");
    assert_eq!(read(&elsewhere.join("keep.txt")), "mine");
}

// ---- --submit --------------------------------------------------------------------------

#[test]
fn submit_without_a_review_folder_names_the_command_to_run_first() {
    let env = fixture();

    let err = submit(&env).unwrap_err().to_string();

    assert!(err.contains(&format!("sbxm run review {ID}")), "{err}");
    assert_eq!(evals(&env, 1, 0), Value::Null);
}

#[test]
fn submit_saves_every_pairs_scores_under_human_keeping_the_other_keys() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);

    submit(&env).unwrap();

    // Repeat 1: A got the best values, B the worst.
    let first = human::assign_labels(ID, 0, &[0, 1]);
    let (best, worst) = (first[0].0, first[1].0);
    let saved = evals(&env, best, 0);
    assert_eq!(saved["human"]["label"], "A");
    assert_eq!(saved["human"]["criteria"]["correct"]["value"], true);
    assert_eq!(saved["human"]["criteria"]["correct"]["score"], 1.0);
    assert_eq!(saved["human"]["criteria"]["clarity"]["value"], "good");
    assert_eq!(saved["human"]["criteria"]["clarity"]["score"], 1.0);
    assert_eq!(saved["human"]["unscored"], json!([]));
    let low = evals(&env, worst, 0);
    assert_eq!(low["human"]["label"], "B");
    assert_eq!(low["human"]["criteria"]["correct"]["score"], 0.0);
    assert_eq!(low["human"]["criteria"]["clarity"]["value"], "poor");
    assert_eq!(low["human"]["criteria"]["clarity"]["score"], 0.0);
    // The judge's entry is still there.
    assert_eq!(evals(&env, 0, 0)["judge"]["label"], "A");
}

#[test]
fn submit_reveals_the_labels_and_prints_the_human_ranking_with_real_names() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);

    let out = submit(&env).unwrap();

    assert!(out.contains("Saved"), "{out}");
    let first = human::assign_labels(ID, 0, &[0, 1]);
    let names = ["claude (model-a)", "codex (model-b)"];
    assert!(
        out.contains(&format!(
            "A = contestants[{}] {}",
            first[0].0, names[first[0].0]
        )),
        "{out}"
    );
    assert!(out.contains("Human ranking"), "{out}");
    assert!(out.contains("contestants[0] claude (model-a):"), "{out}");
    assert!(out.contains("contestants[1] codex (model-b):"), "{out}");
    assert!(!out.contains("Replaced"), "{out}");
}

#[test]
fn a_scale_value_is_matched_to_its_level_ignoring_case() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    fill(&env, 0, "clarity = \" Good \"", "correct = true");

    submit(&env).unwrap();

    let a = human::assign_labels(ID, 0, &[0, 1])[0].0;
    assert_eq!(
        evals(&env, a, 0)["human"]["criteria"]["clarity"]["value"],
        "good"
    );
}

/// `--submit` on this repeat-1 file fails naming `line`, and nothing is saved.
fn refused_at_line(a: &str, b: &str, line: usize, says: &str) {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    fill(&env, 0, a, b);
    let before: Vec<Value> = (0..2)
        .flat_map(|c| (0..2).map(move |r| (c, r)))
        .map(|(c, r)| evals(&env, c, r))
        .collect();

    let err = submit(&env).unwrap_err().to_string();

    assert!(err.contains(&format!("line {line}")), "{err}");
    assert!(err.contains(says), "{err}");
    assert!(err.contains("scores.toml"), "{err}");
    let after: Vec<Value> = (0..2)
        .flat_map(|c| (0..2).map(move |r| (c, r)))
        .map(|(c, r)| evals(&env, c, r))
        .collect();
    assert_eq!(before, after);
}

#[test]
fn a_scale_value_outside_its_levels_is_refused_naming_the_line() {
    refused_at_line(
        "correct = true\nclarity = \"great\"",
        "correct = false",
        3,
        "\"poor\", \"fair\", \"good\"",
    );
}

#[test]
fn a_pass_fail_that_is_not_true_or_false_is_refused_naming_the_line() {
    refused_at_line("correct = \"yes\"", "correct = false", 2, "true or false");
}

#[test]
fn an_unknown_label_is_refused_naming_the_line() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    write(
        &scores_path(&env, 0),
        "[A]\ncorrect = true\n\n[C]\ncorrect = false\n",
    );

    let err = submit(&env).unwrap_err().to_string();

    assert!(err.contains("line 4"), "{err}");
    assert!(err.contains("C"), "{err}");
    assert_eq!(evals(&env, 1, 0), Value::Null);
}

#[test]
fn an_unknown_criterion_is_refused_naming_the_line() {
    refused_at_line("correct = true", "speed = \"fast\"", 5, "speed");
}

#[test]
fn a_toml_syntax_error_is_refused_naming_the_line() {
    refused_at_line("correct = true", "correct = ", 5, "");
}

#[test]
fn a_blank_criterion_is_saved_as_unscored_and_listed() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    fill(
        &env,
        0,
        "correct = true\nclarity = \"\"",
        "correct = false\nclarity = \"poor\"",
    );

    let out = submit(&env).unwrap();

    let a = human::assign_labels(ID, 0, &[0, 1])[0].0;
    let saved = evals(&env, a, 0);
    assert_eq!(saved["human"]["unscored"], json!(["clarity"]));
    assert!(saved["human"]["criteria"].get("clarity").is_none());
    assert!(out.contains("repeat 1/2 A: clarity"), "{out}");
}

#[test]
fn a_missing_label_counts_as_blank() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    // Repeat 2's B couldn't run: its table is removed altogether.
    write(
        &scores_path(&env, 1),
        "[A]\ncorrect = true\nclarity = \"good\"\n",
    );

    let out = submit(&env).unwrap();

    let b = human::assign_labels(ID, 1, &[0, 1])[1].0;
    assert_eq!(
        evals(&env, b, 1)["human"]["unscored"],
        json!(["correct", "clarity"])
    );
    assert!(out.contains("repeat 2/2 B: correct, clarity"), "{out}");
}

#[test]
fn a_file_with_no_scores_at_all_is_refused() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    std::fs::remove_file(scores_path(&env, 1)).unwrap();
    prepare_blank_repeat_2(&env);

    let err = submit(&env).unwrap_err().to_string();

    assert!(err.contains("no criterion is scored"), "{err}");
    assert!(err.contains("repeat-2"), "{err}");
    assert_eq!(evals(&env, 1, 0), Value::Null);
}

/// Puts back repeat 2's untouched `scores.toml`, every value blank.
fn prepare_blank_repeat_2(env: &Env) {
    write(
        &scores_path(env, 1),
        "[A]\ncorrect = \"\"\nclarity = \"\"\n\n[B]\ncorrect = \"\"\nclarity = \"\"\n",
    );
}

#[test]
fn a_second_submit_overwrites_and_says_so() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    submit(&env).unwrap();
    fill(
        &env,
        0,
        "correct = false\nclarity = \"poor\"",
        "correct = false\nclarity = \"poor\"",
    );

    let out = submit(&env).unwrap();

    assert!(
        out.contains("Replaced the human scores saved earlier"),
        "{out}"
    );
    let a = human::assign_labels(ID, 0, &[0, 1])[0].0;
    assert_eq!(
        evals(&env, a, 0)["human"]["criteria"]["correct"]["value"],
        false
    );
    // The file stays for a later sitting.
    assert!(scores_path(&env, 0).is_file());
}

#[test]
fn submit_refuses_a_run_that_lost_its_rubric() {
    let env = fixture();
    prepare(&env).unwrap();
    fill_all(&env);
    let config = CONFIG.split("[[eval.rubric]]").next().unwrap();
    write(&meta(&env).join("run-config.toml"), config);

    assert!(submit(&env).unwrap_err().to_string().contains("no rubric"));
    assert_eq!(evals(&env, 1, 0), Value::Null);
}

// ---- the ranking in run show -----------------------------------------------------------

#[test]
fn run_show_prints_the_human_ranking_once_scores_are_saved() {
    let env = fixture();
    let show = || run_show::render(&env.config_dir(), ID, &run_show::Options::default()).unwrap();
    assert!(!show().contains("Human ranking"));

    prepare(&env).unwrap();
    // Contestant 0 gets the best values in both repeats, whatever its label.
    for r in 0..2 {
        write(
            &scores_path(&env, r),
            &format!(
                "[{}]\ncorrect = true\nclarity = \"good\"\n\n[{}]\ncorrect = false\n",
                label_of(0, r),
                label_of(1, r)
            ),
        );
    }
    submit(&env).unwrap();

    let out = show();
    assert!(out.contains("Human ranking"), "{out}");
    assert!(
        out.contains("  1. contestants[0] claude (model-a): 1.00"),
        "{out}"
    );
    assert!(
        out.contains("  2. contestants[1] codex (model-b): 0.00"),
        "{out}"
    );
    assert!(out.contains("unscored: clarity"), "{out}");
}

// ---- the command line ------------------------------------------------------------------

#[test]
fn the_cli_writes_the_folder_then_submits_it() {
    let env = fixture();
    let sbxm = || {
        let mut cmd = assert_cmd::Command::cargo_bin("sbxm").unwrap();
        cmd.env("SBXM_CONFIG_DIR", env.config_dir());
        cmd
    };

    let written = sbxm().args(["run", "review", ID]).assert().success();
    let out = String::from_utf8(written.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains(&review_dir(&env).display().to_string()),
        "{out}"
    );
    assert!(
        out.contains(&format!("sbxm run review {ID} --submit")),
        "{out}"
    );

    fill_all(&env);
    let submitted = sbxm()
        .args(["run", "review", ID, "--submit"])
        .assert()
        .success();
    let out = String::from_utf8(submitted.get_output().stdout.clone()).unwrap();
    assert!(out.contains("Human ranking"), "{out}");

    sbxm()
        .args(["run", "review", ID, "--submit", "--restart"])
        .assert()
        .failure();
}
