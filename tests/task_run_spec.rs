//! Issue 194 (decision 174(d)): `sbxm task run --spec FILE` is `start --spec` then `review --spec`
//! for one spec task, and stops before `finish`, the same way `task run --issue N` does.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, CLEAN_REVIEW, CODEX_DONE, Fixture, Play, Probe, fixture_with, ok, play,
    play_reviews, source, spec_file,
};
use sbxm::backend::{ExecOutput, FakeBackend};
use sbxm::commands::task_finish;
use sbxm::commands::task_run::{SpecOptions, run_spec};
use sbxm::commands::task_start::Restart;
use sbxm::confirm::Confirm;
use sbxm::github::fake::FakeGitHub;
use sbxm::task::gates::FakeHostRunner;
use sbxm::task::record::{self, Stage, Status};
use sbxm::task::repo::Identity;

fn config() -> Fixture {
    fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n",
    )
}

fn spec(f: &Fixture) -> PathBuf {
    spec_file(f, "Build a thing.\n")
}

fn id_of(spec: &Path) -> String {
    record::spec_id(spec).unwrap().0
}

fn backend_for(f: &Fixture, id: &str, reviews: &[&str]) -> FakeBackend {
    let worker = play(
        &f.env.base_dir().join("tasks").join(id),
        "main",
        id,
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let reviewer = play_reviews(
        &f.env.base_dir(),
        id,
        reviews.iter().map(|s| (*s).to_owned()).collect(),
    );
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, exec| {
            worker(sandbox, exec);
            reviewer(sandbox, exec);
        })
}

fn count(backend: &FakeBackend, needle: &str) -> usize {
    backend
        .execs()
        .iter()
        .filter(|(_, spec)| spec.argv.iter().any(|a| a.contains(needle)))
        .count()
}

fn options(f: &Fixture, spec: PathBuf) -> SpecOptions {
    SpecOptions {
        repo_root: f.env.tmp.path().join("target-repo"),
        spec,
        worker_harness: None,
        worker_model: None,
        reviewer_harness: None,
        reviewer_model: None,
        time_limit: None,
        reviewer_time_limit: None,
        profile: None,
        base: Some("main".into()),
        repo: Some("o/r".into()),
        clone_source: Some(source(f)),
        identity: Some(Identity {
            name: "Dev".into(),
            email: "dev@example.com".into(),
        }),
        disk: Default::default(),
    }
}

