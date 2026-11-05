//! Issue 418 (decision 285): with `[fix] resume = true`, a Claude worker's session is kept in the
//! task's folder as `worker-session.jsonl` once its run ends, and a fix round continues it
//! (`claude -p --resume <session>`) with the same fix prompt. A session over 64 MiB, an empty or
//! missing one, or one the CLI can't find makes the fix round run fresh, and says so; a
//! non-Claude worker, or the switch off, runs fresh as before.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::task_fixture::{
    CLAUDE_DONE, CODEX_DONE, Fixture, Play, ctx, fixture_with, ok, play_reviews, play_tasks,
    source, worked_task,
};
use sbxm::backend::{ExecOutput, ExecSpec, FakeBackend};
use sbxm::github::fake::FakeGitHub;
use sbxm::task::pipeline::{self, Prepared, ReviewReport};
use sbxm::task::record::{self, Stage, Status};

/// The session id in `CLAUDE_DONE`.
const SESSION: &str = "92ae7cb6-403a-4abc-a295-072f335d20cb";
const MIB64: usize = 64 * 1024 * 1024;

const CLEAN: &str = "Must-fix findings: 0\n\nNothing found.\n";
const ONE: &str =
    "Must-fix findings: 1\n\n## Must fix\n\n### M-1 - a.txt:1 does the wrong thing.\n";

fn config(resume: bool) -> Fixture {
    fixture_with(&format!(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"codex\"\n\n\
         [fix]\nresume = {resume}\n",
    ))
}

/// What the "sandbox" does when sbxm copies the session out of `~/.claude/projects`.
#[derive(Clone)]
enum Saved {
    /// The session file has these bytes.
    Bytes(Arc<Vec<u8>>),
    /// No session file: the copy script fails.
    Missing,
}

fn is_save(spec: &ExecSpec) -> bool {
    spec.argv.first().is_some_and(|a| a == "sh")
        && spec.argv.iter().any(|a| a.contains(".claude/projects"))
        && spec.argv.iter().any(|a| a.contains("find"))
}

fn is_restore(spec: &ExecSpec) -> bool {
    spec.argv.first().is_some_and(|a| a == "sh")
        && spec.argv.iter().any(|a| a.contains(".claude/projects"))
        && spec.argv.iter().any(|a| a.contains("mkdir"))
}

fn is_claude(spec: &ExecSpec) -> bool {
    spec.argv.iter().any(|a| a == "claude")
}

/// The worker (and a fix round) commit `a.txt`; the n-th review is `reviews[n]`; the session copy
/// out of the sandbox writes what `saved` says at the path sbxm names. Every resumed `claude`
/// run answers with `resumed`.
fn backend(f: &Fixture, reviews: &[&str], saved: Saved, resumed: ExecOutput) -> FakeBackend {
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
    // The local clone is mounted in the sandbox under another path on Windows (`C:\x` is `/c/x`),
    // so the copy the sandbox makes lands in the host workspace's `.sbxm-task/`, where sbxm reads it.
    let workspace = f.env.base_dir().join("tasks").join("issue-41");
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_responder(move |_sandbox, spec| {
            if is_save(spec) {
                // `sh -c <script> sh <session> <destination in the clone>`
                let in_clone = PathBuf::from(spec.argv.last().unwrap());
                let dest = workspace
                    .join(in_clone.parent().unwrap().file_name().unwrap())
                    .join(in_clone.file_name().unwrap());
                return Some(match &saved {
                    Saved::Bytes(bytes) => {
                        fs::create_dir_all(dest.parent().unwrap()).unwrap();
                        fs::write(&dest, bytes.as_slice()).unwrap();
                        ok("")
                    }
                    Saved::Missing => ExecOutput {
                        stdout: String::new(),
                        stderr: "no session file".into(),
                        exit_code: Some(1),
                    },
                });
            }
            if is_restore(spec) {
                return Some(ok(""));
            }
            if spec.argv == ["printenv", "HOME"] {
                return Some(ok("/home/agent\n"));
            }
            if is_claude(spec) && spec.argv.iter().any(|a| a == "--resume") {
                return Some(resumed.clone());
            }
            None
        })
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            reviewer(sandbox, spec);
        })
}

