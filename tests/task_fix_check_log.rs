//! Issue 448 (decision 300): `sbxm task fix --pr N --check-run <id>` works from a failed workflow
//! run's log: the last 64 KiB in all, split evenly between the run's failed jobs, cut at a line
//! boundary and never inside a UTF-8 character, given to the worker as `.sbxm-task/check-log.md`.
//! A run that isn't a failed run of the PR's current head is refused before anything is created.

mod common;

use sbxm::github::{JobLog, PrInfo, PrState, RunStatus, WorkflowRun};
use sbxm::task::pr_fix::{CHECK_LOG_LIMIT, check_log, failed_run};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn run(id: u64, status: RunStatus) -> WorkflowRun {
    WorkflowRun {
        id,
        workflow: "CI".into(),
        attempt: 2,
        status,
        url: format!("https://github.com/o/r/actions/runs/{id}"),
        actor: "tutros".into(),
    }
}

fn job(name: &str, log: &str) -> JobLog {
    JobLog {
        job: name.into(),
        log: log.into(),
    }
}

/// `n` bytes of log: lines of 99 `x` and a newline, the last one shorter if `n` isn't a multiple
/// of 100.
fn lines_of(n: usize) -> String {
    let mut text = String::with_capacity(n);
    while text.len() < n {
        let left = n - text.len();
        if left >= 100 {
            text.push_str(&"x".repeat(99));
            text.push('\n');
        } else {
            text.push_str(&"x".repeat(left - 1));
            text.push('\n');
        }
    }
    text
}

/// The sections of a check log: (its first line, the log after it).
fn sections(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("Failed run ") {
            out.push((line.trim_end().to_owned(), String::new()));
        } else {
            out.last_mut().expect("a header first").1.push_str(line);
        }
    }
    out
}

#[test]
fn a_log_of_exactly_64_kib_is_kept_whole() {
    let log = lines_of(CHECK_LOG_LIMIT);
    assert_eq!(log.len(), 65_536);

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let parts = sections(&text);
    assert_eq!(parts.len(), 1, "{text:.300}");
    assert_eq!(parts[0].1, log);
    assert!(
        parts[0].0.contains("run 5")
            && parts[0].0.contains("job test")
            && parts[0].0.contains("0 line(s) dropped from the start"),
        "{}",
        parts[0].0
    );
}

#[test]
fn one_byte_over_64_kib_drops_the_first_line_and_says_so() {
    let log = format!("y\n{}", lines_of(CHECK_LOG_LIMIT - 1));
    assert_eq!(log.len(), 65_537);

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let parts = sections(&text);
    assert_eq!(parts[0].1, &log[2..]);
    assert!(
        parts[0].0.contains("1 line(s) dropped from the start"),
        "{}",
        parts[0].0
    );
}

#[test]
fn a_cut_lands_on_a_line_boundary() {
    // Lines of 100 bytes: 64 KiB + 50 bytes means the cut falls inside a line, which is dropped.
    let log = lines_of(CHECK_LOG_LIMIT + 50);

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let kept = &sections(&text)[0].1;
    assert!(kept.len() <= CHECK_LOG_LIMIT);
    assert!(log.ends_with(kept.as_str()));
    let cut = log.len() - kept.len();
    assert_eq!(&log[cut - 1..cut], "\n");
}

#[test]
fn a_multi_byte_character_across_the_cut_is_not_split() {
    // Lines of 25 four-byte characters and a newline: the raw cut falls inside a character, and
    // the kept part starts at the next line instead.
    let line = format!("{}\n", "😀".repeat(25));
    let mut log = String::new();
    while log.len() < CHECK_LOG_LIMIT + 50 {
        log.push_str(&line);
    }
    let raw = log.len() - CHECK_LOG_LIMIT;
    assert!(
        !log.is_char_boundary(raw),
        "the test must cut inside a character"
    );

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let (header, kept) = &sections(&text)[0];
    assert!(log.ends_with(kept.as_str()));
    assert!(kept.len() <= CHECK_LOG_LIMIT);
    assert!(kept.len() > CHECK_LOG_LIMIT - line.len());
    assert!(kept.starts_with(&line), "{kept:.200}");
    let dropped = (log.len() - kept.len()) / line.len();
    assert!(
        header.ends_with(&format!(": {dropped} line(s) dropped from the start")),
        "{header}"
    );
}

