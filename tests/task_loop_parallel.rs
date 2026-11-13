//! Issue 615 (milestone 5 slice 4, decisions 317(4), (6), 321): the loop keeps up to
//! `sandbox_slots` local and `cloud_slots` cloud children running at once, each an issue task end
//! to end; its log row names the slot a task ran in.

use std::fs;

use sbxm::backend::Location;
use sbxm::task::queue::{Row, append_row};

// --- the log's Slot column ---

fn row(issue: u32, slot: Option<Location>) -> Row {
    Row {
        issue,
        title: "Fix it".into(),
        size: Some("S".into()),
        slot,
        started: 1_761_231_558, // 2025-10-23T14:59:18Z
        ready: None,
        worker_s: None,
        reviewer_s: None,
        rounds: None,
        must_fix: None,
        outcome: "parked at working".into(),
        stops: Vec::new(),
        idle_before: None,
    }
}

#[test]
fn a_rows_slot_column_names_where_its_task_ran() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("loop.md");

    append_row(&path, "o/r", &row(1, Some(Location::Local))).unwrap();
    append_row(&path, "o/r", &row(2, Some(Location::Cloud))).unwrap();
    append_row(&path, "o/r", &row(3, None)).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("| # | Issue | Size | Slot | Started | Ready (time to ready) |"),
        "{text}"
    );
    assert!(
        text.contains("| 1 | #1 Fix it | S | local | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 2 | #2 Fix it | S | cloud | 14:59:18 | - |"),
        "{text}"
    );
    assert!(
        text.contains("| 3 | #3 Fix it | S | - | 14:59:18 | - |"),
        "{text}"
    );
}

// --- `sbxm status` lists every running child ---

mod status_children {
    use std::fs;
    use std::path::Path;

    use sbxm::backend::{FakeBackend, Location};
    use sbxm::commands::status::{self, Env, LoopSource};
    use sbxm::github::fake::FakeGitHub;
    use sbxm::task::queue::{LockFile, lock_path};
    use sbxm::task::record::{self, Kind, NewTask, Process, ProcessProbe, Record, Stage, Status};

    const T0: u64 = 1_790_000_000;
    const LOOP_START: u64 = T0 - 1_000;

    struct Alive;

    impl ProcessProbe for Alive {
        fn start_time(&self, pid: u32) -> Option<u64> {
            Some(if pid == 1 { LOOP_START } else { T0 })
        }
    }

    /// Issue `number`'s task at `stage` in `slot`, run by a child (pid 10 + number) of the loop.
    fn child_task(base: &Path, number: u32, stage: Stage, slot: Location) {
        let mut rec = Record::new(
            &NewTask {
                kind: Kind::Issue,
                number,
                repo: "o/r",
                title: &format!("title {number}"),
                base: "main",
                branch: "b",
                config_hash: "h",
                id: None,
            },
            T0,
            Process::new(10 + number, T0),
        );
        rec.stage = stage;
        rec.status = Status::Running;
        rec.location = slot;
        rec.process = Some(Process::new(10 + number, T0));
        rec.loop_owner = Some(Process::new(1, LOOP_START));
        record::write(&record::task_dir(base, &rec.id), &rec).unwrap();
    }

    fn loop_section(base: &Path) -> String {
        listed_section(base, &[])
    }

    /// The Loop section for a loop started with `--issue` over `issues` (none: the labeled
    /// queue), its section up to the jobs line.
    fn listed_section(base: &Path, issues: &[u32]) -> String {
        let path = lock_path(base);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = LockFile {
            process: Process::new(1, LOOP_START),
            issues: issues.to_vec(),
            cap: None,
            progress: None,
        };
        fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
        let gh = FakeGitHub::default();
        let source = LoopSource {
            github: &gh,
            repo: Ok("o/r".into()),
        };
        let env = Env {
            now: Box::new(|| T0),
            sleep: Box::new(|_| {}),
            terminal: false,
            width: 120,
            iterations: None,
            monotonic: Box::new(|| std::time::Duration::ZERO),
            slots: None,
            cloud_slots: None,
            disk: sbxm::status::footer::Disk {
                free_space: |_| Ok(40 * 1024 * 1024 * 1024),
                same_drive: |_, _| true,
                state_dir: Some(std::path::PathBuf::from("/state")),
                min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
            },
        };
        let mut out = Vec::new();
        status::run_with(
            base,
            &FakeBackend::default(),
            &Alive,
            &env,
            None,
            Some(&source),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let section = text.split("Loop:").nth(1).expect("a Loop section");
        section.split("\n  done:").next().unwrap().to_owned()
    }

    #[test]
    fn status_lists_every_running_child_with_its_slot_and_stage() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 3, Stage::Working, Location::Local);
        child_task(&base, 4, Stage::Reviewing, Location::Cloud);
        child_task(&base, 5, Stage::Working, Location::Cloud);

        let section = loop_section(&base);

        assert!(
            section.ends_with(
                "\n  task: issue-3 (title 3) at working, local\
                 \n  task: issue-4 (title 4) at reviewing, cloud\
                 \n  task: issue-5 (title 5) at working, cloud"
            ),
            "{section}"
        );
    }

    #[test]
    fn status_names_a_single_childs_slot_too() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 4, Stage::Working, Location::Cloud);

        let section = loop_section(&base);

        assert!(
            section.ends_with("\n  task: issue-4 (title 4) at working, cloud"),
            "{section}"
        );
    }

    #[test]
    fn a_listed_loop_counts_its_other_children_as_running_not_pending() {
        let env = common_env();
        let base = env.path().join("base");
        child_task(&base, 3, Stage::Working, Location::Local);
        child_task(&base, 4, Stage::Working, Location::Cloud);

        let text = listed_section(&base, &[3, 4, 5]);

        assert!(
            text.contains(
                "\n  task: issue-3 (title 3) at working, local, task 1 of 3\
                 \n  task: issue-4 (title 4) at working, cloud"
            ),
            "{text}"
        );
    }

    fn common_env() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("base")).unwrap();
        tmp
    }
}
