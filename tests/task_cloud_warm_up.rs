//! Issue 560: every strict cloud sandbox of a task is warmed up at the task's base before sbxm's
//! deny, then checked out at the task's branch: the worker's (`tests/task_cloud.rs` has its
//! start's order, the `[warm]` and nothing-to-warm cases and the failures), each reviewer's, the
//! worker sandbox a fix round or `task resume` makes again. A fix round in the worker sandbox the
//! run kept is not warmed up again. Through `FakeBackend`.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::cloud_task::{
    CLEAN, ONE, cloud_backend_observing, identity, reviewer_sandbox, spec, task_record, task_run,
    worker_sandbox,
};
use common::task_fixture::{CLAUDE_DONE, Fixture, fixture_with, is_headless, ok};
use sbxm::backend::{FakeBackend, Location};
use sbxm::task::record::Stage;

const LOCAL_TOML: &str = "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\nsandbox = [\"cargo test\"]\n";

const STRICT_PROFILE: &str = "[template]\nstrict = true\nallow = [\"crates.io\"]\n";

/// What every sandbox of these tests starts out allowing: both providers' hosts, a kept host and
/// the build hosts sbxm denies.
fn strict_list() -> Vec<&'static str> {
    vec![
        "api.anthropic.com",
        "claude.ai",
        "console.anthropic.com",
        "api.openai.com",
        "openai.com",
        "auth.openai.com",
        "chatgpt.com",
        "files.openai.com",
        "releases.openai.com",
        "crates.io",
        "index.crates.io",
        "github.com",
    ]
}

/// Adds a `Cargo.toml` and its lock to `main` in the fixture's origin; returns the new tip, the
/// base every task then starts from.
fn cargo_base(f: &Fixture) -> String {
    let seed = f.env.tmp.path().join("origin-cargo");
    common::git(
        f.env.tmp.path(),
        &[
            "clone",
            "-q",
            &common::task_fixture::source(f),
            seed.to_str().unwrap(),
        ],
    );
    std::fs::write(seed.join("Cargo.toml"), "[package]\nname = \"t\"\n").unwrap();
    std::fs::write(seed.join("Cargo.lock"), "version = 4\n").unwrap();
    common::git(&seed, &["add", "-A"]);
    common::git(&seed, &["commit", "-q", "-m", "cargo"]);
    common::git(&seed, &["push", "-q", "origin", "main"]);
    common::git(&seed, &["rev-parse", "HEAD"]).trim().to_owned()
}

/// Where a clone's HEAD was when an `exec` ran: its commit, and the branch it was on (`None` when
/// detached).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    commit: String,
    branch: Option<String>,
}

fn head_of(clone: &Path) -> Head {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(clone)
            .args(args)
            .output()
            .unwrap()
    };
    let commit = String::from_utf8(run(&["rev-parse", "HEAD"]).stdout).unwrap();
    let branch = run(&["symbolic-ref", "-q", "--short", "HEAD"]);
    Head {
        commit: commit.trim().to_owned(),
        branch: branch
            .status
            .success()
            .then(|| String::from_utf8(branch.stdout).unwrap().trim().to_owned()),
    }
}