#[test]
fn a_line_longer_than_the_share_is_dropped_whole_never_cut() {
    let log = format!("{}\n", "x".repeat(CHECK_LOG_LIMIT + 1));

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let (header, kept) = &sections(&text)[0];
    assert_eq!(kept, "", "{text:.200}");
    assert!(
        header.ends_with(": 1 line(s) dropped from the start"),
        "{header}"
    );
}

#[test]
fn a_last_line_longer_than_the_share_without_a_newline_is_dropped_whole_too() {
    let log = format!("y\n{}", "x".repeat(CHECK_LOG_LIMIT + 3));

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    let (header, kept) = &sections(&text)[0];
    assert_eq!(kept, "", "{text:.200}");
    assert!(
        header.ends_with(": 2 line(s) dropped from the start"),
        "{header}"
    );
}

#[test]
fn two_failed_jobs_each_get_at_most_half_and_an_unused_share_is_not_passed_on() {
    let small = lines_of(10 * 1024);
    let big = lines_of(200 * 1024);

    let text = check_log(
        &run(5, RunStatus::Failed),
        &[job("lint", &small), job("test", &big)],
    );

    let parts = sections(&text);
    assert_eq!(parts.len(), 2, "{text:.300}");
    assert!(parts[0].0.contains("job lint"), "{}", parts[0].0);
    assert_eq!(parts[0].1, small);
    assert!(parts[1].0.contains("job test"), "{}", parts[1].0);
    assert!(
        parts[1].1.len() <= CHECK_LOG_LIMIT / 2,
        "{}",
        parts[1].1.len()
    );
    assert!(parts[1].1.len() > CHECK_LOG_LIMIT / 2 - 100);
    assert!(big.ends_with(parts[1].1.as_str()));
}

#[test]
fn a_failed_job_with_an_empty_log_is_named_with_nothing_else() {
    let text = check_log(
        &run(5, RunStatus::Failed),
        &[job("setup", ""), job("test", "boom\n")],
    );

    let parts = sections(&text);
    assert_eq!(parts.len(), 2, "{text}");
    assert!(
        parts[0].0.contains("job setup") && parts[0].0.contains("empty"),
        "{}",
        parts[0].0
    );
    assert_eq!(parts[0].1, "");
    assert_eq!(parts[1].1, "boom\n");
}

#[test]
fn a_run_with_no_failed_job_listed_still_names_the_run() {
    let text = check_log(&run(5, RunStatus::Failed), &[]);

    assert!(text.starts_with("Failed run 5"), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}

fn pr() -> PrInfo {
    PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: "o/r".into(),
        state: PrState::Open,
        title: "Add x".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    }
}

#[test]
fn a_failed_run_of_the_head_is_the_one_to_fix() {
    let runs = [run(4, RunStatus::Passed), run(5, RunStatus::Failed)];

    let chosen = failed_run(&pr(), "o/r", 5, &runs).unwrap();

    assert_eq!(chosen.id, 5);
}

#[test]
fn a_run_the_head_does_not_list_is_refused_naming_what_to_pass() {
    // A run of an older head, or of another repo, isn't among the head's runs.
    let message = format!(
        "{:#}",
        failed_run(&pr(), "o/r", 9, &[run(5, RunStatus::Failed)]).unwrap_err()
    );

    assert!(message.contains("run 9"), "{message}");
    assert!(message.contains(HEAD), "{message}");
    assert!(message.contains("--check-run"), "{message}");
    assert!(
        message.contains(&format!("gh run list --repo o/r --commit {HEAD}")),
        "{message}"
    );
}

