//! M2b slice 4: `task.json` (spec §3): stage transitions, atomic writes, the schema check
//! and `interrupted`, with the process probe faked.

use std::fs;

use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, SCHEMA, Stage, Status, is_valid_id,
};

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

const T0: u64 = 1_790_000_000; // 2026-09-21T14:13:20Z

fn new_record() -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 41,
            repo: "o/r",
            title: "Fix it",
            base: "main",
            branch: "issue-41",
            config_hash: "abc123",
        },
        T0,
        Process::new(1234, T0),
    )
}

fn new_pr_record() -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Pr,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "pr-7",
            config_hash: "abc123",
        },
        T0,
        Process::new(1234, T0),
    )
}

#[test]
fn a_new_record_is_prepared_and_running() {
    let record = new_record();
    assert_eq!(record.schema, SCHEMA);
    assert_eq!((record.id.as_str(), record.number), ("issue-41", 41));
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Running)
    );
    assert_eq!(record.stages.len(), 1);
    assert_eq!(record.stages[0].stage, Stage::Prepared);
    assert_eq!(record.stages[0].at, "2026-09-21T14:13:20Z");
    assert_eq!(record.process.as_ref().unwrap().pid, 1234);
    assert!(!record.fix_round);
    assert!(record.gates.is_empty() && record.related.is_empty());
}

#[test]
fn a_pr_record_gets_a_pr_id() {
    let record = Record::new(
        &NewTask {
            kind: Kind::Pr,
            number: 7,
            repo: "o/r",
            title: "t",
            base: "main",
            branch: "feature",
            config_hash: "h",
        },
        T0,
        Process::new(1, T0),
    );
    assert_eq!(record.id, "pr-7");
}

#[test]
fn the_whole_pipeline_is_a_legal_walk_with_every_stage_stamped() {
    let mut record = new_record();
    let process = || Process::new(1234, T0);
    let steps = [
        (Stage::Working, Status::Completed),
        (Stage::Gating, Status::Passed),
        (Stage::Reviewing, Status::Completed),
        (Stage::Fixing, Status::Completed),
        (Stage::Gating, Status::Passed),
        (Stage::Reviewing, Status::Completed),
    ];
    for (i, (stage, done)) in steps.into_iter().enumerate() {
        record.advance(stage, T0 + 1 + i as u64, process()).unwrap();
        assert_eq!((record.stage, record.status), (stage, Status::Running));
        record.finish(done).unwrap();
    }
    record.advance(Stage::Ready, T0 + 10, process()).unwrap();
    assert_eq!(record.status, Status::Ok);
    record.advance(Stage::Finished, T0 + 11, process()).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.stages.len(), 9);
}

#[test]
fn a_pr_task_goes_straight_from_prepared_to_reviewing() {
    let mut record = new_pr_record();
    record
        .advance(Stage::Reviewing, T0 + 1, Process::new(1, T0))
        .unwrap();
    assert_eq!(record.stage, Stage::Reviewing);
}

#[test]
fn an_issue_task_cannot_skip_its_worker() {
    let mut record = new_record();
    let message = format!(
        "{:#}",
        record
            .advance(Stage::Gating, T0, Process::new(1, T0))
            .unwrap_err()
    );
    assert!(
        message.contains("prepared") && message.contains("gating"),
        "{message}"
    );
    assert!(
        record
            .advance(Stage::Reviewing, T0, Process::new(1, T0))
            .is_err()
    );
}

#[test]
fn a_pr_task_cannot_get_a_worker_a_fix_round_or_a_finish() {
    let mut record = new_pr_record();
    assert!(
        record
            .advance(Stage::Working, T0, Process::new(1, T0))
            .is_err()
    );
    record
        .advance(Stage::Reviewing, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    assert!(
        record
            .advance(Stage::Fixing, T0, Process::new(1, T0))
            .is_err()
    );
    record
        .advance(Stage::Ready, T0, Process::new(1, T0))
        .unwrap();
    assert!(
        record
            .advance(Stage::Finished, T0, Process::new(1, T0))
            .is_err()
    );
}

#[test]
fn an_illegal_jump_is_refused_and_changes_nothing() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    let before = record.clone();

    let message = format!(
        "{:#}",
        record
            .advance(Stage::Finished, T0, Process::new(1, T0))
            .unwrap_err()
    );

    assert!(
        message.contains("working") && message.contains("finished"),
        "{message}"
    );
    assert_eq!(record, before);
}

#[test]
fn a_stage_that_is_still_running_cannot_be_left() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    let message = format!(
        "{:#}",
        record
            .advance(Stage::Gating, T0, Process::new(1, T0))
            .unwrap_err()
    );
    assert!(message.contains("running"), "{message}");
}

