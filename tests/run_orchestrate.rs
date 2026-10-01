//! M2a slice 6: one throwaway sandbox per (contestant, repeat) pair,
//! contestants in parallel and repeats as sequential waves, each sandbox
//! always removed (decisions 6, 15, 16, 95, 114).

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use sbxm::backend::{ExecOutput, FakeBackend, SkillsStore, Stdin};
use sbxm::harness::Harness;
use sbxm::headless::RunStatus;
use sbxm::run::config::RunConfig;
use sbxm::run::id::RunRoots;
use sbxm::run::kits::{self, RunKits};
use sbxm::run::orchestrate::{self, PairOutcome, in_sandbox_path};

const RUN_ID: &str = "2026-09-30-abc123";
/// Real output fixtures from slices 1 and 2.
const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");
const CODEX_PONG: &str = include_str!("../src/fixtures/codex-ndjson-pong.jsonl");
const CODEX_TRUNCATED: &str = include_str!("../src/fixtures/codex-ndjson-truncated.jsonl");

fn name(contestant: usize, repeat: u32) -> String {
    format!("sbxm-run-{RUN_ID}-{contestant}-{repeat}")
}

fn out(stdout: &str) -> ExecOutput {
    ExecOutput {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: Some(0),
    }
}

/// Claude and Codex contestants with `extra` appended (a `[run]` table must
/// come before the contestants, so `run` is inserted there).
struct Setup {
    /// Held only so the temp dir lives as long as the test.
    _env: Env,
    config: RunConfig,
    kits: RunKits,
    roots: RunRoots,
    workspaces: PathBuf,
}

fn setup(run_table: &str) -> Setup {
    let env = Env::new();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"Reply with exactly: PONG\"\n\n{run_table}\
             [[contestants]]\nharness = \"claude\"\nmodel = \"claude-haiku-4-5-20251001\"\n\n\
             [[contestants]]\nharness = \"codex\"\nmodel = \"gpt-5.6-luna\"\n"
        ),
    )
    .unwrap();
    let config = RunConfig::load(&path).unwrap();
    let kits = kits::build(
        &env.config_dir(),
        &config,
        &env.tmp.path().join("kits"),
        &FakeBackend::default(),
    )
    .unwrap();
    let workspaces = env.tmp.path().join("runs").join(RUN_ID);
    let meta = env.tmp.path().join("meta").join(RUN_ID);
    std::fs::create_dir_all(&workspaces).unwrap();
    std::fs::create_dir_all(&meta).unwrap();
    Setup {
        _env: env,
        config,
        kits,
        roots: RunRoots {
            id: RUN_ID.into(),
            meta,
            workspaces: workspaces.clone(),
        },
        workspaces,
    }
}

fn scripted() -> FakeBackend {
    FakeBackend::default()
        .with_exec_output_for(&name(0, 0), out(CLAUDE_PONG))
        .with_exec_output_for(&name(1, 0), out(CODEX_PONG))
}

fn execute(s: &Setup, backend: &FakeBackend) -> Vec<PairOutcome> {
    orchestrate::execute(backend, &s.config, &s.kits, &s.roots)
}

/// The calls made for one sandbox, in order, from the backend's log.
fn calls_for(backend: &FakeBackend, sandbox: &str) -> Vec<String> {
    backend
        .log()
        .into_iter()
        .filter(|l| l.ends_with(sandbox))
        .map(|l| l.split(' ').next().unwrap().to_owned())
        .collect()
}

#[test]
fn each_pair_gets_a_sandbox_that_is_created_used_and_removed() {
    let s = setup("");
    let backend = scripted();

    let outcomes = execute(&s, &backend);

    assert_eq!(outcomes.len(), 2);
    for i in 0..2 {
        assert_eq!(calls_for(&backend, &name(i, 0)), ["create", "exec", "rm"]);
    }
    assert_eq!(backend.creates().len(), 2);
}

#[test]
fn the_sandbox_is_sized_named_and_given_the_right_kits() {
    let s = setup("[run]\ncpus = 2\nmemory = \"3g\"\n\n");
    let backend = scripted();

    execute(&s, &backend);

    let mut creates = backend.creates();
    creates.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, (create, harness)) in creates
        .iter()
        .zip([Harness::Claude, Harness::Codex])
        .enumerate()
    {
        assert_eq!(create.name, name(i, 0));
        assert_eq!(create.agent, harness.agent_arg());
        assert_eq!(create.workspace, s.workspaces.join(i.to_string()).join("0"));
        assert_eq!((create.cpus, create.memory.as_str()), (2, "3g"));
        assert_eq!(create.skills, SkillsStore::ReadOnly);
        assert_eq!(create.kits, s.kits.get(harness).unwrap().dirs);
    }
}

