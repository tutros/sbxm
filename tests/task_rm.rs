//! M2b slice 10, steps 1-2: what `task rm` plans and removes (spec §4, §12, decision 153).

mod common;

use std::fs;
use std::path::PathBuf;

use common::dir_link;
use common::task_fixture::{CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play};
use sbxm::backend::{FakeBackend, SandboxInfo};
use sbxm::task::finish::{plan_removal, remove};
use sbxm::task::record::{self, ProcessProbe, Status};

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// Issue 41 worked, plus the extra folders a review leaves behind.
fn worked(f: &Fixture) -> (FakeBackend, sbxm::task::pipeline::Prepared) {
    let b = backend().with_exec_output_matching("claude", ok(CLAUDE_DONE));
    let b = b.with_exec_hook(play(
        &f.env.base_dir().join("tasks").join("issue-41"),
        "main",
        "issue-41",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    ));
    let prepared = common::task_fixture::worked_task(f, &b);
    (b, prepared)
}

fn tasks(f: &Fixture) -> PathBuf {
    f.env.base_dir().join("tasks")
}

#[test]
fn the_plan_lists_the_sandbox_the_clone_and_the_task_folder() {
    let f = fixture();
    let (_b, prepared) = worked(&f);
    let sandbox = prepared.record.worker.as_ref().unwrap().sandbox.clone();

    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    assert_eq!(plan.sandboxes, vec![sandbox.clone()]);
    let listing = plan.listing();
    assert!(listing.contains(&format!("sandbox {sandbox}")), "{listing}");
    assert!(
        listing.contains(&tasks(&f).join("issue-41").display().to_string()),
        "{listing}"
    );
    assert!(
        listing.contains(&prepared.meta.display().to_string()),
        "{listing}"
    );
    // The reviewer's folders aren't there, so they aren't listed.
    assert!(!listing.contains("issue-41-review"), "{listing}");
}

#[test]
fn leftover_review_and_gate_checkouts_are_listed_too() {
    let f = fixture();
    let (_b, _p) = worked(&f);
    fs::create_dir_all(tasks(&f).join("issue-41-review")).unwrap();
    fs::create_dir_all(tasks(&f).join("issue-41-gates")).unwrap();

    let listing = plan_removal(&f.env.base_dir(), "issue-41", &Probe)
        .unwrap()
        .listing();

    assert!(listing.contains("issue-41-review"), "{listing}");
    assert!(listing.contains("issue-41-gates"), "{listing}");
}

#[test]
fn an_id_that_is_not_a_task_id_is_refused_before_any_path_is_built() {
    let f = fixture();
    for id in ["../x", "issue-0", "issue-4/1", "issue-41-review", "", "pr-"] {
        let err = plan_removal(&f.env.base_dir(), id, &Probe).unwrap_err();
        assert!(
            format!("{err:#}").contains("isn't a task id"),
            "{id}: {err:#}"
        );
    }
}

#[test]
fn an_unknown_task_says_so() {
    let f = fixture();
    let err = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap_err();
    assert!(format!("{err:#}").contains("no task issue-41"), "{err:#}");
}

#[test]
fn a_running_task_is_refused_but_an_interrupted_one_can_go() {
    let f = fixture();
    let (_b, mut prepared) = worked(&f);
    prepared.record.status = Status::Running;
    record::write(&prepared.meta, &prepared.record).unwrap();

    let err = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap_err();
    assert!(format!("{err:#}").contains("is running"), "{err:#}");

    plan_removal(&f.env.base_dir(), "issue-41", &Gone).unwrap();
}

#[test]
fn a_sandbox_name_that_is_not_the_tasks_is_refused() {
    let f = fixture();
    let (_b, mut prepared) = worked(&f);
    prepared.record.worker.as_mut().unwrap().sandbox = "sbxm-someone-else-claude".into();
    record::write(&prepared.meta, &prepared.record).unwrap();

    let err = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap_err();

    assert!(
        format!("{err:#}").contains("isn't one of its sandboxes"),
        "{err:#}"
    );
}

#[test]
fn an_unreadable_record_is_still_removable_with_a_note() {
    let f = fixture();
    let (_b, prepared) = worked(&f);
    fs::write(prepared.meta.join("task.json"), "{ not json").unwrap();

    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    assert!(plan.sandboxes.is_empty());
    assert!(plan.listing().contains("task.json can't be read"));
}