#[test]
fn a_timed_out_worker_still_goes_on_to_gates() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::TimedOut).unwrap();
    record
        .advance(Stage::Gating, T0, Process::new(1, T0))
        .unwrap();
}

#[test]
fn failed_gates_stop_the_task() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record
        .advance(Stage::Gating, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::GatesFailed).unwrap();
    assert!(
        record
            .advance(Stage::Reviewing, T0, Process::new(1, T0))
            .is_err()
    );
}

#[test]
fn gating_can_begin_after_the_worker_and_again_after_a_pass_or_a_failure() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record.begin_gating(T0 + 1, Process::new(1, T0)).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Running)
    );
    record.finish(Status::GatesFailed).unwrap();

    // The user fixed something by hand and runs the gates again.
    record.begin_gating(T0 + 2, Process::new(1, T0)).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Running)
    );
    record.finish(Status::Passed).unwrap();
    record.begin_gating(T0 + 3, Process::new(1, T0)).unwrap();
    assert_eq!(
        record
            .stages
            .iter()
            .filter(|s| s.stage == Stage::Gating)
            .count(),
        3
    );
}

#[test]
fn gating_cannot_begin_while_gates_are_running_or_from_the_wrong_stage() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    // Still running the worker.
    assert!(record.begin_gating(T0, Process::new(1, T0)).is_err());
    record.finish(Status::Completed).unwrap();
    record.begin_gating(T0, Process::new(1, T0)).unwrap();
    // Gates are running now.
    let message = format!(
        "{:#}",
        record.begin_gating(T0, Process::new(1, T0)).unwrap_err()
    );
    assert!(message.contains("running"), "{message}");

    let mut reviewing = new_record();
    reviewing
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    reviewing.finish(Status::Completed).unwrap();
    reviewing
        .advance(Stage::Gating, T0, Process::new(1, T0))
        .unwrap();
    reviewing.finish(Status::Passed).unwrap();
    reviewing
        .advance(Stage::Reviewing, T0, Process::new(1, T0))
        .unwrap();
    reviewing.finish(Status::Completed).unwrap();
    assert!(reviewing.begin_gating(T0, Process::new(1, T0)).is_err());
}

#[test]
fn gates_whose_process_is_gone_are_given_up_as_failed_so_they_can_run_again() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1234, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record.begin_gating(T0, Process::new(1234, T0)).unwrap();

    // The process is still there: nothing changes.
    assert!(!record.abandon_interrupted_gates(&Probe(Some(T0))));
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::Running)
    );

    // The process is gone (a crash, Ctrl-C): the gates did not finish, so they count as failed.
    assert!(record.abandon_interrupted_gates(&Probe(None)));
    assert_eq!(
        (record.stage, record.status),
        (Stage::Gating, Status::GatesFailed)
    );
    record.begin_gating(T0 + 1, Process::new(1234, T0)).unwrap();
}

#[test]
fn only_running_gates_can_be_given_up() {
    let mut record = new_record();
    // Not in the gating stage at all, even though its process is gone.
    assert!(!record.abandon_interrupted_gates(&Probe(None)));
    assert_eq!(
        (record.stage, record.status),
        (Stage::Prepared, Status::Running)
    );

    record
        .advance(Stage::Working, T0, Process::new(1234, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record.begin_gating(T0, Process::new(1234, T0)).unwrap();
    record.finish(Status::Passed).unwrap();
    assert!(!record.abandon_interrupted_gates(&Probe(None)));
    assert_eq!(record.status, Status::Passed);
}

#[test]
fn a_review_can_begin_after_the_gates_and_again_after_a_failed_review() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record.begin_gating(T0, Process::new(1, T0)).unwrap();
    record.finish(Status::Passed).unwrap();

    record.begin_review(T0 + 1, Process::new(1, T0)).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Running)
    );
    record.finish(Status::Failed).unwrap();

    // The reviewer failed: the same round is tried again.
    record.begin_review(T0 + 2, Process::new(1, T0)).unwrap();
    assert_eq!(
        (record.stage, record.status),
        (Stage::Reviewing, Status::Running)
    );
    assert_eq!(
        record
            .stages
            .iter()
            .filter(|s| s.stage == Stage::Reviewing)
            .count(),
        2
    );
}

