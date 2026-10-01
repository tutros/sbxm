//! M2a slice 7: the orchestrator seeds each contestant's workspace and
//! captures its diff (decisions 14, 102, 113). The fake backend's exec hook
//! plays the agent by changing the host workspace while the "sandbox" runs.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::Env;
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend};
use sbxm::run::config::RunConfig;
use sbxm::run::id::RunRoots;
use sbxm::run::kits::{self, RunKits};
use sbxm::run::orchestrate::{self, PairOutcome};

const RUN_ID: &str = "2026-09-30-abc123";
const CLAUDE_PONG: &str = include_str!("../src/fixtures/claude-stream-json-pong.jsonl");

fn name(contestant: usize, repeat: u32) -> String {
    format!("sbxm-run-{RUN_ID}-{contestant}-{repeat}")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

struct Setup {
    env: Env,
    config: RunConfig,
    kits: RunKits,
    roots: RunRoots,
}

impl Setup {
    fn workspace(&self, contestant: usize, repeat: u32) -> PathBuf {
        self.roots
            .workspaces
            .join(contestant.to_string())
            .join(repeat.to_string())
    }
}

/// Two Claude contestants; a seed with `a.txt` and `sub/b.txt` when `seeded`.
fn setup(seeded: bool, run_table: &str) -> Setup {
    let env = Env::new();
    let seed = if seeded {
        let seed = env.tmp.path().join("seed");
        write(&seed.join("a.txt"), "alpha\n");
        write(&seed.join("sub").join("b.txt"), "bravo\n");
        Some(seed)
    } else {
        None
    };
    let seed_line = seed
        .map(|s| {
            format!(
                "seed = {}\n",
                toml::Value::String(s.to_str().unwrap().to_owned())
            )
        })
        .unwrap_or_default();
    let path = env.tmp.path().join("run.toml");
    std::fs::write(
        &path,
        format!(
            "[task]\nprompt = \"p\"\n{seed_line}\n{run_table}\
             [[contestants]]\nharness = \"claude\"\nmodel = \"one\"\n\n\
             [[contestants]]\nharness = \"claude\"\nmodel = \"two\"\n"
        ),
    )
    .unwrap();
    let config = RunConfig::load(&path).unwrap();
    let meta = env.tmp.path().join("meta").join(RUN_ID);
    let workspaces = env.tmp.path().join("runs").join(RUN_ID);
    std::fs::create_dir_all(&meta).unwrap();
    std::fs::create_dir_all(&workspaces).unwrap();
    let kits = kits::build(
        &env.config_dir(),
        &config,
        &meta.join("kits"),
        &FakeBackend::default(),
    )
    .unwrap();
    Setup {
        env,
        config,
        kits,
        roots: RunRoots {
            id: RUN_ID.into(),
            meta,
            workspaces,
        },
    }
}

fn pong() -> FakeBackend {
    FakeBackend::default().with_default_exec_output(ExecOutput {
        stdout: CLAUDE_PONG.into(),
        stderr: String::new(),
        exit_code: Some(0),
    })
}

fn execute(s: &Setup, backend: &FakeBackend) -> Vec<PairOutcome> {
    orchestrate::execute(backend, &s.config, &s.kits, &s.roots)
}

/// Plays the agent running git, so like sbxm's own git calls it waits out the
/// brief Windows "Permission denied" while antivirus or the indexer holds a new
/// file (issue #39).
fn git(dir: &Path, args: &[&str]) -> String {
    let mut tries = 0;
    let out = loop {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        tries += 1;
        if out.status.success()
            || tries == 5
            || !String::from_utf8_lossy(&out.stderr).contains("Permission denied")
        {
            break out;
        }
        std::thread::sleep(std::time::Duration::from_millis(50 << tries));
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn patch(outcome: &PairOutcome) -> String {
    outcome
        .diff
        .as_ref()
        .expect("a diff was captured")
        .as_ref()
        .unwrap()
        .patch
        .clone()
}

#[test]
fn a_seeded_pair_starts_from_a_fresh_repo_with_one_baseline_commit() {
    let s = setup(true, "");

    execute(&s, &pong());

    for i in 0..2 {
        let ws = s.workspace(i, 0);
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "alpha\n"
        );
        assert_eq!(git(&ws, &["rev-list", "--count", "HEAD"]), "1");
    }
}

#[test]
fn a_seeded_pairs_diff_holds_what_the_agent_changed() {
    let s = setup(true, "");
    let ws0 = s.workspace(0, 0);
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox == name(0, 0) {
            write(&ws0.join("a.txt"), "alpha edited\n");
            write(&ws0.join("new.txt"), "fresh\n");
        }
    });

    let outcomes = execute(&s, &backend);

    let first = patch(&outcomes[0]);
    assert!(
        first.contains("+alpha edited") && first.contains("diff --git a/new.txt b/new.txt"),
        "{first}"
    );
    // The other contestant's copy of the seed is untouched: an empty diff.
    assert_eq!(patch(&outcomes[1]), "");
}

#[test]
fn a_seeded_diff_survives_the_agent_committing_and_wrecking_git() {
    let s = setup(true, "");
    let ws0 = s.workspace(0, 0);
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox == name(0, 0) {
            write(&ws0.join("new.txt"), "fresh\n");
            git(&ws0, &["add", "-A"]);
            git(
                &ws0,
                &[
                    "-c",
                    "user.name=a",
                    "-c",
                    "user.email=a@a",
                    "commit",
                    "-q",
                    "-m",
                    "work",
                ],
            );
            std::fs::remove_dir_all(ws0.join(".git")).unwrap();
        }
    });

    let outcomes = execute(&s, &backend);

    assert!(patch(&outcomes[0]).contains("diff --git a/new.txt b/new.txt"));
}

