//! The Comparison runs section of `sbxm status` (decision 171, issue 91): rows read from run
//! fixtures on disk, the one `sbx ls` for gone sandboxes, and the boundary cases of the issue.

use std::fs;
use std::path::Path;

use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::status::runs::{self, Gone, RunState, Runs};

const START: &str = "2026-10-30T10:00:00Z";
const START_SECS: u64 = 1_793_354_400;

fn runs_dir(base: &Path) -> std::path::PathBuf {
    base.join(".sbxm").join("runs")
}

/// A run folder with `run.json` for two contestants and `repeat` repeats.
fn run(base: &Path, id: &str, repeat: u32, completed_at: Option<&str>) -> std::path::PathBuf {
    let dir = runs_dir(base).join(id);
    fs::create_dir_all(&dir).unwrap();
    let json = serde_json::json!({
        "run_id": id,
        "started_at": START,
        "completed_at": completed_at,
        "repeat": repeat,
        "contestants": [
            {"index": 0, "harness": "claude", "model": "opus"},
            {"index": 1, "harness": "codex", "model": "gpt"},
        ],
    });
    fs::write(dir.join("run.json"), json.to_string()).unwrap();
    dir
}

fn save_pair(dir: &Path, contestant: u32, repeat: u32) {
    let pair = dir.join(contestant.to_string()).join(repeat.to_string());
    fs::create_dir_all(&pair).unwrap();
    fs::write(pair.join("result.json"), "{}").unwrap();
}

fn sandbox(name: &str) -> SandboxInfo {
    SandboxInfo {
        name: name.to_owned(),
        agent: String::new(),
        status: "running".to_owned(),
    }
}