struct Out {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn go(
    f: &Fixture,
    opts: &SpecOptions,
    restart: Option<&Restart>,
    backend: &FakeBackend,
    github: &FakeGitHub,
) -> Out {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = run_spec(
        &f.env.config_dir(),
        opts,
        restart,
        backend,
        github,
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    Out {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn stage(f: &Fixture, id: &str) -> (Stage, Status) {
    let task = record::read(&record::task_dir(&f.env.base_dir(), id).join("task.json")).unwrap();
    (task.stage, task.status)
}

#[test]
fn it_starts_then_reviews_and_stops_before_finish() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let github = FakeGitHub::default();

    let out = go(&f, &options(&f, s.clone()), None, &b, &github);

    out.result.unwrap();
    let text = &out.out;
    let worker = text.find(&format!("{id}: worker completed")).expect(text);
    let review = text.find(&format!("{id}: review round 1")).expect(text);
    assert!(worker < review, "{text}");
    assert!(text.contains(&format!("{id}: ready")), "{text}");
    // Only the last step's hint is left, and it is the finish command.
    assert!(!text.contains("sbxm task review --spec"), "{text}");
    assert!(
        text.contains(&format!("next: sbxm task finish --spec {}", s.display())),
        "{text}"
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
    // No GitHub call anywhere in this path.
    assert!(github.calls().is_empty(), "{:?}", github.calls());
}

/// Issue 140, decision 180(b): a post-worker gate failure doesn't stop `run --spec`; the review
/// reruns the gates and gives the failure to a fix round.
#[test]
fn failing_gates_after_the_worker_go_on_to_the_review_and_its_fix_round() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let fixed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]).with_exec_responder(move |_, spec| {
        let mentions = |needle: &str| spec.argv.iter().any(|a| a.contains(needle));
        if mentions("fix-prompt.md") {
            fixed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        (mentions("cargo test") && !fixed.load(std::sync::atomic::Ordering::SeqCst)).then(|| {
            ExecOutput {
                stdout: String::new(),
                stderr: "undefined reference to `oops`\n".into(),
                exit_code: Some(101),
            }
        })
    });

    let out = go(
        &f,
        &options(&f, s.clone()),
        None,
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    let text = &out.out;
    let failed = text
        .find("gates: failed: `cargo test` (sandbox, exit 101)")
        .expect(text);
    assert!(!text.contains("next: sbxm task review"), "{text}");
    let review = text.find(&format!("{id}: review round 2")).expect(text);
    assert!(failed < review, "{text}");
    assert!(text.contains("the fix round ran 1 time(s)"), "{text}");
    assert!(
        text.contains(&format!("next: sbxm task finish --spec {}", s.display())),
        "{text}"
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
}

/// Issue 186: unlike a gate that ran and failed, one that could not run at all gives the review
/// nothing to react to, so `task run --spec` stops after the start phase instead of going on to it.
#[test]
fn a_sandbox_gate_that_cannot_run_stops_before_any_reviewer() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]).with_failing_exec_matching("cargo test");

    let out = go(
        &f,
        &options(&f, s.clone()),
        None,
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("gates could not run"), "{message}");
    assert!(out.out.contains("gates: could not run"), "{}", out.out);
    assert!(!out.out.contains("next: sbxm task review"), "{}", out.out);
    assert_eq!(stage(&f, &id), (Stage::Gating, Status::GatesFailed));
    assert_eq!(count(&b, "codex"), 0, "no reviewer ran");
    assert_eq!(b.creates().len(), 1, "no reviewer sandbox");
}

struct Yes;

impl Confirm for Yes {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

/// Decision 185(b), issue #207 review M-1: `task run --restart --spec` settles the old task's
/// set-aside copy the moment the new task's preparation completes — before its worker runs — not
/// only once the whole `start` phase (worker, then its gates) has also finished. A gate holds the
/// worker's one `exec` call, so the assertions run while the command is still blocked there.
#[test]
fn restart_settles_the_old_task_before_its_worker_runs() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    // The first run's worker fails before committing anything, so its `repo.git` has no commits
    // beyond the base: nothing for decision 178's protection to refuse (see
    // `restart_discards_the_old_task_then_runs`), and `--restart` can discard it right away.
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let (gated, gate) = backend_for(&f, &id, &[CLEAN_REVIEW]).with_exec_gate();

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            go(
                &f,
                &options(&f, s.clone()),
                Some(&Restart {
                    confirm: &Yes,
                    yes: false,
                }),
                &gated,
                &FakeGitHub::default(),
            )
        });

        gate.wait_for_blocked(1);
        assert!(
            record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).is_ok(),
            "the new task's preparation has completed"
        );
        assert!(
            !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
            "settled before the worker ran, not left set aside"
        );
        gate.open();

        let out = handle.join().unwrap();
        out.result.unwrap_or_else(|e| panic!("{e:#}: {}", out.out));
    });
}

/// A `Write` that always fails, modeling a warning writer whose output can't be delivered (e.g. a
/// closed pipe).
struct FailingWrite;