fn bytes(n: usize) -> Saved {
    Saved::Bytes(Arc::new(vec![b'x'; n]))
}

fn meta(f: &Fixture) -> PathBuf {
    record::task_dir(&f.env.base_dir(), "issue-41")
}

fn kept(f: &Fixture) -> PathBuf {
    meta(f).join("worker-session.jsonl")
}

fn saved_session(f: &Fixture) -> Option<String> {
    record::read(&meta(f).join("task.json"))
        .unwrap()
        .worker
        .unwrap()
        .session
}

fn review(
    f: &Fixture,
    backend: &FakeBackend,
    prepared: &mut Prepared,
) -> anyhow::Result<ReviewReport> {
    let github = FakeGitHub::default();
    let source = source(f);
    pipeline::review_issue(&ctx(f, &source, backend, &github).env(), prepared)
}

/// The `claude` runs that read the fix prompt, in order.
fn fix_runs(backend: &FakeBackend) -> Vec<Vec<String>> {
    backend
        .execs()
        .into_iter()
        .filter(|(_, spec)| is_claude(spec) && spec.argv.iter().any(|a| a.contains("fix-prompt")))
        .map(|(_, spec)| spec.argv)
        .collect()
}

fn resumes(argv: &[String]) -> Option<&str> {
    argv.iter()
        .position(|a| a == "--resume")
        .map(|at| argv[at + 1].as_str())
}

fn said(prepared: &Prepared) -> String {
    prepared.events.join("\n")
}

fn count(backend: &FakeBackend, pick: fn(&ExecSpec) -> bool) -> usize {
    backend.execs().iter().filter(|(_, s)| pick(s)).count()
}

#[test]
fn the_workers_session_is_kept_in_the_task_folder() {
    let f = config(true);
    let backend = backend(&f, &[CLEAN], bytes(10), ok(CLAUDE_DONE));

    let _prepared = worked_task(&f, &backend);

    assert_eq!(fs::read(kept(&f)).unwrap(), vec![b'x'; 10]);
    assert_eq!(saved_session(&f).as_deref(), Some(SESSION));
    // The copy names the run's own session.
    let save = backend
        .execs()
        .into_iter()
        .find(|(_, s)| is_save(s))
        .unwrap()
        .1;
    assert!(save.argv.iter().any(|a| a == SESSION), "{:?}", save.argv);
    // The copy left in the clone is removed inside the sandbox, never by the host.
    let copy = save.argv.last().unwrap().clone();
    assert!(
        backend.execs().iter().any(|(_, s)| !is_save(s)
            && s.argv.get(2).is_some_and(|a| a.contains("rm -f"))
            && s.argv.last() == Some(&copy)),
        "{copy}"
    );
}