/// Each warm-up step and agent run, in order: the sandbox, `warm` or `agent`, and the clone's
/// [`Head`] then (an agent's after its fake run committed).
type Seen = Arc<Mutex<Vec<(String, &'static str, Head)>>>;

/// [`cloud_backend_observing`] for one task, with [`strict_list`] allowed, recording [`Seen`].
fn strict_backend(f: &Fixture, id: &str, reviews: &[&str], listed: bool) -> (FakeBackend, Seen) {
    let seen: Seen = Arc::default();
    let record = seen.clone();
    let backend = cloud_backend_observing(
        &f.env.tmp.path().join("cloud-fs"),
        &[id],
        reviews,
        listed,
        ok(CLAUDE_DONE),
        move |sandbox, clone, spec| {
            let what = if is_headless(spec) {
                "agent"
            } else if spec.argv.join(" ").contains("--locked") {
                "warm"
            } else {
                return;
            };
            record
                .lock()
                .unwrap()
                .push((sandbox.to_owned(), what, head_of(clone)));
        },
    )
    .with_cloud_allow(&strict_list());
    (backend, seen)
}

/// What each make of `sandbox` did, as one word per step: `create`, `clone`,
/// `warm`, `deny` (sbxm's whole hold: the policy reads and denies), `checkout`, `agent`. A
/// sandbox used again without a create (a fix round in the kept worker) starts its own list at
/// its agent.
fn makes(backend: &FakeBackend, sandbox: &str) -> Vec<Vec<&'static str>> {
    let mut execs = backend.execs().into_iter();
    let mut out: Vec<Vec<&'static str>> = Vec::new();
    for entry in backend.log() {
        let spec = entry
            .starts_with("exec ")
            .then(|| execs.next().unwrap())
            .filter(|(s, _)| s == sandbox)
            .map(|(_, spec)| spec);
        if !entry.ends_with(sandbox) && !entry.contains(&format!("{sandbox} ")) && spec.is_none() {
            continue;
        }
        let word = match spec {
            Some(spec) => {
                let argv = spec.argv.join(" ");
                if is_headless(&spec) {
                    "agent"
                } else if argv.contains("git clone") {
                    "clone"
                } else if argv.contains("--locked") {
                    "warm"
                } else if argv.contains("checkout") {
                    "checkout"
                } else {
                    continue;
                }
            }
            None if entry.starts_with("create ") => "create",
            None if entry.starts_with("policy-") => "deny",
            None => continue,
        };
        match word {
            "create" => out.push(vec!["create"]),
            "agent" if out.last().is_none_or(|m| m.contains(&"agent")) => out.push(vec!["agent"]),
            _ => {
                let make = out.last_mut().expect("a step before any create");
                if !(word == "deny" && make.last() == Some(&"deny")) {
                    make.push(word);
                }
            }
        }
    }
    out
}

/// A sandbox made, warmed at the base, held, checked out at the branch, then its agent.
const WARMED: [&str; 8] = [
    "create", "clone", "warm", "warm", "warm", "deny", "checkout", "agent",
];

/// The `warm` heads of `seen` in `sandbox`.
fn warm_heads(seen: &Seen, sandbox: &str) -> Vec<Head> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(s, what, _)| s == sandbox && *what == "warm")
        .map(|(_, _, head)| head.clone())
        .collect()
}

/// The `agent` heads of `seen` in `sandbox`.
fn agent_heads(seen: &Seen, sandbox: &str) -> Vec<Head> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(s, what, _)| s == sandbox && *what == "agent")
        .map(|(_, _, head)| head.clone())
        .collect()
}

/// Boundaries: a reviewer sandbox is warmed at the base, though the branch it reviews has the
/// worker's commits, and the reviewer runs on the branch; a fix round in the worker sandbox the
/// run kept gets no second warm-up.
#[test]
fn each_reviewer_is_warmed_at_the_base_and_a_fix_round_in_the_kept_worker_is_not_warmed_again() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let base = cargo_base(&f);
    let (spec, id) = spec(&f);
    let (backend, seen) = strict_backend(&f, &id, &[ONE, CLEAN], true);
    let (worker, reviewer) = (worker_sandbox(&id), reviewer_sandbox(&id));

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.expect(&ran.out);
    let task = task_record(&f, &id);
    assert_eq!((task.stage, task.round), (Stage::Ready, 1));
    // The worker's sandbox: made and warmed once; the fix round runs in it with no warm-up.
    assert_eq!(makes(&backend, &worker), [WARMED.to_vec(), vec!["agent"]]);
    let at_base = Head {
        commit: base.clone(),
        branch: None,
    };
    assert_eq!(warm_heads(&seen, &worker), vec![at_base.clone(); 3]);
    // Each of the three review rounds' sandboxes: warmed at the base, reviewed on the branch.
    assert_eq!(makes(&backend, &reviewer), vec![WARMED.to_vec(); 3]);
    assert_eq!(warm_heads(&seen, &reviewer), vec![at_base; 9]);
    let reviewed = agent_heads(&seen, &reviewer);
    assert_eq!(reviewed.len(), 3);
    for head in reviewed {
        assert_eq!(head.branch.as_deref(), Some(id.as_str()));
        assert_ne!(head.commit, base, "the reviewer ran at the base");
    }
}

