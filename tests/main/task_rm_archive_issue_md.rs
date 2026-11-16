//! Issue 698: `task rm` archives the task's `issue.md` with its other text records, before the
//! task folder goes, so `task facts` still reads the task's size once it is archived.

use crate::common;

use std::fs;

use common::fix_task::yes_on_a_terminal;
use common::task_fixture::{CLAUDE_DONE, Play, Probe, backend, fixture, ok, play, worked_task};
use sbxm::commands::{task_facts, task_rm};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::merged::archive_dir;
use sbxm::task::record::{self, Status};
use serde_json::Value;

const ISSUE_MD: &str = "title:\tt\nstate:\tOPEN\nlabels:\tready-for-worker, size:M\n--\nbody\n";

#[test]
fn task_rm_archive_issue_md_is_copied_before_the_folder_goes_and_facts_reads_its_size() {
    let f = fixture();
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".into()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = worked_task(&f, &b);
    prepared.record.status = Status::Failed;
    record::write(&prepared.meta, &prepared.record).unwrap();
    let meta = prepared.meta.clone();
    let started = prepared.record.stages[0].at[..10].to_owned();
    drop(prepared);
    fs::write(meta.join("issue.md"), ISSUE_MD).unwrap();

    let mut out = Vec::new();
    task_rm::run(
        &f.env.config_dir(),
        &task_rm::Options {
            target: task_rm::Target::Issue(41),
            yes: false,
            force: false,
            repo_root: f.env.tmp.path().join("target-repo"),
            dry_run: false,
            forget_ambiguous: false,
        },
        &backend(),
        &FakeGitHub::default(),
        &Probe,
        &yes_on_a_terminal(),
        &mut out,
    )
    .unwrap();

    let archive = archive_dir(&f.env.base_dir(), "issue-41");
    assert_eq!(
        fs::read_to_string(archive.join("issue.md")).unwrap(),
        ISSUE_MD
    );
    assert!(!meta.exists());

    let (mut facts, mut warn) = (Vec::new(), Vec::new());
    task_facts::run(
        &f.env.config_dir(),
        &task_facts::Options {
            since: started,
            until: None,
            json: true,
        },
        u64::MAX / 2,
        &mut facts,
        &mut warn,
    )
    .unwrap();
    let rows: Value = serde_json::from_slice(&facts).unwrap();
    let sizes: Vec<&Value> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["size"])
        .collect();
    assert_eq!(sizes, ["M"], "{}", String::from_utf8_lossy(&warn));
}