fn only_row(runs: Runs) -> runs::RunRow {
    let Runs::Rows(mut rows) = runs else {
        panic!("runs unreadable");
    };
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn info(row: &runs::RunRow) -> &runs::RunInfo {
    match &row.state {
        RunState::Run(info) => info,
        other => panic!("not a run: {other:?}"),
    }
}

#[test]
fn no_runs_folder_says_there_are_no_runs() {
    let base = tempfile::tempdir().unwrap();
    let found = runs::read(base.path(), &FakeBackend::default());
    assert_eq!(found, Runs::Rows(vec![]));
    assert_eq!(runs::render(&found, 0, None), "\nNo comparison runs.\n");
}

#[test]
fn an_empty_runs_folder_says_there_are_no_runs() {
    let base = tempfile::tempdir().unwrap();
    fs::create_dir_all(runs_dir(base.path())).unwrap();
    let found = runs::read(base.path(), &FakeBackend::default());
    assert_eq!(runs::render(&found, 0, None), "\nNo comparison runs.\n");
}

#[test]
fn zero_of_n_pairs_saved() {
    let base = tempfile::tempdir().unwrap();
    run(base.path(), "r1", 2, None);
    let backend =
        FakeBackend::with_sandboxes(vec![sandbox("sbxm-run-r1-0-0"), sandbox("sbxm-run-r1-1-0")]);
    let row = only_row(runs::read(base.path(), &backend));
    let i = info(&row);
    assert_eq!((i.done, i.total), (0, 4));
    assert_eq!(i.gone, Gone::No);
    assert_eq!(i.completed_at, None);
}

#[test]
fn every_pair_saved_without_completed_at_is_shown_unfinished() {
    let base = tempfile::tempdir().unwrap();
    let dir = run(base.path(), "r1", 1, None);
    save_pair(&dir, 0, 0);
    save_pair(&dir, 1, 0);
    let row = only_row(runs::read(base.path(), &FakeBackend::default()));
    let i = info(&row);
    assert_eq!((i.done, i.total), (2, 2));
    assert_eq!(i.completed_at, None);
    assert_eq!(i.gone, Gone::No);
    let text = runs::render(&Runs::Rows(vec![row]), START_SECS + 90, None);
    assert!(text.contains("2/2"), "{text}");
    assert!(text.contains("1m"), "{text}");
}

#[test]
fn a_completed_run_shows_completed_at_and_its_duration() {
    let base = tempfile::tempdir().unwrap();
    let dir = run(base.path(), "r1", 1, Some("2026-10-30T10:12:00Z"));
    save_pair(&dir, 0, 0);
    save_pair(&dir, 1, 0);
    fs::write(dir.join("run.log"), "2026-10-30T10:12:00Z [results] done\n").unwrap();
    let found = runs::read(base.path(), &FakeBackend::default());
    let text = runs::render(&found, START_SECS + 999_999, None);
    insta::assert_snapshot!(text, @r"

    RUN  CONTESTANTS             PAIRS  TIME  COMPLETED             FLAG  LOG
    r1   claude:opus, codex:gpt  2/2    12m   2026-10-30T10:12:00Z        [results] done
    ");
}

#[test]
fn a_pair_of_the_running_wave_without_a_sandbox_is_flagged() {
    let base = tempfile::tempdir().unwrap();
    let dir = run(base.path(), "r1", 2, None);
    save_pair(&dir, 0, 0); // finished; its sandbox is removed, which is fine
    let backend = FakeBackend::with_sandboxes(vec![sandbox("sbxm-run-r1-1-0")]);
    let row = only_row(runs::read(base.path(), &backend));
    assert_eq!(info(&row).gone, Gone::No);

    let backend = FakeBackend::default();
    let row = only_row(runs::read(base.path(), &backend));
    // Pair (1, 0) is the only unsaved pair of wave 0; wave 1 hasn't started.
    assert_eq!(info(&row).gone, Gone::Pairs(1));
    let text = runs::render(&Runs::Rows(vec![row]), START_SECS, None);
    assert!(text.contains("sandbox gone"), "{text}");
}

#[test]
fn a_completed_run_is_never_flagged() {
    let base = tempfile::tempdir().unwrap();
    run(base.path(), "r1", 1, Some("2026-10-30T10:12:00Z"));
    let row = only_row(runs::read(base.path(), &FakeBackend::default()));
    assert_eq!(info(&row).gone, Gone::No);
}

#[test]
fn a_failing_sbx_ls_shows_unknown_and_the_rest_still_shows() {
    let base = tempfile::tempdir().unwrap();
    run(base.path(), "r1", 1, None);
    let found = runs::read(base.path(), &FakeBackend::default().failing_list());
    let row = only_row(found.clone());
    assert_eq!(info(&row).gone, Gone::Unknown);
    let text = runs::render(&found, START_SECS, None);
    assert!(text.contains("sandbox unknown"), "{text}");
    assert!(text.contains("claude:opus, codex:gpt"), "{text}");
}

#[test]
fn sbx_ls_is_not_asked_when_no_run_has_unfinished_pairs() {
    let base = tempfile::tempdir().unwrap();
    run(base.path(), "r1", 1, Some("2026-10-30T10:12:00Z"));
    let found = runs::read_with(base.path(), &|| panic!("sbx ls was called"));
    assert!(matches!(found, Runs::Rows(rows) if rows.len() == 1));
}

#[test]
fn malformed_and_half_written_folders_are_rows_not_crashes() {
    let base = tempfile::tempdir().unwrap();
    let root = runs_dir(base.path());
    fs::create_dir_all(root.join("a-empty")).unwrap();
    fs::create_dir_all(root.join("b-garbage")).unwrap();
    fs::write(root.join("b-garbage").join("run.json"), "{ not json").unwrap();
    let c = root.join("c-nocontestants");
    fs::create_dir_all(&c).unwrap();
    fs::write(c.join("run.json"), r#"{"repeat": 1, "contestants": []}"#).unwrap();
    let d = root.join("d-hugerepeat");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("run.json"),
        r#"{"repeat": 4000000000, "contestants": [{"harness":"claude","model":"m"}]}"#,
    )
    .unwrap();
    let e = root.join("e-norepeat");
    fs::create_dir_all(&e).unwrap();
    fs::write(e.join("run.json"), r#"{"contestants": [{}]}"#).unwrap();
    fs::write(root.join("stray-file"), "x").unwrap();
    // A run.json that is a folder cannot be read as a file.
    fs::create_dir_all(root.join("f-dir").join("run.json")).unwrap();

    let found = runs::read(base.path(), &FakeBackend::default());
    let Runs::Rows(rows) = &found else { panic!() };
    assert_eq!(rows.len(), 6);
    for row in rows {
        assert!(matches!(row.state, RunState::Malformed(_)), "{row:?}");
    }
    let text = runs::render(&found, 0, None);
    assert!(text.contains("malformed: no run.json"), "{text}");
    assert!(
        text.contains("malformed: run.json is not valid JSON"),
        "{text}"
    );
}

#[test]
fn a_run_next_to_a_malformed_one_still_shows() {
    let base = tempfile::tempdir().unwrap();
    run(base.path(), "good", 1, None);
    fs::create_dir_all(runs_dir(base.path()).join("bad")).unwrap();
    let found = runs::read(base.path(), &FakeBackend::default());
    let text = runs::render(&found, START_SECS, None);
    assert!(text.contains("good"), "{text}");
    assert!(text.contains("bad"), "{text}");
}

#[test]
fn the_runs_path_being_a_file_is_one_line_not_an_error() {
    let base = tempfile::tempdir().unwrap();
    fs::create_dir_all(base.path().join(".sbxm")).unwrap();
    fs::write(runs_dir(base.path()), "x").unwrap();
    let found = runs::read(base.path(), &FakeBackend::default());
    assert!(matches!(found, Runs::Unreadable(_)));
    assert!(runs::render(&found, 0, None).contains("Comparison runs couldn't be read"));
}

#[test]
fn the_last_log_line_is_shown_without_its_timestamp() {
    let base = tempfile::tempdir().unwrap();
    let dir = run(base.path(), "r1", 1, None);
    fs::write(
        dir.join("run.log"),
        "# header\n2026-10-30T10:01:00Z [pairs] first\n2026-10-30T10:02:00Z [pairs] second\n",
    )
    .unwrap();
    let row = only_row(runs::read(base.path(), &FakeBackend::default()));
    assert_eq!(info(&row).log_line.as_deref(), Some("[pairs] second"));
}