/// Boundary: a worker sandbox gone before a fix round is made again and warmed at the base, though
/// the branch has the worker's commit; the fix round runs on the branch, after the deny.
#[test]
fn a_worker_sandbox_made_again_for_a_fix_round_is_warmed_at_the_base() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let base = cargo_base(&f);
    let (spec, id) = spec(&f);
    let (backend, seen) = strict_backend(&f, &id, &[ONE, CLEAN], false);
    let worker = worker_sandbox(&id);

    let ran = task_run(&f, spec, Some(Location::Cloud), &backend);

    ran.result.expect(&ran.out);
    assert_eq!(makes(&backend, &worker), vec![WARMED.to_vec(); 2]);
    let at_base = Head {
        commit: base.clone(),
        branch: None,
    };
    assert_eq!(warm_heads(&seen, &worker), vec![at_base; 6]);
    let fixed = &agent_heads(&seen, &worker)[1];
    assert_eq!(fixed.branch.as_deref(), Some(id.as_str()));
    assert_ne!(fixed.commit, base);
}

/// `task resume --spec FILE --rounds N` (the process that ran the task is gone).
fn resume_rounds(f: &Fixture, spec: std::path::PathBuf, rounds: u32, backend: &FakeBackend) {
    resume_rounds_or_not(f, spec, Some(rounds), backend);
}

/// `task resume --spec FILE`, with `--rounds` when given.
fn resume_rounds_or_not(
    f: &Fixture,
    spec: std::path::PathBuf,
    rounds: Option<u32>,
    backend: &FakeBackend,
) {
    struct Gone;
    impl sbxm::task::record::ProcessProbe for Gone {
        fn start_time(&self, _pid: u32) -> Option<u64> {
            None
        }
    }
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = sbxm::commands::task_resume::run(
        &f.env.config_dir(),
        &sbxm::commands::task_resume::Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            target: sbxm::commands::task_resume::Target::Spec(spec),
            rounds,
            should_fix: false,
            disk: Default::default(),
            identity: Some(identity()),
            forget_ambiguous: false,
        },
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &Gone,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let said = format!(
        "{}{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(warn).unwrap()
    );
    result.expect(&said);
}

/// Boundary: the worker sandbox `task resume` makes again for a task whose branch has commits is
/// still warmed at the base, and its branch's commits are checked out only after the deny.
#[test]
fn a_worker_sandbox_made_again_by_resume_is_warmed_at_the_base() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let base = cargo_base(&f);
    let (spec, id) = spec(&f);
    let (first, _) = strict_backend(&f, &id, &[ONE], true);
    let ran = task_run(&f, spec.clone(), Some(Location::Cloud), &first);
    ran.result.expect(&ran.out);
    assert_eq!(task_record(&f, &id).stage, Stage::Ready);
    let (gone, seen) = strict_backend(&f, &id, &[CLEAN], false);
    let worker = worker_sandbox(&id);

    resume_rounds(&f, spec, 1, &gone);

    assert_eq!(makes(&gone, &worker), [WARMED.to_vec()]);
    let at_base = Head {
        commit: base.clone(),
        branch: None,
    };
    assert_eq!(warm_heads(&seen, &worker), vec![at_base; 3]);
    let resumed = &agent_heads(&seen, &worker)[0];
    assert_eq!(resumed.branch.as_deref(), Some(id.as_str()));
    assert_ne!(resumed.commit, base);
    assert_eq!(task_record(&f, &id).stage, Stage::Ready);
}

// Review M-1: a first start whose warm-up fails keeps its task, `prepared/failed`, so its
// `run.log` holds the failed step and `task resume` makes the sandbox afresh.