impl std::io::Write for FailingWrite {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("warning output failed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Issue #207 review round 2, M-1: `run_spec_from` wrote the newly prepared task's own warnings
/// with `?` *before* calling `on_prepared`, so a warning writer that fails stops the function
/// there and the old task's set-aside copy is never settled, even though preparation (folder,
/// kits, sandbox, `task.json`) already completed — exactly the boundary decision 185(b) requires.
/// Reproduced with a worker harness (codex) that can't apply the profile's
/// `harness.claude.managed_settings` (so `prepared.warnings`, only known once preparation
/// succeeds, is non-empty) and a `warn` writer that always errors. The reviewer is left at its
/// default (picked to differ from the worker automatically), so no same-harness warning — which
/// `config.warnings` would already carry *before* preparation — masks the one this test is about.
#[test]
fn restart_settles_after_preparation_even_if_printing_a_warning_fails() {
    let f =
        fixture_with("[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n");
    f.env.write_profile(
        "default",
        "[harness.claude]\nmanaged_settings = \"claude/managed.json\"\n",
    );
    let managed = f.env.profiles_dir().join("default").join("claude");
    fs::create_dir_all(&managed).unwrap();
    fs::write(managed.join("managed.json"), "{}").unwrap();

    let s = spec(&f);
    let id = id_of(&s);
    let mut opts = options(&f, s.clone());
    opts.worker_harness = Some(sbxm::harness::Harness::Codex);

    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "codex",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let first = run_spec(
        &f.env.config_dir(),
        &opts,
        None,
        &failing,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut warn,
    );
    assert!(first.is_err());

    let gated = FakeBackend::with_secrets(&["anthropic", "openai"]);
    let mut out = Vec::new();
    let result = run_spec(
        &f.env.config_dir(),
        &opts,
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &gated,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut FailingWrite,
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("warning output failed"), "{message}");
    assert!(
        record::read(&record::task_dir(&f.env.base_dir(), &id).join("task.json")).is_ok(),
        "the new task's preparation has completed"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
        "settled once prepared, even though printing its warning failed"
    );
}

/// The first run's worker fails before committing anything, so its `repo.git` has no commits
/// beyond the base: nothing for decision 178's protection to refuse (see the next test for the
/// case where it does), and `--restart` can discard it and run again right away.
#[test]
fn restart_discards_the_old_task_then_runs() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    assert!(
        out.out
            .contains(&format!("removed sandbox sbxm-task-{id}-claude")),
        "{}",
        out.out
    );
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
    // The printed `task finish --spec FILE` works on the restarted task.
    assert!(
        out.out
            .contains(&format!("next: sbxm task finish --spec {}", s.display())),
        "{}",
        out.out
    );
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(s.clone()),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
}

/// Decision 178: a finished spec task's result lives only in its `repo.git` until fetched, so
/// `--restart` protects it exactly as `task rm --spec` does, since it has no `--force` of its own.
#[test]
fn restart_refuses_to_discard_an_unfetched_finished_result() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &backend_for(&f, &id, &[CLEAN_REVIEW]),
        &FakeGitHub::default(),
    );
    first.result.unwrap();
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: task_finish::Target::Spec(s.clone()),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &FakeGitHub::default(),
        &Probe,
        &mut Vec::new(),
    )
    .unwrap();
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--force"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task's result was not deleted"
    );
}

/// Decision 185 point 4: `task run --spec --restart` refuses before the prompt while a leftover
/// copy of the task exists under `tasks-replaced/`, naming the folder and the two ways out;
/// nothing is deleted or created.
#[test]
fn restart_refuses_before_the_prompt_while_a_leftover_copy_exists() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &backend_for(&f, &id, &[CLEAN_REVIEW]),
        &FakeGitHub::default(),
    );
    first.result.unwrap();
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    // An earlier `--restart` that was interrupted after discarding but before settling.
    sbxm::task::finish::set_aside(
        &f.env.base_dir(),
        &id,
        true,
        &b,
        &Probe,
        &Yes,
        &mut Vec::new(),
    )
    .unwrap();
    let (removes_before, creates_before) = (b.removes().len(), b.creates().len());

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("tasks-replaced"), "{message}");
    assert!(message.contains("move it back"), "{message}");
    assert!(message.contains("delete it"), "{message}");
    assert_eq!(b.removes().len(), removes_before, "nothing more removed");
    assert_eq!(b.creates().len(), creates_before, "nothing created");
}