#[test]
fn a_record_from_a_newer_sbxm_is_not_removed() {
    let f = fixture();
    let (_b, prepared) = worked(&f);
    let path = prepared.meta.join("task.json");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\"schema\": 1", "\"schema\": 99");
    fs::write(&path, text).unwrap();

    let err = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap_err();

    assert!(format!("{err:#}").contains("upgrade sbxm"), "{err:#}");
}

#[test]
fn removal_takes_the_sandbox_then_the_clone_then_the_task_folder_and_nothing_else() {
    let f = fixture();
    let (b, prepared) = worked(&f);
    let bystander = tasks(&f).join("issue-42");
    fs::create_dir_all(&bystander).unwrap();
    fs::write(bystander.join("keep.txt"), "x").unwrap();
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &b);

    assert!(report.is_clean(), "{report:?}");
    assert_eq!(b.removes(), vec![prepared.record.worker.unwrap().sandbox]);
    assert!(!prepared.meta.exists());
    assert!(!tasks(&f).join("issue-41").exists());
    assert!(bystander.join("keep.txt").exists());
    assert!(f.env.base_dir().join(".sbxm").join("tasks").exists());
}

#[test]
fn a_link_in_place_of_a_clone_is_not_followed_and_the_rest_still_goes() {
    let f = fixture();
    let (b, prepared) = worked(&f);
    let outside = f.env.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious.txt"), "x").unwrap();
    fs::create_dir_all(tasks(&f).join("issue-41-gates")).unwrap();
    fs::remove_dir_all(tasks(&f).join("issue-41")).unwrap();
    dir_link(&tasks(&f).join("issue-41"), &outside);
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &b);

    assert!(!report.is_clean());
    let stayed = report.stayed.join("\n");
    assert!(stayed.contains("symlink or junction"), "{stayed}");
    assert!(outside.join("precious.txt").exists());
    assert!(!tasks(&f).join("issue-41-gates").exists());
    // The task folder stays so a second rm still knows the sandboxes.
    assert!(prepared.meta.join("task.json").exists());
    assert!(stayed.contains("second `sbxm task rm`"), "{stayed}");
}

#[test]
fn a_link_inside_a_clone_is_removed_without_touching_what_it_points_at() {
    let f = fixture();
    let (b, _prepared) = worked(&f);
    let outside = f.env.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("precious.txt"), "x").unwrap();
    dir_link(&tasks(&f).join("issue-41").join("escape"), &outside);
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &b);

    assert!(report.is_clean(), "{report:?}");
    assert!(outside.join("precious.txt").exists());
    assert!(!tasks(&f).join("issue-41").exists());
}

#[test]
fn a_sandbox_that_will_not_go_keeps_the_task_folder_and_is_reported() {
    let f = fixture();
    let (_b, prepared) = worked(&f);
    let sandbox = prepared.record.worker.as_ref().unwrap().sandbox.clone();
    let stuck = FakeBackend::failing_remove().and_sandboxes(vec![SandboxInfo {
        name: sandbox.clone(),
        agent: "claude".into(),
        status: "running".into(),
    }]);
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &stuck);

    assert!(!report.is_clean());
    assert!(report.stayed[0].contains(&sandbox), "{report:?}");
    assert!(prepared.meta.join("task.json").exists());
    // The clone did go: removal is best effort and carries on.
    assert!(!tasks(&f).join("issue-41").exists());
}

#[test]
fn a_task_folder_that_resolves_elsewhere_is_not_deleted() {
    let f = fixture();
    let (b, _prepared) = worked(&f);
    // `.sbxm/tasks` itself is a link to somewhere else.
    let elsewhere = f.env.tmp.path().join("elsewhere");
    fs::create_dir_all(elsewhere.join("issue-41")).unwrap();
    fs::write(elsewhere.join("issue-41").join("keep.txt"), "x").unwrap();
    let root = record::tasks_root(&f.env.base_dir());
    let moved = f.env.base_dir().join(".sbxm").join("tasks-real");
    fs::rename(&root, &moved).unwrap();
    dir_link(&root, &elsewhere);
    let plan = plan_removal(&f.env.base_dir(), "issue-41", &Probe).unwrap();

    let report = remove(&plan, &b);

    assert!(!report.is_clean(), "{report:?}");
    assert!(elsewhere.join("issue-41").join("keep.txt").exists());
}