#[test]
fn a_passed_run_is_refused_naming_what_to_pass() {
    let message = format!(
        "{:#}",
        failed_run(&pr(), "o/r", 5, &[run(5, RunStatus::Passed)]).unwrap_err()
    );

    assert!(message.contains("passed"), "{message}");
    assert!(message.contains("--check-run"), "{message}");
}

#[test]
fn a_run_still_going_is_refused_naming_what_to_pass() {
    let message = format!(
        "{:#}",
        failed_run(&pr(), "o/r", 5, &[run(5, RunStatus::Going)]).unwrap_err()
    );

    assert!(message.contains("still going"), "{message}");
    assert!(message.contains("--check-run"), "{message}");
}

mod gh_backend {
    //! `GhBackend::run_failed_log`: the failed jobs from `gh run view --json jobs`, each with its
    //! lines of `gh run view --log-failed` (written by hand in the shape `gh` 2.83 prints them:
    //! `<job>\t<step>\t<timestamp> <text>`; no GitHub access where issue 448 was built).

    use std::sync::Mutex;

    use anyhow::{Result, bail};
    use sbxm::github::gh::{GhBackend, GhRunner};
    use sbxm::github::{GitHubBackend, JobLog};

    struct Scripted {
        calls: Mutex<Vec<Vec<String>>>,
        outputs: Mutex<Vec<String>>,
    }

