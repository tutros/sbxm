//! Issue 671: what `sbxm status` shows that needs the user's eye. Where each task runs (the
//! WHERE column), a task the loop parked at `ready` with the command to run next, every open task
//! PR with its risk and whether auto-merge is armed, a dead loop with the tasks it left, and when
//! an idle loop next looks for work.

use crate::common;

use std::fs;
use std::path::Path;

use common::Env as TestEnv;
use sbxm::backend::Location;
use sbxm::status::tasks::{self, TaskRow, TaskTime};
use sbxm::task::record::{
    self, Kind, NewTask, Process, ProcessProbe, Record, SandboxLife, SandboxRole,
};

const REPO: &str = "o/r";
const T0: u64 = 1_790_000_000;

struct Probe(Option<u64>);

impl ProcessProbe for Probe {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        self.0
    }
}

fn task(number: u32) -> Record {
    Record::new(
        &NewTask {
            kind: Kind::Issue,
            number,
            repo: REPO,
            title: &format!("title {number}"),
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        T0,
        Process::new(1, T0),
    )
}

fn save(base: &Path, rec: &Record) {
    record::write(&record::task_dir(base, &rec.id), rec).unwrap();
}

fn life(name: &str, removed: bool) -> SandboxLife {
    SandboxLife {
        name: name.to_owned(),
        role: SandboxRole::Worker,
        location: Location::Cloud,
        created_at: "2026-11-15T10:00:00Z".to_owned(),
        ttl_s: 3_600,
        removed_at: removed.then(|| "2026-11-15T11:00:00Z".to_owned()),
    }
}

fn row(id: &str, location: Option<&str>) -> TaskRow {
    TaskRow {
        id: id.to_owned(),
        title: String::new(),
        stage: "working",
        status: "running".to_owned(),
        time: TaskTime::InStage(None),
        commits_ahead: None,
        must_fix: None,
        pr: None,
        location: location.map(str::to_owned),
        log_line: None,
    }
}

// ---- 1. where each task runs ----------------------------------------------------------------

#[test]
fn a_local_task_runs_local() {
    let base = TestEnv::new();
    let b = base.base_dir();
    save(&b, &task(1));

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("local"));
}

#[test]
fn a_cloud_task_names_its_sandbox_still_there() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = task(2);
    rec.location = Location::Cloud;
    rec.sandboxes = vec![life("old-box", true), life("live-box", false)];
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("cloud (live-box)"));
}

#[test]
fn a_cloud_task_without_a_sandbox_is_cloud_alone() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let mut rec = task(3);
    rec.location = Location::Cloud;
    rec.sandboxes = vec![life("gone-box", true)];
    save(&b, &rec);

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows[0].location.as_deref(), Some("cloud"));
}

#[test]
fn an_older_record_without_a_location_field_has_none() {
    let base = TestEnv::new();
    let b = base.base_dir();
    let rec = task(4);
    save(&b, &rec);
    let path = record::task_dir(&b, &rec.id).join("task.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("task.json is JSON");
    json.as_object_mut().unwrap().remove("location");
    fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let rows = tasks::rows(&b, &Probe(None)).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].location, None);
}

#[test]
fn the_where_column_shows_each_location_and_a_dash_for_none() {
    let text = tasks::render(
        &[
            row("issue-1", Some("local")),
            row("issue-2", Some("cloud (live-box)")),
            row("issue-3", None),
        ],
        0,
        None,
    );

    insta::assert_snapshot!(text, @r"
    ID       STAGE    STATUS   TIME  AHEAD  FINDINGS  PR  WHERE             LOG
    issue-1  working  running  -     -      -         -   local             -
    issue-2  working  running  -     -      -         -   cloud (live-box)  -
    issue-3  working  running  -     -      -         -   -                 -
    ");
}
