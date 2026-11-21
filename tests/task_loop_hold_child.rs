//! Issue 727, review M-1: a queued spec job's task, run as a child of `task loop`, is held before
//! its review through the commands' own wiring, as an issue task is. This test binary is such a
//! child: it sets the loop for the whole process, as `main` does from the loop's environment
//! variable, so it has a binary of its own.

mod common;

use common::cloud_task::run_options;
use common::task_fixture::{
    CLAUDE_DONE, CLEAN_REVIEW, CODEX_DONE, Fixture, Play, Probe, fixture, ok, play, play_reviews,
    spec_file,
};
use sbxm::backend::{FakeBackend, Location};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{PrFile, PrFiles, PrInfo, PrState};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::hold::Wait;
use sbxm::task::pipeline::OnHold;
use sbxm::task::queue::LOOP_HOOK;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Status};

const START: u64 = 1_793_613_600;

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
fn a_loop_spec_task_sharing_a_file_with_an_open_loop_pr_is_held_before_its_review() {
    record::started_by_loop(Process::new(77, START));
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
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
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &run_options(&f, spec, Some(Location::Local)),
        None,
        &backend,
        &github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let err = result.unwrap_err();
    assert!(err.is::<OnHold>(), "{err:#}");
    let rec = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!(rec.stage, Stage::Held);
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
    assert_eq!(reviews, 0, "no reviewer ran");
}

/// No recorded process is alive: the task's start has ended.
struct Gone;

impl record::ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

#[test]
fn a_loop_spec_task_resumed_after_its_worker_is_held_before_its_review() {
    use common::task_fixture::{ctx, source};
    use sbxm::commands::task_resume::{self, Options, Target};
    use sbxm::task::pipeline;

    record::started_by_loop(Process::new(77, START));
    let f = fixture();
    let spec = spec_file(&f, "Build a thing.\n");
    let (id, _) = record::spec_id(&spec).unwrap();
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
    // The worker ran; its command ended before the gates.
    {
        let github = FakeGitHub::default();
        let source = source(&f);
        let context = ctx(&f, &source, &backend, &github);
        let mut prepared = pipeline::prepare_spec(&context, &spec).unwrap();
        pipeline::run_worker(&context, &mut prepared).unwrap();
    }
    let github = open_loop_pr(&f);
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_resume::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: Target::Spec(spec),
            rounds: None,
            should_fix: false,
            disk: Default::default(),
            identity: None,
            forget_ambiguous: false,
        },
        &backend,
        &github,
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let err = result.unwrap_err();
    assert!(err.is::<OnHold>(), "{err:#}");
    let rec = record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).unwrap();
    assert_eq!(rec.stage, Stage::Held);
}
