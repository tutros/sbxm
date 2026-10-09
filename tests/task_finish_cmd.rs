//! M2b slice 10, step 6: `sbxm task finish` as a command.

mod common;

use std::fs;

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::commands::task_finish::{Options, Target, run};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::record::{self, Process, Stage, Status};

fn ready(f: &Fixture) {
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
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), "Must-fix findings: 0\n").unwrap();
}

fn go(f: &Fixture, issue: u32, github: &FakeGitHub) -> (anyhow::Result<()>, String) {
    let mut out = Vec::new();
    let result = run(
        &f.env.config_dir(),
        &Options {
            target: Target::Issue(issue),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        github,
        &Probe,
        &mut out,
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn it_prints_the_pr_url_and_the_next_step() {
    let f = fixture();
    ready(&f);

    let (result, out) = go(&f, 41, &FakeGitHub::default());

    result.unwrap();
    assert!(
        out.contains("issue-41: pushed issue-41 and opened https://github.com/o/r/pull/"),
        "{out}"
    );
    assert!(out.contains("sbxm task rm --issue 41"), "{out}");
}

#[test]
fn a_draft_pr_is_called_a_draft_with_what_is_left() {
    let f = fixture();
    ready(&f);
    let meta = f
        .env
        .base_dir()
        .join(".sbxm")
        .join("tasks")
        .join("issue-41");
    fs::write(meta.join("review.md"), "Must-fix findings: 1\n").unwrap();

    let (result, out) = go(&f, 41, &FakeGitHub::default());

    result.unwrap();
    assert!(
        out.contains("issue-41: pushed issue-41 and opened draft PR https://github.com/o/r/pull/"),
        "{out}"
    );
    assert!(out.contains("1 must-fix finding(s) left"), "{out}");
}

#[test]
fn issue_zero_and_an_unknown_task_are_refused_with_a_fix() {
    let f = fixture();

    let (zero, _) = go(&f, 0, &FakeGitHub::default());
    let (unknown, _) = go(&f, 41, &FakeGitHub::default());

    assert!(format!("{:#}", zero.unwrap_err()).contains("pass --issue <n>"));
    assert!(
        format!("{:#}", unknown.unwrap_err()).contains("no task issue-41; run `sbxm task status`")
    );
}