    impl GhRunner for &'static Scripted {
        fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
            self.calls.lock().unwrap().push(args.to_vec());
            match self.outputs.lock().unwrap().pop() {
                Some(out) => Ok(out),
                None => bail!("no scripted output left"),
            }
        }
    }

    fn backend(outputs: &[&str]) -> (GhBackend, &'static Scripted) {
        let script: &'static Scripted = Box::leak(Box::new(Scripted {
            calls: Mutex::default(),
            outputs: Mutex::new(outputs.iter().rev().map(|s| (*s).to_owned()).collect()),
        }));
        (GhBackend::with_runner(Box::new(script)), script)
    }

    const JOBS: &str = r#"{"jobs":[
        {"name":"lint","conclusion":"success","status":"completed"},
        {"name":"setup","conclusion":"failure","status":"completed"},
        {"name":"test","conclusion":"failure","status":"completed"}
    ]}"#;

    #[test]
    fn each_failed_job_gets_its_own_lines_and_one_with_none_an_empty_log() {
        let log = "test\tRun tests\t2026-11-05T10:00:00.0000000Z running 3 tests\n\
                   test\tRun tests\t2026-11-05T10:00:01.0000000Z test a ... FAILED\n";
        let (gh, script) = backend(&[JOBS, log]);

        let jobs = gh.run_failed_log("o/r", 42).unwrap();

        assert_eq!(
            jobs,
            vec![
                JobLog {
                    job: "setup".into(),
                    log: String::new(),
                },
                JobLog {
                    job: "test".into(),
                    log: log.into(),
                },
            ]
        );
        let calls = script.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                ["run", "view", "42", "--repo", "o/r", "--json", "jobs"]
                    .map(str::to_owned)
                    .to_vec(),
                ["run", "view", "42", "--repo", "o/r", "--log-failed"]
                    .map(str::to_owned)
                    .to_vec(),
            ]
        );
    }

    #[test]
    fn a_job_in_the_log_that_the_jobs_list_missed_is_kept() {
        let (gh, _) = backend(&[r#"{"jobs":[]}"#, "build\tstep\tline\n"]);

        let jobs = gh.run_failed_log("o/r", 42).unwrap();

        assert_eq!(
            jobs,
            vec![JobLog {
                job: "build".into(),
                log: "build\tstep\tline\n".into(),
            }]
        );
    }
}

mod fake_backend {
    use sbxm::github::fake::{FakeGitHub, GhCall};
    use sbxm::github::{GitHubBackend, JobLog};

    #[test]
    fn the_fake_answers_the_scripted_log_and_records_the_call() {
        let jobs = vec![JobLog {
            job: "test".into(),
            log: "boom\n".into(),
        }];
        let gh = FakeGitHub::default().with_run_log(42, jobs.clone());

        assert_eq!(gh.run_failed_log("o/r", 42).unwrap(), jobs);
        assert!(gh.run_failed_log("o/r", 43).unwrap().is_empty());
        assert_eq!(
            gh.calls(),
            vec![
                GhCall::RunFailedLog("o/r".into(), 42),
                GhCall::RunFailedLog("o/r".into(), 43)
            ]
        );
    }
}

mod command {
    //! `sbxm task fix --pr 7 --check-run <id>` end to end on `FakeGitHub` and `FakeBackend`: the
    //! worker reads the log from `.sbxm-task/check-log.md`, then the gates, the in-task review and
    //! the push run as for any `task fix`; every refusal comes before anything is created.

    use std::fs;
    use std::sync::{Arc, Mutex};

    use super::common::git;
    use super::common::task_fixture::{
        CLAUDE_DONE, CODEX_DONE, Fixture, Play, Probe, add_pr_head, fixture_with, is_headless, ok,
        play, play_reviews,
    };
    use sbxm::backend::FakeBackend;
    use sbxm::commands::task_fix::{Options, run};
    use sbxm::github::fake::{FakeGitHub, GhCall};
    use sbxm::github::{JobLog, PrInfo, PrState, RunStatus, WorkflowRun};
    use sbxm::task::gates::FakeHostRunner;
    use sbxm::task::record;
    use sbxm::task::repo::Identity;

    fn config() -> Fixture {
        fixture_with(
            "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = [\"cargo test\"]\n\n\
             [reviewer]\nharness = \"codex\"\n",
        )
    }

    fn pr7(head: &str) -> PrInfo {
        PrInfo {
            number: 7,
            head_ref: "feature-x".into(),
            head_oid: head.into(),
            is_cross_repository: false,
            head_repo: "o/r".into(),
            state: PrState::Open,
            title: "Add the x feature".into(),
            body: "This adds x.".into(),
            closing_issues: vec![],
            base_moved: false,
        }
    }

    fn ci(id: u64, status: RunStatus) -> WorkflowRun {
        WorkflowRun {
            id,
            workflow: "CI".into(),
            attempt: 2,
            status,
            url: format!("https://github.com/o/r/actions/runs/{id}"),
            actor: "tutros".into(),
        }
    }

    /// The failed test's line: something the worker's `check-log.md` must hold.
    const FAILED_LINE: &str =
        "test\tRun tests\t2026-11-05T10:00:00Z test lock::released ... FAILED";

    fn pr_head(f: &Fixture) -> String {
        let head = add_pr_head(f, 7);
        git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
        head
    }

    /// PR 7 at `head`, whose run 5 failed twice with a log for job `test`; no review on the PR.
    fn github(head: &str, runs: Vec<WorkflowRun>) -> FakeGitHub {
        FakeGitHub::default()
            .with_user("tutros")
            .with_default_branch("main")
            .with_pr(pr7(head))
            .with_runs(head, runs)
            .with_run_log(
                5,
                vec![JobLog {
                    job: "test".into(),
                    log: format!("{FAILED_LINE}\n"),
                }],
            )
    }

    const CLEAN: &str =
        "Must-fix findings: 0\nRisk: low\n\nScope: `aaaaaaa..bbbbbbb`\n\nNothing.\n";

    /// What the worker and the reviewer found in their `.sbxm-task/` when they ran: (name, text).
    type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

    fn backend(f: &Fixture, head: &str, seen: Seen) -> FakeBackend {
        let id = record::fix_id(7, head).unwrap();
        let tasks = f.env.base_dir().join("tasks");
        let workspace = tasks.join(&id);
        let review_clone = tasks.join(format!("{id}-review"));
        let worker = play(
            &workspace,
            "main",
            "feature-x",
            Play {
                commits: vec!["src/lock.rs".into()],
                result_md: Some(b"the check passes\n".to_vec()),
                bundle_bytes: None,
            },
        );
        let reviewer = play_reviews(&f.env.base_dir(), &id, vec![CLEAN.to_owned()]);
        FakeBackend::with_secrets(&["anthropic", "openai"])
            .with_exec_output_matching("claude", ok(CLAUDE_DONE))
            .with_exec_output_matching("codex", ok(CODEX_DONE))
            .with_exec_hook(move |sandbox, exec| {
                if is_headless(exec) {
                    let (who, dir) = if sandbox.contains("-review-") {
                        ("reviewer", review_clone.join(".sbxm-task"))
                    } else {
                        ("worker", workspace.join(".sbxm-task"))
                    };
                    for name in ["check-log.md", "prompt.md", "issue.md"] {
                        if let Ok(text) = fs::read_to_string(dir.join(name)) {
                            seen.lock().unwrap().push((who.into(), name.into(), text));
                        }
                    }
                }
                worker(sandbox, exec);
                reviewer(sandbox, exec);
            })
    }

    fn options(f: &Fixture, check_run: u64) -> Options {
        Options {
            repo_root: f.env.tmp.path().join("target-repo"),
            pr: 7,
            repo: Some("o/r".into()),
            base: None,
            clone_source: Some(f.origin.to_str().unwrap().to_owned()),
            identity: Some(Identity {
                name: "Dev".into(),
                email: "dev@example.com".into(),
            }),
            disk: Default::default(),
            check_run: Some(check_run),
            location: None,
        }
    }

    fn go(
        f: &Fixture,
        check_run: u64,
        backend: &FakeBackend,
        github: &FakeGitHub,
    ) -> (anyhow::Result<()>, String) {
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let result = run(
            &f.env.config_dir(),
            &options(f, check_run),
            backend,
            github,
            &Probe,
            &FakeHostRunner::default(),
            &mut out,
            &mut warn,
        );
        (result, String::from_utf8(out).unwrap())
    }

    fn seen_by(seen: &Seen, who: &str, name: &str) -> Option<String> {
        seen.lock()
            .unwrap()
            .iter()
            .find(|(w, n, _)| w == who && n == name)
            .map(|(_, _, text)| text.clone())
    }

    #[test]
    fn the_worker_reads_the_log_and_the_fix_is_gated_reviewed_and_pushed() {
        let f = config();
        let head = pr_head(&f);
        let github = github(
            &head,
            vec![ci(4, RunStatus::Passed), ci(5, RunStatus::Failed)],
        );
        let seen = Seen::default();
        let backend = backend(&f, &head, Arc::clone(&seen));

        let (result, out) = go(&f, 5, &backend, &github);

        result.unwrap();
        let id = record::fix_id(7, &head).unwrap();
        let meta = record::task_dir(&f.env.base_dir(), &id);
        // The log is the worker's input, as a file next to the prompt, which only names it.
        let log = seen_by(&seen, "worker", "check-log.md").expect("the worker's check-log.md");
        assert!(log.starts_with("Failed run 5 (CI"), "{log}");
        assert!(log.contains(FAILED_LINE), "{log}");
        assert_eq!(fs::read_to_string(meta.join("check-log.md")).unwrap(), log);
        let prompt = seen_by(&seen, "worker", "prompt.md").unwrap();
        assert!(prompt.contains(".sbxm-task/check-log.md"), "{prompt}");
        assert!(!prompt.contains("lock::released"), "{prompt}");
        let issue = seen_by(&seen, "worker", "issue.md").unwrap();
        assert!(issue.contains("check-log.md"), "{issue}");
        assert!(!issue.contains("lock::released"), "{issue}");
        // The reviewer gets the log too, to check the fix against it.
        assert_eq!(
            seen_by(&seen, "reviewer", "check-log.md").as_deref(),
            Some(log.as_str())
        );
        // No review was read: the log alone is the input.
        assert!(
            !github
                .calls()
                .contains(&GhCall::PrComments("o/r".into(), 7)),
            "{:?}",
            github.calls()
        );
        // Gates, review and push as for any `task fix`.
        let task = record::read(&meta.join("task.json")).unwrap();
        assert!(
            task.gates
                .iter()
                .any(|g| g.command == "cargo test" && g.passed)
        );
        assert_eq!(task.stage, record::Stage::Finished);
        let tip = git(&f.origin, &["rev-parse", "refs/heads/feature-x"]);
        assert_ne!(tip, head);
        git(&f.origin, &["merge-base", "--is-ancestor", &head, &tip]);
        assert!(out.contains("run 5"), "{out}");
        assert!(
            out.contains(&format!("{id}: pushed 1 commit(s) onto PR #7's branch")),
            "{out}"
        );
    }

    /// A refusal: its message, with nothing written under the base dir and no sandbox call made.
    fn refused(f: &Fixture, check_run: u64, github: &FakeGitHub) -> String {
        let backend = FakeBackend::with_secrets(&["anthropic", "openai"]);
        let (result, _) = go(f, check_run, &backend, github);
        let message = format!("{:#}", result.unwrap_err());
        assert!(backend.log().is_empty(), "{:?}", backend.log());
        let tasks = f.env.base_dir().join("tasks");
        assert!(
            !tasks.exists() || fs::read_dir(&tasks).unwrap().next().is_none(),
            "{message}"
        );
        let root = record::tasks_root(&f.env.base_dir());
        assert!(
            !root.exists() || fs::read_dir(&root).unwrap().next().is_none(),
            "{message}"
        );
        assert!(
            !github
                .calls()
                .iter()
                .any(|c| matches!(c, GhCall::RunFailedLog(..))),
            "{message}"
        );
        message
    }

    #[test]
    fn a_run_of_an_older_head_is_refused_and_nothing_is_created() {
        let f = config();
        let head = pr_head(&f);
        // Run 5 belongs to an older head: the current head lists only run 6.
        let github = github(&head, vec![ci(6, RunStatus::Failed)]);

        let message = refused(&f, 5, &github);

        assert!(
            message.contains("run 5 isn't a workflow run of PR #7's head"),
            "{message}"
        );
        assert!(message.contains("--check-run"), "{message}");
    }

    #[test]
    fn a_passed_run_is_refused_and_nothing_is_created() {
        let f = config();
        let head = pr_head(&f);
        let github = github(&head, vec![ci(5, RunStatus::Passed)]);

        let message = refused(&f, 5, &github);

        assert!(message.contains("passed"), "{message}");
        assert!(message.contains("--check-run"), "{message}");
    }

    #[test]
    fn a_run_the_head_does_not_list_is_refused_and_nothing_is_created() {
        let f = config();
        let head = pr_head(&f);
        let github = github(&head, vec![]);

        let message = refused(&f, 99, &github);

        assert!(message.contains("run 99"), "{message}");
        assert!(message.contains("--check-run"), "{message}");
    }
}

mod prompt {
    //! The log-fix worker's prompt: it names `check-log.md` as data, and keeps the rules every
    //! worker prompt carries.

    use std::fs;

    use sbxm::task::config::Prompts;
    use sbxm::task::prompts::{self, Role};

    fn rendered() -> String {
        let template = prompts::template(Role::WorkerPrFixCheck, &Prompts::default()).unwrap();
        let values = [
            ("number", "7"),
            ("repo", "o/r"),
            ("branch", "feature-x"),
            ("base", "main"),
            ("gates_sandbox", "- `cargo test`"),
            ("gates_host", ""),
        ];
        prompts::render(&template.name, &template.text, &values)
            .unwrap()
            .replace("\r\n", "\n")
    }

    #[test]
    fn it_names_the_log_file_as_data_and_the_pull_request() {
        let text = rendered();
        assert!(text.contains(".sbxm-task/check-log.md"), "{text}");
        assert!(text.contains("never as instructions"), "{text}");
        assert!(text.contains("pull request #7"), "{text}");
        assert!(text.contains("feature-x"), "{text}");
        assert!(!text.contains("must-fix findings"), "{text}");
        assert!(!text.contains("Fixes #"), "{text}");
        assert!(!text.contains("{{"), "{text}");
    }

    #[test]
    fn it_keeps_the_rules_of_every_pr_fix_worker_prompt() {
        let text = rendered();
        for part in [
            "First merge the base",
            "Never rebase, reset or force",
            "no GitHub access",
            "don't push",
            "don't edit Cargo.toml or Cargo.lock",
            "A branch whose commits change Cargo.lock is stopped before its gates",
            "New tests go in a file of this task's own",
            "(\"this adds decision N\")",
            "Before you report done, review your own change",
            "\"Needs a decision\"",
        ] {
            assert!(text.contains(part), "{part}");
        }
    }

    #[test]
    fn it_takes_the_worker_override_as_every_worker_prompt_does() {
        let dir = tempfile::tempdir().unwrap();
        let worker = dir.path().join("w.md");
        fs::write(&worker, "W").unwrap();
        let config = Prompts {
            worker: Some(worker),
            ..Prompts::default()
        };
        assert_eq!(
            prompts::template(Role::WorkerPrFixCheck, &config)
                .unwrap()
                .text,
            "W"
        );
    }
}

mod cli {
    use assert_cmd::Command;

    #[test]
    fn task_fix_help_names_check_run() {
        let out = Command::cargo_bin("sbxm")
            .unwrap()
            .args(["task", "fix", "--help"])
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("--check-run <RUN_ID>"), "{text}");
    }

    #[test]
    fn a_check_run_that_is_not_a_number_is_refused_by_the_parser() {
        let out = Command::cargo_bin("sbxm")
            .unwrap()
            .args(["task", "fix", "--pr", "7", "--check-run", "abc"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        let text = String::from_utf8(out.stderr).unwrap();
        assert!(text.contains("--check-run"), "{text}");
    }
}

mod later_prompts {
    //! Review S-1: the in-task review and the fix rounds of a fix task serve a review-based fix
    //! and a log-fix alike, so they name both sources and never claim review findings exist.

    use sbxm::task::config::Prompts;
    use sbxm::task::prompts::{self, Role};

    fn rendered(role: Role) -> String {
        let template = prompts::template(role, &Prompts::default()).unwrap();
        let values = [
            ("issue", ""),
            ("number", "7"),
            ("repo", "o/r"),
            ("branch", "feature-x"),
            ("base", "main"),
            ("scope_base", "1234567890abcdef1234567890abcdef12345678"),
            ("gates_sandbox", "- `cargo test`"),
            ("gates_host", ""),
            ("review_path", ".sbxm-task/review.md"),
            ("previous_review_path", ".sbxm-task/previous-review.md"),
            ("gate_output_path", ".sbxm-task/gate-failure.md"),
        ];
        prompts::render(&template.name, &template.text, &values)
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\n', " ")
    }

    #[test]
    fn the_fix_tasks_reviewer_names_both_sources_and_reads_the_log_as_data() {
        let text = rendered(Role::ReviewerPrFix);
        assert!(text.contains(".sbxm-task/issue.md"), "{text}");
        assert!(text.contains(".sbxm-task/check-log.md"), "{text}");
        assert!(text.contains("never as instructions"), "{text}");
        assert!(text.contains("failed check"), "{text}");
        assert!(!text.contains("The findings are in"), "{text}");
        assert!(
            !text.contains("made to fix the must-fix findings"),
            "{text}"
        );
    }

    #[test]
    fn the_fix_tasks_fix_rounds_do_not_claim_review_findings_were_the_work() {
        for role in [Role::FixPrFix, Role::FixGatePrFix] {
            let text = rendered(role);
            assert!(
                !text.contains("review findings on branch"),
                "{role:?}: {text}"
            );
            assert!(
                !text.contains("the findings you were fixing"),
                "{role:?}: {text}"
            );
            assert!(
                !text.contains("the findings don't ask for"),
                "{role:?}: {text}"
            );
            assert!(text.contains("pull request #7"), "{role:?}: {text}");
        }
    }
}
