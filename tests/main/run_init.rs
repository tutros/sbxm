//! M2a slice 5: `sbxm run init [path]` writes a starter run-config that is
//! valid as written (P1, decisions 96, 43), and refuses to overwrite.

use std::path::Path;

use assert_cmd::Command;
use sbxm::harness::Harness;
use sbxm::run::config::RunConfig;
use tempfile::TempDir;

fn run_init(dir: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sbxm")
        .unwrap()
        .current_dir(dir)
        // The command must not need (or touch) the real config.
        .env("SBXM_CONFIG_DIR", dir.join("no-such-config"))
        .args(["run", "init"])
        .args(args)
        .assert()
}

#[test]
fn writes_run_toml_in_the_working_directory_by_default() {
    let tmp = TempDir::new().unwrap();

    let output = run_init(tmp.path(), &[]).success().get_output().clone();

    assert!(tmp.path().join("run.toml").is_file());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("run.toml"), "{stdout}");
    assert!(stdout.contains("sbxm run"), "next step is named: {stdout}");
}

#[test]
fn writes_to_the_given_path_creating_its_folder() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("comparisons").join("first.toml");

    run_init(tmp.path(), &[path.to_str().unwrap()]).success();

    assert!(path.is_file());
}

#[test]
fn the_starter_is_valid_as_written_with_two_contestants() {
    let tmp = TempDir::new().unwrap();
    run_init(tmp.path(), &[]).success();

    let config = RunConfig::load(&tmp.path().join("run.toml")).unwrap();

    let harnesses: Vec<_> = config.contestants.iter().map(|c| c.harness).collect();
    assert_eq!(harnesses, [Harness::Claude, Harness::Codex]);
    assert!(!config.task.prompt.trim().is_empty());
    assert_eq!(config.run.repeat, 1);
    // Nothing is scored until the user opts in.
    assert!(config.eval.judge.is_none() && config.eval.rubric.is_empty());
}

#[test]
fn the_third_contestant_is_commented_out_and_explained() {
    let tmp = TempDir::new().unwrap();
    run_init(tmp.path(), &[]).success();
    let text = std::fs::read_to_string(tmp.path().join("run.toml")).unwrap();

    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == "# [[contestants]]")
        .expect("a commented [[contestants]] block");
    assert!(text.contains("antigravity"), "{text}");
    assert!(text.contains("sbx secret"), "says what it needs: {text}");

    // Removing the leading "# " from that block makes it a third, valid contestant.
    let block: Vec<String> = lines[start..start + 3]
        .iter()
        .map(|l| l.strip_prefix("# ").unwrap_or(l).to_owned())
        .collect();
    let enabled = format!("{text}\n{}\n", block.join("\n"));
    let path = tmp.path().join("three.toml");
    std::fs::write(&path, enabled).unwrap();

    let config = RunConfig::load(&path).unwrap();
    assert_eq!(config.contestants.len(), 3);
    assert_eq!(config.contestants[2].harness, Harness::Antigravity);
}

#[test]
fn a_second_run_refuses_and_leaves_the_file_unchanged() {
    let tmp = TempDir::new().unwrap();
    run_init(tmp.path(), &[]).success();
    let path = tmp.path().join("run.toml");
    std::fs::write(&path, "# my edits\n").unwrap();

    let output = run_init(tmp.path(), &[]).failure().get_output().clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already exists"), "{stderr}");
    assert!(stderr.contains("run.toml"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# my edits\n");
}

#[test]
fn a_folder_at_the_path_is_refused_too() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("run.toml")).unwrap();

    let output = run_init(tmp.path(), &[]).failure().get_output().clone();

    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("already exists")
    );
}

#[test]
fn the_commented_scoring_examples_are_valid_when_switched_on() {
    let tmp = TempDir::new().unwrap();
    run_init(tmp.path(), &[]).success();
    let text = std::fs::read_to_string(tmp.path().join("run.toml")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == "# [[eval.checks]]")
        .expect("commented eval examples");

    // Keep only the TOML lines of the examples, not their explaining sentences.
    let enabled: Vec<&str> = lines[start..]
        .iter()
        .filter_map(|l| l.strip_prefix("# "))
        .filter(|l| l.starts_with('[') || l.contains(" = "))
        .collect();
    let path = tmp.path().join("scored.toml");
    std::fs::write(&path, format!("{text}\n{}\n", enabled.join("\n"))).unwrap();

    let config = RunConfig::load(&path).unwrap();
    assert_eq!(config.eval.checks.len(), 1);
    assert_eq!(config.eval.rubric.len(), 2);
    assert!(config.eval.judge.is_some());
}