#[test]
fn the_prompt_model_timeout_and_workdir_reach_the_headless_command() {
    let s = setup("[run]\ntimeout = \"2m\"\nbudget_usd = 1.5\n\n");
    let backend = scripted();

    execute(&s, &backend);

    let execs = backend.execs();
    let claude = &execs.iter().find(|(n, _)| *n == name(0, 0)).unwrap().1;
    assert_eq!(
        &claude.argv[..4],
        ["timeout", "-v", "--kill-after=10", "120"]
    );
    assert!(claude.argv.contains(&"Reply with exactly: PONG".to_owned()));
    assert!(
        claude
            .argv
            .contains(&"claude-haiku-4-5-20251001".to_owned())
    );
    assert!(claude.argv.contains(&"--max-budget-usd".to_owned()));
    assert_eq!(claude.stdin, Stdin::Closed);
    assert_eq!(
        claude.workdir,
        Some(in_sandbox_path(&s.workspaces.join("0").join("0")))
    );

    // An unseeded workspace isn't a git repo (decision 113); Codex has no budget flag.
    let codex = &execs.iter().find(|(n, _)| *n == name(1, 0)).unwrap().1;
    assert!(codex.argv.contains(&"--skip-git-repo-check".to_owned()));
    assert!(!codex.argv.iter().any(|a| a.contains("budget")));
    assert_eq!(codex.stdin, Stdin::Piped(String::new()));
}

#[test]
fn outcomes_carry_each_contestants_parsed_result_in_contestant_order() {
    let s = setup("");

    let outcomes = execute(&s, &scripted());

    let results: Vec<_> = outcomes
        .iter()
        .map(|o| o.result.as_ref().unwrap())
        .collect();
    assert_eq!(results[0].answer, "PONG");
    assert_eq!(results[0].status, RunStatus::Completed);
    assert_eq!(results[1].answer, "PONG");
    assert_eq!(
        outcomes
            .iter()
            .map(|o| (o.contestant, o.repeat))
            .collect::<Vec<_>>(),
        [(0, 0), (1, 0)]
    );
    assert_eq!(outcomes[0].sandbox, name(0, 0));
    assert!(outcomes.iter().all(|o| o.remove_error.is_none()));
}

#[test]
fn workspaces_are_created_and_kept() {
    let s = setup("");

    execute(&s, &scripted());

    assert!(s.workspaces.join("0").join("0").is_dir());
    assert!(s.workspaces.join("1").join("0").is_dir());
}

#[test]
fn repeat_two_makes_two_sandboxes_per_contestant_ordered_by_wave() {
    let s = setup("[run]\nrepeat = 2\n\n");
    let backend = FakeBackend::default().with_default_exec_output(out(CLAUDE_PONG));

    let outcomes = execute(&s, &backend);

    let mut created: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
    created.sort();
    assert_eq!(created, [name(0, 0), name(0, 1), name(1, 0), name(1, 1)]);
    assert_eq!(
        outcomes
            .iter()
            .map(|o| (o.repeat, o.contestant))
            .collect::<Vec<_>>(),
        [(0, 0), (0, 1), (1, 0), (1, 1)]
    );
    assert_eq!(backend.removes().len(), 4);
}

#[test]
fn wave_two_starts_only_after_every_wave_one_pair_has_finished() {
    let s = setup("[run]\nrepeat = 2\n\n");
    let (backend, gate) = FakeBackend::default()
        .with_default_exec_output(out(CLAUDE_PONG))
        .with_exec_gate();

    std::thread::scope(|scope| {
        let run = scope.spawn(|| execute(&s, &backend));

        // Both wave-one pairs are inside `exec`; nothing of wave two exists.
        gate.wait_for_blocked(2);
        let mut now: Vec<String> = backend.creates().into_iter().map(|c| c.name).collect();
        now.sort();
        assert_eq!(now, [name(0, 0), name(1, 0)]);
        assert!(backend.removes().is_empty());

        gate.open();
        assert_eq!(run.join().unwrap().len(), 4);
    });

    let log = backend.log();
    let last_wave_one_rm = log
        .iter()
        .rposition(|l| l == &format!("rm {}", name(0, 0)) || l == &format!("rm {}", name(1, 0)))
        .unwrap();
    let first_wave_two_create = log
        .iter()
        .position(|l| {
            l == &format!("create {}", name(0, 1)) || l == &format!("create {}", name(1, 1))
        })
        .unwrap();
    assert!(last_wave_one_rm < first_wave_two_create, "{log:?}");
}