/// `task run --spec FILE --cloud` with the command's writers teed into the task's `run.log`, as
/// `main` does, and its error written the way `main` ends a failed task command.
fn task_run_logged(f: &Fixture, spec: std::path::PathBuf, id: &str, backend: &FakeBackend) {
    use sbxm::task::runlog::{RunLog, Tee};

    let log = Arc::new(Mutex::new(RunLog::new(
        &f.env.base_dir(),
        "# header".to_owned(),
        vec![id.to_owned()],
        None,
        Box::new(sbxm::run::results::now),
        Box::new(sbxm::task::runlog::recorded_stage),
        Box::new(|_| {}),
    )));
    let mut out = Tee::new(Vec::new(), Arc::clone(&log));
    let mut warn = Tee::new(Vec::new(), log);
    let result = sbxm::commands::task_run::run_spec(
        &f.env.config_dir(),
        &common::cloud_task::run_options(f, spec, Some(Location::Cloud)),
        None,
        backend,
        &sbxm::github::fake::FakeGitHub::default(),
        &common::task_fixture::Probe,
        &sbxm::task::gates::FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    let err = result.expect_err("the warm-up fails the run");
    use std::io::Write;
    writeln!(warn, "Error: {err:?}").unwrap();
}

/// `lines` numbered lines of output.
fn numbered(lines: usize) -> String {
    (1..=lines).map(|i| format!("out line {i}\n")).collect()
}

#[test]
fn a_first_start_whose_warm_up_fails_keeps_its_log_and_is_resumed_afresh() {
    let f = fixture_with(LOCAL_TOML);
    f.env.write_profile("default", STRICT_PROFILE);
    let base = cargo_base(&f);
    let (spec, id) = spec(&f);
    let worker = worker_sandbox(&id);
    let (failing, _) = strict_backend(&f, &id, &[CLEAN], false);
    let failing = failing.with_exec_output_matching(
        "cargo fetch",
        sbxm::backend::ExecOutput {
            stdout: numbered(50),
            stderr: String::new(),
            exit_code: Some(101),
        },
    );

    task_run_logged(&f, spec.clone(), &id, &failing);

    // The sandbox went, no agent ran, and the task stays `prepared/failed` with its folder.
    assert!(failing.log().contains(&format!("rm {worker}")));
    assert!(
        !failing.execs().iter().any(|(_, spec)| is_headless(spec)),
        "an agent ran"
    );
    let task = task_record(&f, &id);
    assert_eq!(
        (task.stage, task.status),
        (Stage::Prepared, sbxm::task::record::Status::Failed)
    );
    assert!(task.sandboxes.iter().all(|e| e.removed_at.is_some()));
    let run_log = std::fs::read_to_string(
        sbxm::task::record::task_dir(&f.env.base_dir(), &id).join("run.log"),
    )
    .unwrap();
    assert!(
        run_log.contains("warm-up step 1 of 3 `cargo fetch --locked` failed with exit 101"),
        "{run_log}"
    );
    assert!(
        run_log.contains("out line 11") && run_log.contains("out line 50"),
        "{run_log}"
    );

    // The failure gone, `task resume` makes the sandbox afresh: warmed at the base, held, then
    // the worker, and the task goes on to `ready`.
    let (fresh, seen) = strict_backend(&f, &id, &[CLEAN], false);
    resume_rounds_or_not(&f, spec, None, &fresh);

    assert_eq!(makes(&fresh, &worker)[0], WARMED.to_vec());
    let at_base = Head {
        commit: base,
        branch: None,
    };
    assert_eq!(warm_heads(&seen, &worker), vec![at_base; 3]);
    assert_eq!(task_record(&f, &id).stage, Stage::Ready);
}

/// Review S-1: a failed step's last 40 lines are taken from all of its output, not from the
/// gates' 8 KiB tail: 60 lines of about 310 bytes each keep their whole last 40.
#[test]
fn a_failed_step_keeps_its_last_40_lines_whole_however_long() {
    let pad = "x".repeat(300);
    let lines: String = (1..=60).map(|i| format!("long line {i} {pad}\n")).collect();
    assert!(lines.len() > 16 * 1024);
    let backend = FakeBackend::default().with_default_exec_output(sbxm::backend::ExecOutput {
        stdout: lines,
        stderr: String::new(),
        exit_code: Some(1),
    });
    let steps = sbxm::commands::template_build::warm_steps(None);

    let err = sbxm::task::warm_up::run(
        &backend,
        "sbxm-task-x-claude",
        "/home/agent/work/x",
        &steps,
        sbxm::task::warm_up::LIMIT,
    )
    .unwrap_err();

    let message = format!("{err:#}");
    for i in 21..=60 {
        assert!(
            message.contains(&format!("long line {i} {pad}")),
            "line {i} isn't whole: {message}"
        );
    }
    assert!(!message.contains("long line 20 "), "{message}");
    assert!(sbxm::task::warm_up::failed(&err));
}