/// Issue #194 review finding M-1: `--restart` must not discard the old task before the new
/// start's own inputs are validated, the same as it already doesn't for `--issue`.
#[test]
fn restart_validates_the_new_start_before_discarding_the_old_task() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s.clone());
    opts.time_limit = Some("soon".into());

    let out = go(
        &f,
        &opts,
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("--time-limit"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was deleted before the new start's inputs were validated"
    );
}

/// A `Confirm` that makes the spec file unusable before saying yes (`validate_spec_file` rejects
/// non-UTF-8).
struct CorruptThenYes<'a> {
    spec: &'a Path,
}

impl Confirm for CorruptThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        fs::write(self.spec, [0xff, 0xfe, 0xfd]).unwrap();
        Ok(true)
    }
}

/// Decision 185 (issue #207, supersedes decision 184's "nothing after a confirmed discard"): the
/// spec is read again after a confirmed discard, so a file that became unusable while the prompt
/// was pending fails the start there; the old task, set aside rather than deleted, is put back
/// instead of being lost.
#[test]
fn restart_fails_after_the_discard_if_the_spec_becomes_unusable_during_confirmation() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &CorruptThenYes { spec: &s },
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("the old task was discarded"), "{message}");
    assert!(message.contains("put back"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was put back"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
        "nothing left set aside"
    );
}

/// A `Confirm` that removes the spec file before saying yes: `task finish|resume|rm --spec FILE`
/// could not address a task started under that path's id afterwards.
struct RemoveThenYes<'a> {
    spec: &'a Path,
}

impl Confirm for RemoveThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        fs::remove_file(self.spec).unwrap();
        Ok(true)
    }
}

/// Decision 185 (issue #207, supersedes decision 184's "nothing after a confirmed discard"): a
/// spec removed while the prompt was pending fails the start after the discard, and the old task,
/// set aside rather than deleted, is put back instead of being lost. (Before decision 185, a task
/// started under the old id would have ended `ready` with a `task finish --spec FILE` hint that
/// cannot work, since every spec command derives the id from the path again; now there is no such
/// task at all.)
#[test]
fn restart_fails_after_the_discard_if_the_spec_is_removed_during_confirmation() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &RemoveThenYes { spec: &s },
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("the old task was discarded"), "{message}");
    assert!(message.contains("put back"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was put back"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
        "nothing left set aside"
    );
}

/// A `Confirm` that removes the worker's secret from the backend right when the user says yes,
/// modeling it vanishing while the confirmation prompt is pending (issue #207's original finding,
/// issue 194's review M-1): the reviewer's and worker's secrets are both still stored for the
/// before-the-prompt checks, but only the reviewer's is left by the time `prepare_spec_resolved`
/// checks the worker's secret again, after the discard.
struct SecretVanishesThenYes<'a> {
    backend: &'a FakeBackend,
}

impl Confirm for SecretVanishesThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        self.backend.set_secrets(&["openai"]);
        Ok(true)
    }
}

/// Decision 185 (issue #207): the worker's secret disappearing while `task run --spec --restart`
/// waits for confirmation must not cost the old task its result. Before the fix, `prepare_spec_resolved`'s
/// second secret check (after the discard) failed on the missing secret and the old task was
/// already gone; now it is put back.
#[test]
fn restart_fails_after_the_discard_if_the_worker_secret_vanishes_during_confirmation() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = FakeBackend::with_secrets(&["anthropic", "openai"]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &SecretVanishesThenYes { backend: &b },
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("anthropic"), "{message}");
    assert!(message.contains("put back"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was put back"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
        "nothing left set aside"
    );
}

/// Issue #194 review finding M-2: `--restart` must not discard the old task before the new
/// start's worker secret is checked, the same as the reviewer's already is.
#[test]
fn restart_validates_the_worker_secret_before_discarding_the_old_task() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    // Only the reviewer's secret ("openai", codex) is stored; the worker's ("anthropic", claude)
    // is missing.
    let b = FakeBackend::with_secrets(&["openai"]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("anthropic"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was deleted before the worker's secret was validated"
    );
}