#[test]
fn a_review_cannot_begin_while_running_or_once_it_is_complete() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    record
        .advance(Stage::Gating, T0, Process::new(1, T0))
        .unwrap();
    record.finish(Status::Passed).unwrap();
    record
        .advance(Stage::Reviewing, T0, Process::new(1, T0))
        .unwrap();
    let message = format!(
        "{:#}",
        record.begin_review(T0, Process::new(1, T0)).unwrap_err()
    );
    assert!(message.contains("running"), "{message}");

    record.finish(Status::Completed).unwrap();
    assert!(record.begin_review(T0, Process::new(1, T0)).is_err());
}

#[test]
fn a_status_that_does_not_belong_to_the_stage_is_refused() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1, T0))
        .unwrap();
    let message = format!("{:#}", record.finish(Status::Passed).unwrap_err());
    assert!(
        message.contains("passed") && message.contains("working"),
        "{message}"
    );
}

#[test]
fn write_then_read_round_trips_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let task = dir.path().join("tasks").join("issue-41");
    let mut record = new_record();
    record.hooks = serde_json::json!({"plan": {"x": 1}});

    record::write(&task, &record).unwrap();

    assert_eq!(record::read(&task.join("task.json")).unwrap(), record);
    let names: Vec<_> = fs::read_dir(&task)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["task.json"]);
}

#[test]
fn a_failed_write_is_an_error_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("issue-41");
    fs::write(&blocker, "a file where the task folder should be").unwrap();

    let message = format!("{:#}", record::write(&blocker, &new_record()).unwrap_err());

    assert!(message.contains("issue-41"), "{message}");
}

#[test]
fn a_newer_schema_is_refused_with_a_hint_to_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task.json");
    let mut value = serde_json::to_value(new_record()).unwrap();
    value["schema"] = serde_json::json!(SCHEMA + 1);
    fs::write(&path, value.to_string()).unwrap();

    let message = format!("{:#}", record::read(&path).unwrap_err());

    assert!(message.contains("upgrade sbxm"), "{message}");
    assert!(message.contains("task.json"), "{message}");
}

#[test]
fn a_damaged_record_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task.json");
    fs::write(&path, "{ not json").unwrap();
    let message = format!("{:#}", record::read(&path).unwrap_err());
    assert!(message.contains("task.json"), "{message}");
}

#[test]
fn a_running_task_is_interrupted_when_its_process_is_gone_or_was_replaced() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1234, T0))
        .unwrap();

    assert!(
        !record.is_interrupted(&Probe(Some(T0))),
        "same process, still running"
    );
    assert!(record.is_interrupted(&Probe(None)), "pid absent");
    assert!(
        record.is_interrupted(&Probe(Some(T0 + 500))),
        "the pid now belongs to a different process"
    );
}

#[test]
fn the_real_probe_sees_this_process_and_not_a_missing_one() {
    use sbxm::task::record::SystemProbe;
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::current(&SystemProbe))
        .unwrap();
    assert!(
        !record.is_interrupted(&SystemProbe),
        "this very process is running it"
    );

    record.process = Some(Process::new(u32::MAX - 1, T0));
    assert!(record.is_interrupted(&SystemProbe), "no such pid");
}

#[test]
fn a_running_task_with_no_process_is_interrupted() {
    let mut record = new_record();
    record.process = None;
    assert!(record.is_interrupted(&Probe(Some(T0))));
}

#[test]
fn a_task_that_is_not_running_is_never_interrupted() {
    let mut record = new_record();
    record
        .advance(Stage::Working, T0, Process::new(1234, T0))
        .unwrap();
    record.finish(Status::Completed).unwrap();
    assert!(!record.is_interrupted(&Probe(None)));
}

#[test]
fn load_all_lists_tasks_by_id_and_skips_folders_without_a_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = record::tasks_root(dir.path());
    for number in [12, 3] {
        let mut record = new_record();
        record.id = format!("issue-{number}");
        record.number = number;
        record::write(&root.join(&record.id), &record).unwrap();
    }
    fs::create_dir_all(root.join("issue-99")).unwrap();

    let ids: Vec<_> = record::load_all(dir.path())
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();

    assert_eq!(ids, ["issue-3", "issue-12"]);
}

#[test]
fn load_all_with_no_tasks_folder_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(record::load_all(dir.path()).unwrap().is_empty());
}

#[test]
fn only_issue_and_pr_ids_with_plain_numbers_are_valid() {
    for good in ["issue-1", "pr-12", "issue-4004"] {
        assert!(is_valid_id(good), "{good}");
    }
    for bad in [
        "",
        "issue-",
        "issue-0",
        "issue-01",
        "pr-x",
        "../issue-1",
        "issue-1/..",
        "task-1",
        "issue-1 ",
    ] {
        assert!(!is_valid_id(bad), "{bad:?}");
    }
}