#[test]
fn a_fix_round_resumes_the_workers_session_with_the_same_fix_prompt() {
    let f = config(true);
    let backend = backend(&f, &[ONE, CLEAN], bytes(10), ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    let runs = fix_runs(&backend);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(resumes(&runs[0]), Some(SESSION));
    let claude = runs[0].iter().position(|a| a == "claude").unwrap();
    assert_eq!(
        runs[0][claude..claude + 3],
        [
            "claude",
            "-p",
            "Read the file .sbxm-task/fix-prompt.md in the current directory and follow it exactly."
        ]
    );
    // The session went back into the sandbox before the round.
    let restore = backend
        .execs()
        .into_iter()
        .find(|(_, s)| is_restore(s))
        .unwrap()
        .1;
    assert!(restore.argv.iter().any(|a| a == SESSION));
    // Under the clone's own project folder: its in-sandbox path (on Windows not the host's: `C:\x`
    // is mounted as `/c/x`) with `/` (and every other character but a letter or digit) turned
    // into `-`.
    let clone = prepared
        .sandbox_clone()
        .to_string_lossy()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    assert!(
        restore
            .argv
            .contains(&format!("/home/agent/.claude/projects/{clone}")),
        "{:?}",
        restore.argv
    );
    let record = record::read(&meta(&f).join("task.json")).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_session_of_exactly_64_mib_is_kept() {
    let f = config(true);
    let backend = backend(&f, &[CLEAN], bytes(MIB64), ok(CLAUDE_DONE));

    let prepared = worked_task(&f, &backend);

    assert_eq!(fs::metadata(kept(&f)).unwrap().len(), MIB64 as u64);
    assert_eq!(saved_session(&f).as_deref(), Some(SESSION));
    assert!(!said(&prepared).contains("fresh"), "{}", said(&prepared));
}

#[test]
fn a_session_one_byte_over_64_mib_is_not_kept_and_the_fix_round_runs_fresh() {
    let f = config(true);
    let backend = backend(&f, &[ONE, CLEAN], bytes(MIB64 + 1), ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);

    assert!(!kept(&f).exists());
    assert_eq!(saved_session(&f), None);
    let at_save = said(&prepared);
    assert!(at_save.contains("64 MiB"), "{at_save}");

    review(&f, &backend, &mut prepared).unwrap();

    let runs = fix_runs(&backend);
    assert_eq!(runs.len(), 1);
    assert_eq!(resumes(&runs[0]), None);
    assert_eq!(count(&backend, is_restore), 0);
    assert!(
        said(&prepared).contains("runs fresh"),
        "{}",
        said(&prepared)
    );
}

#[test]
fn an_empty_session_is_not_kept_and_the_fix_round_runs_fresh() {
    let f = config(true);
    let backend = backend(&f, &[ONE, CLEAN], bytes(0), ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);

    assert!(!kept(&f).exists());
    assert_eq!(saved_session(&f), None);
    assert!(said(&prepared).contains("empty"), "{}", said(&prepared));

    review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(resumes(&fix_runs(&backend)[0]), None);
    assert!(
        said(&prepared).contains("runs fresh"),
        "{}",
        said(&prepared)
    );
}

#[test]
fn a_missing_session_is_not_kept_and_the_fix_round_runs_fresh() {
    let f = config(true);
    let backend = backend(&f, &[ONE, CLEAN], Saved::Missing, ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);

    assert!(!kept(&f).exists());
    assert_eq!(saved_session(&f), None);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert_eq!(resumes(&fix_runs(&backend)[0]), None);
    assert!(
        said(&prepared).contains("runs fresh"),
        "{}",
        said(&prepared)
    );
    let record = record::read(&meta(&f).join("task.json")).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn a_kept_session_file_that_is_gone_from_the_task_folder_runs_the_fix_round_fresh() {
    let f = config(true);
    let backend = backend(&f, &[ONE, CLEAN], bytes(10), ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);
    fs::remove_file(kept(&f)).unwrap();

    review(&f, &backend, &mut prepared).unwrap();

    assert_eq!(resumes(&fix_runs(&backend)[0]), None);
    assert!(
        said(&prepared).contains("runs fresh"),
        "{}",
        said(&prepared)
    );
}

/// The `claude` CLI says `No conversation found with session ID: <id>` and exits 1 before any
/// model call when the session isn't there (checked by hand, 2026-11-04): the round runs again
/// fresh, never fails the task for it.
#[test]
fn a_session_the_cli_cannot_find_falls_back_to_a_fresh_fix_round() {
    let f = config(true);
    let not_found = ExecOutput {
        stdout: String::new(),
        stderr: format!("No conversation found with session ID: {SESSION}\n"),
        exit_code: Some(1),
    };
    let backend = backend(&f, &[ONE, CLEAN], bytes(10), not_found);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    let runs = fix_runs(&backend);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(resumes(&runs[0]), Some(SESSION));
    assert_eq!(resumes(&runs[1]), None);
    assert!(
        said(&prepared).contains("could not be resumed"),
        "{}",
        said(&prepared)
    );
    let record = record::read(&meta(&f).join("task.json")).unwrap();
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
}

#[test]
fn with_the_switch_off_nothing_is_kept_and_fix_rounds_run_fresh() {
    let f = config(false);
    let backend = backend(&f, &[ONE, CLEAN], bytes(10), ok(CLAUDE_DONE));
    let mut prepared = worked_task(&f, &backend);

    review(&f, &backend, &mut prepared).unwrap();

    assert!(!kept(&f).exists());
    assert_eq!(saved_session(&f), None);
    assert_eq!(count(&backend, is_save), 0);
    assert_eq!(count(&backend, is_restore), 0);
    assert_eq!(resumes(&fix_runs(&backend)[0]), None);
    assert!(!said(&prepared).contains("fresh"), "{}", said(&prepared));
}

/// A Codex worker with a Claude reviewer: the reviewer writes `review.md` in its clone.
fn codex_worker_backend(f: &Fixture, reviews: Vec<&'static str>) -> FakeBackend {
    let worker = play_tasks(
        &f.env.base_dir(),
        "main",
        Play {
            commits: vec!["a.txt".into()],
            result_md: Some(b"done\n".to_vec()),
            bundle_bytes: None,
        },
    );
    let clone: PathBuf = f.env.base_dir().join("tasks").join("issue-41-review");
    let next = Arc::new(Mutex::new(0usize));
    FakeBackend::with_secrets(&["anthropic", "openai"])
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_output_matching("codex", ok(CODEX_DONE))
        .with_exec_hook(move |sandbox, spec| {
            worker(sandbox, spec);
            if sandbox.contains("-review-") && is_claude(spec) {
                let mut n = next.lock().unwrap();
                let text = reviews[(*n).min(reviews.len() - 1)];
                *n += 1;
                write_review(&clone, text);
            }
        })
}

fn write_review(clone: &Path, text: &str) {
    let dir = clone.join(".sbxm-task");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("review.md"), text).unwrap();
}

#[test]
fn a_non_claude_worker_with_the_switch_on_runs_its_fix_round_fresh_as_before() {
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\n\n[worker]\nharness = \"codex\"\nfix_rounds = 1\n\n\
         [gates]\nsandbox = [\"cargo test\"]\n\n[reviewer]\nharness = \"claude\"\n\n\
         [fix]\nresume = true\n",
    );
    let backend = codex_worker_backend(&f, vec![ONE, CLEAN]);
    let mut prepared = worked_task(&f, &backend);

    let report = review(&f, &backend, &mut prepared).unwrap();

    assert!(report.fix_ran);
    assert!(!kept(&f).exists());
    assert_eq!(saved_session(&f), None);
    assert_eq!(count(&backend, is_save), 0);
    assert_eq!(count(&backend, is_restore), 0);
    assert!(
        !backend
            .execs()
            .iter()
            .any(|(_, s)| s.argv.iter().any(|a| a == "--resume"))
    );
    assert!(!said(&prepared).contains("fresh"), "{}", said(&prepared));
}

/// A cloud task's worker sandbox is removed once the automatic run is done, and its clone goes
/// with a sandbox made again: the session comes out through `sbx cp` into the task's folder and
/// goes back in by `cp` before the fix round. The fake runs the scripts for real, with the
/// sandbox's home under a temp folder.
#[test]
fn a_cloud_fix_round_resumes_the_session_kept_in_the_task_folder() {
    use common::cloud_task::{self, HOME};
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\n\
         sandbox = [\"cargo test\"]\n\n[fix]\nresume = true\n",
    );
    let (spec, id) = cloud_task::spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend =
        cloud_task::cloud_backend_listing(&root, &id, &[cloud_task::ONE, cloud_task::CLEAN], false);
    // The worker's session, where the CLI left it (in a project folder of its own choosing).
    let projects = root
        .join(HOME.trim_start_matches('/'))
        .join(".claude/projects");
    let elsewhere = projects.join("-somewhere-else");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join(format!("{SESSION}.jsonl")), "{\"turn\":1}\n").unwrap();

    let ran = cloud_task::task_run(&f, spec, Some(sbxm::backend::Location::Cloud), &backend);

    ran.result.unwrap();
    let meta = record::task_dir(&f.env.base_dir(), &id);
    assert_eq!(
        fs::read_to_string(meta.join("worker-session.jsonl")).unwrap(),
        "{\"turn\":1}\n"
    );
    let runs = fix_runs(&backend);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(resumes(&runs[0]), Some(SESSION), "{}", ran.out);
    // Put back under the cloud clone's own project folder, and gone from the clone.
    let clone = format!("{HOME}/work/{id}");
    let restored = projects
        .join(clone.replace(|c: char| !c.is_ascii_alphanumeric(), "-"))
        .join(format!("{SESSION}.jsonl"));
    assert_eq!(fs::read_to_string(restored).unwrap(), "{\"turn\":1}\n");
    assert!(
        !root
            .join(clone.trim_start_matches('/'))
            .join(".sbxm-task/worker-session.jsonl")
            .exists()
    );
    assert!(!ran.out.contains("fresh"), "{}", ran.out);
}