/// PR 208 review round 3, M-1: a custom worker prompt that cannot be rendered for a spec task is
/// known from the config alone, so it is refused before the prompt and the old task stays
/// (decision 184).
#[test]
fn restart_validates_the_worker_prompt_before_discarding_the_old_task() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let repo_root = f.env.tmp.path().join("target-repo");
    fs::write(repo_root.join("worker.md"), "Work on {{issue}}.\n").unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"codex\"\n\n[prompts]\nworker = \"worker.md\"\n",
    )
    .unwrap();
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    let message = format!("{:#}", out.result.unwrap_err());
    assert!(message.contains("has no value here"), "{message}");
    assert!(b.creates().is_empty(), "nothing started");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task was deleted before the worker prompt was validated"
    );
}

/// A `Confirm` that edits the spec file, validly, before saying yes.
struct EditThenYes<'a> {
    spec: &'a Path,
}

impl Confirm for EditThenYes<'_> {
    fn is_interactive(&self) -> bool {
        true
    }

    fn confirm(&self, _prompt: &str) -> anyhow::Result<bool> {
        fs::write(self.spec, "Build a different thing.\n").unwrap();
        Ok(true)
    }
}

/// PR 208 review round 3, S-2: the spec is read again after the discard, so a valid edit at the
/// same path made while the prompt was pending is what the new task starts from.
#[test]
fn restart_uses_a_valid_spec_change_made_during_confirmation() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(
        &f,
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &EditThenYes { spec: &s },
            yes: false,
        }),
        &b,
        &FakeGitHub::default(),
    );

    out.result.unwrap();
    assert_eq!(stage(&f, &id), (Stage::Ready, Status::Ok));
    let source =
        fs::read_to_string(record::task_dir(&f.env.base_dir(), &id).join("source.md")).unwrap();
    assert_eq!(source, "Build a different thing.\n");
}

/// `start --spec` and `review --spec` would both warn about the same-harness reviewer; `run
/// --spec` says it once (the `Once` writer, as for `--issue`).
#[test]
fn the_configs_warnings_are_said_once() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
         [reviewer]\nharness = \"claude\"\n",
    );
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);

    let out = go(&f, &options(&f, s), None, &b, &FakeGitHub::default());

    let _ = out.result;
    assert!(out.warn.contains("warning:"), "{}", out.warn);
    let lines: Vec<&str> = out.warn.lines().collect();
    let mut unique = lines.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(lines.len(), unique.len(), "{}", out.warn);
}

#[test]
fn a_bad_reviewer_flag_is_refused_before_the_worker_starts() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s);
    opts.reviewer_time_limit = Some("soon".into());

    let out = go(&f, &opts, None, &b, &FakeGitHub::default());

    assert!(out.result.is_err());
    assert!(b.creates().is_empty());
}

#[test]
fn an_empty_or_blank_reviewer_model_flag_is_refused_before_the_worker_starts() {
    for bad in ["", "  "] {
        let f = config();
        let s = spec(&f);
        let id = id_of(&s);
        let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
        let mut opts = options(&f, s);
        opts.reviewer_model = Some(bad.into());
        let gh = FakeGitHub::default();

        let out = go(&f, &opts, None, &b, &gh);

        let message = format!("{:#}", out.result.unwrap_err());
        assert!(message.contains("--reviewer-model"), "{bad:?}: {message}");
        assert!(b.creates().is_empty(), "{bad:?}");
        assert_eq!(b.secret_service_calls(), 0, "{bad:?}");
        assert!(gh.calls().is_empty(), "{bad:?}");
    }
}

/// A `Write` that takes its first line and fails from then on.
#[derive(Default)]
struct FailsAfterFirstLine(String);