#[test]
fn an_unseeded_pair_stays_a_plain_folder_and_gets_a_no_index_diff() {
    let s = setup(false, "");
    let ws0 = s.workspace(0, 0);
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox == name(0, 0) {
            write(&ws0.join("made.txt"), "by the agent\n");
            // An agent that runs `git init` and commits must not leak `.git/` paths.
            git(&ws0, &["init", "-q"]);
        }
    });

    let outcomes = execute(&s, &backend);

    let first = patch(&outcomes[0]);
    assert!(
        first.contains("diff --git a/made.txt b/made.txt"),
        "{first}"
    );
    assert!(!first.contains(".git/"), "{first}");
    // Untouched, the workspace is not a repo (decision 113).
    assert!(!s.workspace(1, 0).join(".git").exists());
    assert_eq!(patch(&outcomes[1]), "");
}

#[test]
fn seeded_pairs_do_not_get_the_git_workaround_flag_and_unseeded_ones_do() {
    let codex = |seeded: bool| {
        let s = setup(seeded, "");
        // Swap in a Codex contestant by rewriting the run-config.
        let body = std::fs::read_to_string(s.env.tmp.path().join("run.toml"))
            .unwrap()
            .replace("harness = \"claude\"", "harness = \"codex\"");
        std::fs::write(s.env.tmp.path().join("run.toml"), body).unwrap();
        let config = RunConfig::load(&s.env.tmp.path().join("run.toml")).unwrap();
        let meta = s.roots.meta.clone();
        let kits = kits::build(
            &s.env.config_dir(),
            &config,
            &meta.join("kits2"),
            &FakeBackend::default(),
        )
        .unwrap();
        let backend = FakeBackend::default();
        orchestrate::execute(&backend, &config, &kits, &s.roots);
        backend.execs()[0]
            .1
            .argv
            .contains(&"--skip-git-repo-check".to_owned())
    };

    assert!(!codex(true));
    assert!(codex(false));
}

#[test]
fn repeats_of_a_contestant_get_separate_seeded_workspaces_and_diffs() {
    let s = setup(true, "[run]\nrepeat = 2\n\n");
    let workspaces = [s.workspace(0, 0), s.workspace(0, 1)];
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox == name(0, 0) {
            write(&workspaces[0].join("first.txt"), "one\n");
        }
        if sandbox == name(0, 1) {
            write(&workspaces[1].join("second.txt"), "two\n");
        }
    });

    let outcomes = execute(&s, &backend);

    let of = |contestant: usize, repeat: u32| {
        outcomes
            .iter()
            .find(|o| o.contestant == contestant && o.repeat == repeat)
            .unwrap()
    };
    assert!(patch(of(0, 0)).contains("first.txt") && !patch(of(0, 0)).contains("second.txt"));
    assert!(patch(of(0, 1)).contains("second.txt") && !patch(of(0, 1)).contains("first.txt"));
    assert_eq!(patch(of(1, 0)), "");
}

#[test]
fn the_hosts_baseline_repos_are_cleaned_up_after_the_diff() {
    let s = setup(true, "");

    execute(&s, &pong());

    let work = s.roots.meta.join("work");
    assert!(!work.exists() || std::fs::read_dir(&work).unwrap().next().is_none());
}

#[test]
fn a_seed_that_disappeared_fails_the_pair_before_any_sandbox_exists() {
    let s = setup(true, "");
    std::fs::remove_dir_all(s.env.tmp.path().join("seed")).unwrap();
    let backend = pong();

    let outcomes = execute(&s, &backend);

    for outcome in &outcomes {
        let err = outcome.result.as_ref().unwrap_err();
        assert!(err.contains("cannot seed workspace"), "{err}");
        assert!(outcome.diff.is_none());
    }
    // Nothing was created, run or removed.
    assert!(backend.log().is_empty(), "{:?}", backend.log());
}

#[test]
fn a_timed_out_pair_still_gets_its_diff() {
    let s = setup(false, "");
    let ws0 = s.workspace(0, 0);
    let backend = FakeBackend::default()
        .with_default_exec_output(ExecOutput {
            stdout: CLAUDE_PONG.into(),
            stderr: "timeout: sending signal TERM to command 'claude'\n".into(),
            exit_code: Some(124),
        })
        .with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
            if sandbox == name(0, 0) {
                write(&ws0.join("partial.txt"), "half done\n");
            }
        });

    let outcomes = execute(&s, &backend);

    assert!(patch(&outcomes[0]).contains("partial.txt"));
}

#[test]
fn a_diff_failure_is_reported_without_losing_the_result() {
    let s = setup(false, "");
    let ws0 = s.workspace(0, 0);
    let backend = pong().with_exec_hook(move |sandbox: &str, _: &ExecSpec| {
        if sandbox == name(0, 0) {
            std::fs::remove_dir_all(&ws0).unwrap();
        }
    });

    let outcomes = execute(&s, &backend);

    assert!(outcomes[0].result.is_ok());
    let err = outcomes[0].diff.as_ref().unwrap().as_ref().unwrap_err();
    assert!(err.contains("cannot capture the diff"), "{err}");
    assert!(outcomes[1].diff.as_ref().unwrap().is_ok());
}

#[test]
fn a_failed_create_captures_no_diff_but_a_failed_exec_does() {
    let s = setup(false, "");
    let backend = pong()
        .with_failing_create_for(&name(0, 0))
        .with_failing_exec_for(&name(1, 0));

    let outcomes = execute(&s, &backend);

    assert!(outcomes[0].diff.is_none());
    assert!(outcomes[1].result.is_err() && outcomes[1].diff.is_some());
}
