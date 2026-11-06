//! Issue 448 (decision 300): `sbxm task fix --pr N --check-run <id>` works from a failed workflow
//! run's log: the last 64 KiB in all, split evenly between the run's failed jobs, cut at a line
//! boundary and never inside a UTF-8 character, given to the worker as `.sbxm-task/check-log.md`.
//! A run that isn't a failed run of the PR's current head is refused before anything is created.

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
    // One line longer than the whole share, so no line boundary is in reach: the cut falls in
    // the middle of a 4-byte character and moves past it.
    let mut log = String::new();
    while log.len() < CHECK_LOG_LIMIT + 2 {
        log.push('😀');
    }
    log.push_str("a\n");
    let start = log.len() - CHECK_LOG_LIMIT;
    assert!(
        !log.is_char_boundary(start),
        "the test must cut inside a character"
    );

    let text = check_log(&run(5, RunStatus::Failed), &[job("test", &log)]);

    // The text is a `String`, so it is valid UTF-8; the kept part is the log's own tail.
    let kept = &sections(&text)[0].1;
    assert!(log.ends_with(kept.as_str()));
    assert!(kept.len() <= CHECK_LOG_LIMIT && kept.len() > CHECK_LOG_LIMIT - 4);
    assert!(sections(&text)[0].0.contains("part of"), "{text:.200}");
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