/// The fallback line is printed, so `run.log` (a copy of what a task command prints) has it.
#[test]
fn a_fix_round_that_runs_fresh_says_so_in_the_commands_output() {
    use common::cloud_task::{self, HOME};
    let f = fixture_with(
        "[sandbox]\nprofile = \"default\"\nlocation = \"local\"\n\n[gates]\n\
         sandbox = [\"cargo test\"]\n\n[fix]\nresume = true\n",
    );
    let (spec, id) = cloud_task::spec(&f);
    let root = f.env.tmp.path().join("cloud-fs");
    let backend =
        cloud_task::cloud_backend_listing(&root, &id, &[cloud_task::ONE, cloud_task::CLEAN], false);
    // No session file anywhere under the sandbox's `~/.claude/projects`.
    fs::create_dir_all(
        root.join(HOME.trim_start_matches('/'))
            .join(".claude/projects"),
    )
    .unwrap();

    let ran = cloud_task::task_run(&f, spec, Some(sbxm::backend::Location::Cloud), &backend);

    ran.result.unwrap();
    assert_eq!(resumes(&fix_runs(&backend)[0]), None);
    for line in [
        format!("{id}: the worker's session was not kept ("),
        format!("{id}: the fix round runs fresh: no session of the worker was kept"),
    ] {
        assert!(ran.out.contains(&line), "{line}\n{}", ran.out);
    }
    let record = cloud_task::task_record(&f, &id);
    assert_eq!(record.stage, Stage::Ready);
}