#[test]
fn contestants_of_a_wave_run_at_the_same_time() {
    // Both are blocked inside exec together: a sequential loop would hang the gate.
    let s = setup("");
    let (backend, gate) = FakeBackend::default().with_exec_gate();

    std::thread::scope(|scope| {
        let run = scope.spawn(|| execute(&s, &backend));
        gate.wait_for_blocked(2);
        gate.open();
        run.join().unwrap();
    });
}

#[test]
fn a_failed_create_leaves_the_other_pair_untouched_and_is_still_cleaned_up() {
    let s = setup("");
    let backend = scripted().with_failing_create_for(&name(0, 0));

    let outcomes = execute(&s, &backend);

    let err = outcomes[0].result.as_ref().unwrap_err();
    assert!(err.contains("create") && err.contains(&name(0, 0)), "{err}");
    assert_eq!(outcomes[1].result.as_ref().unwrap().answer, "PONG");
    // The failed pair never reached exec, but its half-made sandbox is removed too.
    assert_eq!(calls_for(&backend, &name(0, 0)), ["create", "rm"]);
    assert_eq!(calls_for(&backend, &name(1, 0)), ["create", "exec", "rm"]);
}

#[test]
fn a_backend_error_during_exec_is_reported_and_the_sandbox_removed() {
    let s = setup("");
    let backend = scripted().with_failing_exec_for(&name(1, 0));

    let outcomes = execute(&s, &backend);

    assert!(outcomes[0].result.is_ok());
    let err = outcomes[1].result.as_ref().unwrap_err();
    assert!(err.contains("exec") && err.contains(&name(1, 0)), "{err}");
    assert_eq!(calls_for(&backend, &name(1, 0)), ["create", "exec", "rm"]);
}

#[test]
fn a_timed_out_pair_is_ok_with_partial_output_and_still_removed() {
    let s = setup("");
    let backend = scripted().with_exec_output_for(
        &name(1, 0),
        ExecOutput {
            stdout: CODEX_TRUNCATED.into(),
            stderr: "timeout: sending signal TERM to command 'codex'\n".into(),
            exit_code: Some(124),
        },
    );

    let outcomes = execute(&s, &backend);

    let timed_out = outcomes[1].result.as_ref().unwrap();
    assert_eq!(timed_out.status, RunStatus::TimedOut);
    assert!(
        timed_out.answer.starts_with("I\u{2019}ll create"),
        "{}",
        timed_out.answer
    );
    assert_eq!(calls_for(&backend, &name(1, 0)), ["create", "exec", "rm"]);
}

#[test]
fn a_failed_removal_is_reported_but_keeps_the_result() {
    let s = setup("");
    let backend = FakeBackend::failing_remove().with_default_exec_output(out(CLAUDE_PONG));

    let outcomes = execute(&s, &backend);

    for outcome in &outcomes {
        assert!(outcome.result.is_ok());
        let err = outcome.remove_error.as_ref().unwrap();
        assert!(
            err.contains("remove") && err.contains(&outcome.sandbox),
            "{err}"
        );
    }
}

#[test]
fn host_paths_map_to_the_sandbox_mount() {
    assert_eq!(
        in_sandbox_path(Path::new(r"E:\sbxm-it\runs\x")),
        PathBuf::from("/e/sbxm-it/runs/x")
    );
    assert_eq!(
        in_sandbox_path(Path::new("/already/unix")),
        PathBuf::from("/already/unix")
    );
    assert_eq!(in_sandbox_path(Path::new(r"d:\a")), PathBuf::from("/d/a"));
}

#[test]
fn the_orchestrator_leaves_the_pair_workspaces_empty() {
    // Seeding is slice 7; here a workspace is only created, not filled.
    let s = setup("");

    execute(&s, &scripted());

    assert!(
        s.workspaces
            .join("0")
            .join("0")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
}
