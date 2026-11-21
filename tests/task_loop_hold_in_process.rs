//! Review M-1 of PR 734 (issue 727): with `cloud_slots` unset, the loop runs a queued spec job's
//! `task run --spec` in its own process, not as a child; its task is still held before its
//! review while its changed files overlap other open loop work. The production steps clone
//! `https://github.com/o/r.git`, so this binary points the user's git config (whole-process
//! state) at a local origin, and it has a binary of its own.

mod common;

use std::cell::Cell;
use std::fs;
use std::time::Duration;

use anyhow::Result;
use common::task_fixture::{
    CLAUDE_DONE, CLEAN_REVIEW, CODEX_DONE, Fixture, Play, Probe, fixture, ok, play, play_reviews,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_loop::child::{Child, Exit, Spawner};
use sbxm::commands::task_loop::{self, Env, Options, TaskSteps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrFile, PrFiles, PrInfo, PrState};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::hold::Wait;
use sbxm::task::jobs;
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Status};

const START: u64 = 1_793_613_600;

/// The one-at-a-time loop runs a spec job here, starting no child.
struct NoSpawn;

impl Spawner for NoSpawn {
    fn run(&self, child: &Child, _tick: &dyn Fn()) -> Result<Exit> {
        panic!("a one-at-a-time spec job starts no child: {child:?}")
    }

    fn stop(&self) {}
}

/// `https://github.com/o/r.git` read as the fixture's local origin, and a committer, for every
/// git this process starts with the user's config.
fn origin_as_github(f: &Fixture) {
    let config = f.env.tmp.path().join("gitconfig");
    let origin = f.origin.to_str().unwrap().replace('\\', "/");
    fs::write(
        &config,
        format!(
            "[user]\n\tname = Dev\n\temail = dev@example.com\n[url \"{origin}\"]\n\tinsteadOf = \
             https://github.com/o/r.git\n"
        ),
    )
    .unwrap();
    // The only test of this binary sets it before any thread of its own starts.
    unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", &config) };
}

/// A finished loop task of issue 3 that opened PR #30, and GitHub with #30 open changing `a.txt`.
fn open_loop_pr(f: &Fixture) -> FakeGitHub {
    let mut rec = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: 3,
            repo: "o/r",
            title: "",
            base: "main",
            branch: "issue-3",
            config_hash: "h",
            id: None,
        },
        START,
        Process::new(1, START),
    );
    rec.stage = Stage::Finished;
    rec.status = Status::Ok;
    rec.process = None;
    rec.pr = Some("https://github.com/o/r/pull/30".into());
    rec.hooks = serde_json::json!({ LOOP_HOOK: true });
    record::write(&record::task_dir(&f.env.base_dir(), "issue-3"), &rec).unwrap();
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_pr(PrInfo {
            number: 30,
            head_ref: "issue-3".into(),
            head_oid: "0123456789abcdef0123456789abcdef01234567".into(),
            is_cross_repository: false,
            head_repo: "o/r".into(),
            state: PrState::Open,
            title: "PR 30".into(),
            body: String::new(),
            closing_issues: vec![],
            base_moved: false,
        })
        .with_pr_files(
            30,
            PrFiles {
                changed_files: 1,
                files: vec![PrFile {
                    filename: "a.txt".into(),
                    previous_filename: None,
                }],
            },
        )
}

#[test]
fn a_spec_job_run_in_the_loops_own_process_is_held_before_its_review() {
    let f = fixture();
    origin_as_github(&f);
    let specs = f.env.tmp.path().join("specs");
    fs::create_dir_all(&specs).unwrap();
    fs::write(specs.join("retro.md"), "Build a thing.\n").unwrap();
    let value = toml::Value::String(specs.to_str().unwrap().to_owned());
    f.env.set_config(&format!("specs_dir = {value}"));
    let spec = fs::canonicalize(specs.join("retro.md")).unwrap();
    let (id, _) = record::spec_id(&spec).unwrap();
    let queue = jobs::queue_dir(&f.env.base_dir());
    fs::create_dir_all(&queue).unwrap();
    fs::write(
        queue.join("s.json"),
        r#"{"kind": "spec", "file": "retro.md"}"#,
    )
    .unwrap();

    let worker = play(
        &f.env.base_dir().join("tasks").join(&id),
        "main",
        &id,
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN_REVIEW.to_owned()]);
    let backend = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        });
    let github = open_loop_pr(&f);
    let host = FakeHostRunner::default();
    let config_dir = f.env.config_dir();
    // The loop's checkout: a clone of `o/r` whose `origin/HEAD` names `main`.
    let repo_root = f.env.tmp.path().join("target-repo");
    common::git(&repo_root, &["init", "-q", "-b", "main"]);
    common::git(
        &repo_root,
        &["remote", "add", "origin", "git@github.com:o/r.git"],
    );
    common::git(
        &repo_root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let steps = TaskSteps {
        config_dir: &config_dir,
        repo_root: repo_root.clone(),
        repo: Some("o/r".into()),
        backend: &backend,
        github: &github,
        probe: &Probe,
        host: &host,
        disk: sbxm::disk::Guard::default(),
        stop: None,
        spawner: &NoSpawn,
    };
    let clock = Cell::new(START + 3600);
    let sleeps = Cell::new(0usize);
    let now = || clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        clock.set(clock.get() + d.as_secs());
    };
    let stop = || sleeps.get() > 1;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run(
        &config_dir,
        &Options {
            repo_root,
            issues: Vec::new(),
            cap: None,
            repo: Some("o/r".into()),
            wait: false,
        },
        &steps,
        &github,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    let said = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    assert!(result.is_ok(), "{result:?}\n{said}");

    let rec = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json"))
        .unwrap_or_else(|e| panic!("{e:#}\n{said}"));
    assert_eq!(rec.stage, Stage::Held, "{said}");
    assert_eq!(
        rec.holds[0].on,
        vec![Wait::Pr {
            number: 30,
            armed: false
        }]
    );
    let reviews = backend
        .execs()
        .iter()
        .filter(|(sandbox, _)| sandbox.contains("-review-"))
        .count();
    assert_eq!(reviews, 0, "no reviewer ran\n{said}");
    // The step's mark ended with it: a command run by hand now is not the loop's.
    assert!(!record::run_by_loop());

    // Once #30 is merged the loop releases the task: it resumes and gets its one review.
    github.set_merged(30, "cccccccccccccccccccccccccccccccccccccccc");
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    sleeps.set(0);
    let result = task_loop::run(
        &config_dir,
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            issues: Vec::new(),
            cap: None,
            repo: Some("o/r".into()),
            wait: false,
        },
        &steps,
        &github,
        &Probe,
        &env,
        &mut out,
        &mut warn,
    );
    let said = String::from_utf8(out).unwrap() + &String::from_utf8(warn).unwrap();
    assert!(result.is_ok(), "{result:?}\n{said}");
    let rec = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!(rec.stage, Stage::Finished, "{said}");
    let reviews = backend
        .execs()
        .iter()
        .filter(|(sandbox, _)| sandbox.contains("-review-"))
        .count();
    assert_eq!(reviews, 1, "one review, after the release\n{said}");
}