/// The clone is the agent's: whatever it leaves at the paths the session travels through (a link
/// or a hard link in `.sbxm-task`, a linked `.sbxm-task`, a link under `~/.claude/projects`), the
/// session's copies never write through it or delete what it points at (review M-1). The session
/// scripts run for real here, with the sandbox's home in a temp folder.
#[cfg(unix)]
mod links {
    use super::*;
    use common::task_fixture::issue_text;
    use std::os::unix::fs::symlink;

    /// Like [`backend`], but the `sh` scripts run for real, `printenv HOME` answers `home` (where
    /// the worker's session is), and `meddle` runs in the clone once the worker's run is done.
    fn real_backend(
        f: &Fixture,
        reviews: &[&str],
        home: &Path,
        meddle: impl Fn(&Path) + Send + Sync + 'static,
    ) -> FakeBackend {
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
        let session = home.join(".claude/projects/-somewhere");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join(format!("{SESSION}.jsonl")), "{\"turn\":1}\n").unwrap();
        let home = format!("{}\n", home.display());
        FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_exec_responder(move |_sandbox, spec| {
                if spec.argv == ["printenv", "HOME"] {
                    return Some(ok(&home));
                }
                if spec.argv.first().is_none_or(|a| a != "sh") {
                    return None;
                }
                let out = std::process::Command::new("sh")
                    .args(&spec.argv[1..])
                    .output()
                    .unwrap();
                Some(ExecOutput {
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    exit_code: out.status.code(),
                })
            })
            .with_exec_output_matching("claude", ok(CLAUDE_DONE))
            .with_exec_output_matching("codex", ok(CODEX_DONE))
            .with_exec_hook(move |sandbox, spec| {
                worker(sandbox, spec);
                reviewer(sandbox, spec);
                let first_run = is_claude(spec) && !spec.argv.iter().any(|a| a.contains("fix-"));
                if let (true, Some(dir)) = (first_run, &spec.workdir) {
                    meddle(dir);
                }
            })
    }

    /// The worker's run, without failing the test when a link the worker left makes a later
    /// step of it refuse.
    fn run_worker(f: &Fixture, backend: &FakeBackend) -> Prepared {
        let github = FakeGitHub::default();
        let source = source(f);
        let ctx = ctx(f, &source, backend, &github);
        let mut prepared = pipeline::prepare(&ctx, &issue_text(41)).unwrap();
        let _ = pipeline::run_worker(&ctx, &mut prepared);
        prepared.lock = None;
        prepared
    }

    #[test]
    fn a_link_left_where_the_session_is_copied_is_not_written_through() {
        let f = config(true);
        let home = f.env.tmp.path().join("sandbox-home");
        let backend = real_backend(&f, &[CLEAN], &home, |clone| {
            fs::write(clone.join("a.txt"), "mine\n").unwrap();
            symlink("../a.txt", clone.join(".sbxm-task/worker-session.jsonl")).unwrap();
        });

        let prepared = run_worker(&f, &backend);

        assert_eq!(
            fs::read_to_string(prepared.workspace.join("a.txt")).unwrap(),
            "mine\n"
        );
        assert!(said(&prepared).contains("not kept"), "{}", said(&prepared));
        assert_eq!(saved_session(&f), None);
        assert!(!kept(&f).exists());
    }

    #[test]
    fn a_hard_link_left_where_the_session_is_copied_is_replaced_not_written_through() {
        let f = config(true);
        let home = f.env.tmp.path().join("sandbox-home");
        let backend = real_backend(&f, &[CLEAN], &home, |clone| {
            fs::write(clone.join("a.txt"), "mine\n").unwrap();
            fs::hard_link(
                clone.join("a.txt"),
                clone.join(".sbxm-task/worker-session.jsonl"),
            )
            .unwrap();
        });

        let prepared = run_worker(&f, &backend);

        assert_eq!(
            fs::read_to_string(prepared.workspace.join("a.txt")).unwrap(),
            "mine\n"
        );
        assert_eq!(fs::read_to_string(kept(&f)).unwrap(), "{\"turn\":1}\n");
        assert_eq!(saved_session(&f).as_deref(), Some(SESSION));
    }

    #[test]
    fn a_linked_control_folder_is_neither_written_into_nor_cleaned_up() {
        let f = config(true);
        let home = f.env.tmp.path().join("sandbox-home");
        let elsewhere = f.env.tmp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("worker-session.jsonl"), "mine\n").unwrap();
        let target = elsewhere.clone();
        let backend = real_backend(&f, &[CLEAN], &home, move |clone| {
            let control = clone.join(".sbxm-task");
            fs::rename(&control, clone.join("moved")).unwrap();
            symlink(&target, &control).unwrap();
        });

        let prepared = run_worker(&f, &backend);

        assert_eq!(
            fs::read_to_string(elsewhere.join("worker-session.jsonl")).unwrap(),
            "mine\n"
        );
        assert!(said(&prepared).contains("not kept"), "{}", said(&prepared));
        assert_eq!(saved_session(&f), None);
    }

    #[test]
    fn a_link_left_where_the_session_is_put_back_is_not_written_through() {
        let f = config(true);
        let home = f.env.tmp.path().join("sandbox-home");
        let victim = f.env.tmp.path().join("victim.txt");
        fs::write(&victim, "mine\n").unwrap();
        let (target, projects) = (victim.clone(), home.join(".claude/projects"));
        let backend = real_backend(&f, &[ONE, CLEAN], &home, move |clone| {
            let dir = projects.join(
                clone
                    .to_string_lossy()
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
            );
            fs::create_dir_all(&dir).unwrap();
            symlink(&target, dir.join(format!("{SESSION}.jsonl"))).unwrap();
        });
        let mut prepared = run_worker(&f, &backend);
        assert_eq!(saved_session(&f).as_deref(), Some(SESSION));

        let report = review(&f, &backend, &mut prepared).unwrap();

        assert!(report.fix_ran);
        assert_eq!(fs::read_to_string(&victim).unwrap(), "mine\n");
        assert_eq!(resumes(&fix_runs(&backend)[0]), None);
        assert!(
            said(&prepared).contains("runs fresh"),
            "{}",
            said(&prepared)
        );
    }
}
