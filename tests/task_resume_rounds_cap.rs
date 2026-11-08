//! Issue 436: `task resume --rounds N` gives N more fix rounds from now, whatever the stop: the
//! cap becomes the rounds already used plus N.

mod common;

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, fixture_with, ok, play_reviews, play_tasks, worked_task,
};
use sbxm::backend::FakeBackend;
use sbxm::commands::task_resume::{Options, Target, run};
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, ProcessProbe, Stopped};

const ONE: &str = "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 wrong.\n";

struct Gone;

impl ProcessProbe for Gone {
    fn start_time(&self, _pid: u32) -> Option<u64> {
        None
    }
}

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n",
    )
}

fn backend(f: &Fixture, reviews: &[&str]) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        "issue-41",
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

/// Issue 41 reviewed through `[ONE, ONE]` with its one fix round: stopped `rounds-exhausted`
/// after 1 fix round, with a cap of 1.
fn stopped(f: &Fixture) {
    let first = backend(f, &[ONE, ONE]);
    let mut prepared = worked_task(f, &first);
    let env = sbxm::task::pipeline::TaskEnv {
        config_dir: &f.env.config_dir(),
        config: &f.config,
        backend: &first,
        probe: &Gone,
        host: &FakeHostRunner::default(),
        sleep: &std::thread::sleep,
        scoped_secrets: &[],
        disk: Box::leak(Box::default()),
        identity: None,
        clone_source: None,
    };
    sbxm::task::pipeline::review_issue(&env, &mut prepared).unwrap();
}

fn task_json(f: &Fixture) -> std::path::PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41").join("task.json")
}

fn resume(f: &Fixture, rounds: u32, backend: &FakeBackend) -> String {
    let opts = Options {
        repo_root: f.env.tmp.path().join("target-repo"),
        target: Target::Issue(41),
        rounds: Some(rounds),
        disk: Default::default(),
        identity: None,
        forget_ambiguous: false,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    run(
        &f.env.config_dir(),
        &opts,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn a_repeat_stop_below_its_cap_resumed_with_rounds_1_runs_exactly_one_more_fix_round() {
    let f = config();
    stopped(&f);
    // As the #375 + #376 task was: stopped `repeat-finding` after 1 fix round, with a cap of 3.
    let path = task_json(&f);
    let mut task = record::read(&path).unwrap();
    assert_eq!(task.round, 1);
    task.fix_rounds = 3;
    task.stopped = Some(Stopped::RepeatFinding);
    record::write(path.parent().unwrap(), &task).unwrap();
    let backend = backend(&f, &[ONE, ONE, ONE, ONE, ONE]);

    let out = resume(&f, 1, &backend);

    assert!(
        out.contains("issue-41: 1 more fix round(s), 2 in all (stopped: repeat-finding)"),
        "{out}"
    );
    let saved = record::read(&path).unwrap();
    assert_eq!(saved.round, 2, "exactly one more fix round");
    assert_eq!(saved.fix_rounds, 2, "the cap is the rounds used plus 1");
    assert_eq!(saved.stopped, Some(Stopped::RoundsExhausted));
}

#[test]
fn a_rounds_exhausted_stop_gets_the_cap_it_had_plus_n() {
    let f = config();
    stopped(&f);
    let backend = backend(&f, &[ONE, ONE, ONE, ONE, ONE]);

    let out = resume(&f, 2, &backend);

    assert!(
        out.contains("issue-41: 2 more fix round(s), 3 in all (stopped: rounds-exhausted)"),
        "{out}"
    );
    let saved = record::read(&task_json(&f)).unwrap();
    assert_eq!(saved.round, 3);
    assert_eq!(saved.fix_rounds, 3);
    assert_eq!(saved.stopped, Some(Stopped::RoundsExhausted));
}

#[test]
fn the_rounds_help_says_n_more_from_now_whatever_the_stop() {
    let output = assert_cmd::Command::cargo_bin("sbxm")
        .unwrap()
        .args(["task", "resume", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let flat = stdout.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("N more fix rounds from now (its budget becomes the rounds used plus N, whatever the stop)"),
        "{stdout}"
    );
}