impl std::io::Write for FailsAfterFirstLine {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.contains('\n') {
            return Err(std::io::Error::other("screen output failed"));
        }
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// PR 219 review, M-2: screen output cannot interrupt a restart. The new task's preparation fails
/// (its sandbox can't be made) and the screen stops taking output after the discard's first line:
/// the old task is still put back.
#[test]
fn restart_puts_the_old_task_back_even_when_the_screen_stops_taking_output() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let failing = FakeBackend::with_secrets(&["anthropic", "openai"]).with_exec_output_matching(
        "claude",
        ExecOutput {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: Some(1),
        },
    );
    let first = go(
        &f,
        &options(&f, s.clone()),
        None,
        &failing,
        &FakeGitHub::default(),
    );
    assert!(first.result.is_err());

    let no_sandbox = FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_failing_create_for(&format!("sbxm-task-{id}-claude"));
    let mut out = FailsAfterFirstLine::default();
    let result = run_spec(
        &f.env.config_dir(),
        &options(&f, s.clone()),
        Some(&Restart {
            confirm: &Yes,
            yes: false,
        }),
        &no_sandbox,
        &FakeGitHub::default(),
        &Probe,
        &FakeHostRunner::default(),
        &mut out,
        &mut Vec::new(),
    );

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("put back"), "{message}");
    assert!(
        record::task_dir(&f.env.base_dir(), &id)
            .join("repo.git")
            .is_dir(),
        "the old task is back: {message}"
    );
    assert!(
        !sbxm::task::finish::has_leftover(&f.env.base_dir(), &id),
        "{message}"
    );
}

// Issue 210: a low sandbox drive refuses before anything is created.

#[test]
fn a_low_sandbox_drive_refuses_before_anything_is_created() {
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s);
    opts.disk = common::low_disk(&f.env);

    let out = go(&f, &opts, None, &b, &FakeGitHub::default());

    let err = format!("{:#}", out.result.unwrap_err());
    assert!(err.starts_with(common::LOW_DISK), "{err}");
    assert!(b.log().is_empty(), "{:?}", b.log());
    assert_eq!(b.secret_service_calls(), 0);
    assert!(!record::task_dir(&f.env.base_dir(), &id).exists());
}

/// Issue 210: a drive that fills during the worker stops before the reviewer's sandbox, naming
/// `task review --spec`.
#[test]
fn a_drive_that_fills_during_the_worker_stops_before_the_review_naming_task_review() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static READS: AtomicUsize = AtomicUsize::new(0);
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s.clone());
    // Room for `task run`'s own check (the start reuses it); none left for the review.
    opts.disk = sbxm::disk::Guard {
        free_space: |_| {
            let gib = 1024 * 1024 * 1024;
            Ok(if READS.fetch_add(1, Ordering::SeqCst) < 1 {
                20 * gib
            } else {
                gib
            })
        },
        ..common::low_disk(&f.env)
    };

    let out = go(&f, &opts, None, &b, &FakeGitHub::default());

    let err = format!("{:#}", out.result.unwrap_err());
    assert!(
        err.starts_with(&format!(
            "the worker is done but its review was not started; continue with `sbxm task review \
             --spec {}`: only 1.0 GiB free on the drive of ",
            s.display()
        )),
        "{err}"
    );
    assert_eq!(b.creates().len(), 1, "{:?}", b.log());
}

#[test]
fn the_review_after_the_worker_reads_the_drive_once_so_its_refusal_names_task_review() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static READS: AtomicUsize = AtomicUsize::new(0);
    let f = config();
    let s = spec(&f);
    let id = id_of(&s);
    let b = backend_for(&f, &id, &[CLEAN_REVIEW]);
    let mut opts = options(&f, s);
    opts.disk = sbxm::disk::Guard {
        free_space: |_| {
            READS.fetch_add(1, Ordering::SeqCst);
            Ok(20 * 1024 * 1024 * 1024)
        },
        ..common::low_disk(&f.env)
    };

    let out = go(&f, &opts, None, &b, &FakeGitHub::default());

    out.result.unwrap();
    // `task run`'s own check and the one before the review: the review itself doesn't read the
    // drive again with a refusal that names no way to continue.
    assert_eq!(READS.load(Ordering::SeqCst), 2);
    assert_eq!(b.creates().len(), 2, "{:?}", b.log());
}
