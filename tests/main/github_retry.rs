//! Issue 639: `GhBackend` tries a read-only `gh` call again after a network error, up to 3 times
//! after 5, 15 and 45 s, and never a write; the loop's queue read rides out one transient failure.

use crate::common;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use common::Env as TestEnv;
use common::task_fixture::Probe;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::gh::{GhBackend, GhRunner};
use sbxm::github::{GitHubBackend, PrRequest};

const REPO: &str = "o/r";
const TLS: &str = "Post \"https://api.github.com/graphql\": net/http: TLS handshake timeout";
const ISSUES: &str =
    r#"[{"number":3,"state":"OPEN","title":"Fix the 503 page","labels":[],"body":"EOF"}]"#;

/// Plays `gh`: records every call and answers with the queued outputs in order.
struct Scripted {
    calls: Mutex<Vec<Vec<String>>>,
    outputs: Mutex<Vec<Result<String, String>>>,
}

impl GhRunner for &'static Scripted {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(args.to_vec());
        match self.outputs.lock().unwrap().pop() {
            Some(Ok(out)) => Ok(out),
            Some(Err(message)) => bail!("{message}"),
            None => bail!("no scripted output left"),
        }
    }
}

/// What the backend waited and warned.
#[derive(Default)]
struct Seen {
    waits: Mutex<Vec<Duration>>,
    warnings: Mutex<Vec<String>>,
}

/// A backend over a leaked script, waiting and warning into `Seen`.
fn backend(outputs: Vec<Result<&str, &str>>) -> (GhBackend, &'static Scripted, Arc<Seen>) {
    let script: &'static Scripted = Box::leak(Box::new(Scripted {
        calls: Mutex::default(),
        outputs: Mutex::new(
            outputs
                .into_iter()
                .rev()
                .map(|o| o.map(str::to_owned).map_err(str::to_owned))
                .collect(),
        ),
    }));
    let seen = Arc::new(Seen::default());
    let (waits, warnings) = (Arc::clone(&seen), Arc::clone(&seen));
    let gh = GhBackend::with_runner(Box::new(script)).retrying_with(
        move |wait| waits.waits.lock().unwrap().push(wait),
        move |line| warnings.warnings.lock().unwrap().push(line.to_owned()),
    );
    (gh, script, seen)
}

fn secs(waits: &[u64]) -> Vec<Duration> {
    waits.iter().map(|s| Duration::from_secs(*s)).collect()
}

#[test]
fn a_read_that_fails_once_with_a_network_error_is_tried_again_after_5_s() {
    let (gh, script, seen) = backend(vec![Err(TLS), Ok(ISSUES)]);

    let issues = gh.issues_open(REPO).unwrap();

    assert_eq!(issues[0].number, 3);
    assert_eq!(script.calls.lock().unwrap().len(), 2);
    assert_eq!(*seen.waits.lock().unwrap(), secs(&[5]));
    assert_eq!(
        *seen.warnings.lock().unwrap(),
        ["gh issue list: network error, trying again in 5 s (1 of 3)"]
    );
}

#[test]
fn a_read_that_succeeds_on_the_4th_try_waited_5_15_and_45_s() {
    let (gh, script, seen) = backend(vec![
        Err(TLS),
        Err("read tcp 1.2.3.4:5->6.7.8.9:443: read: connection reset by peer"),
        Err("HTTP 502: Bad Gateway (https://api.github.com/graphql)"),
        Ok(ISSUES),
    ]);

    assert_eq!(gh.issues_open(REPO).unwrap().len(), 1);

    assert_eq!(script.calls.lock().unwrap().len(), 4);
    assert_eq!(*seen.waits.lock().unwrap(), secs(&[5, 15, 45]));
    assert_eq!(
        *seen.warnings.lock().unwrap(),
        [
            "gh issue list: network error, trying again in 5 s (1 of 3)",
            "gh issue list: network error, trying again in 15 s (2 of 3)",
            "gh issue list: network error, trying again in 45 s (3 of 3)",
        ]
    );
}

#[test]
fn a_read_that_fails_4_times_fails_with_the_network_error_message() {
    let (gh, script, seen) = backend(vec![
        Err(TLS),
        Err("dial tcp: i/o timeout"),
        Err("unexpected EOF"),
        Err(TLS),
        Ok(ISSUES),
    ]);

    let message = format!("{:#}", gh.issues_open(REPO).unwrap_err());

    assert_eq!(
        message,
        format!("`gh issue list` failed with a network error after 4 tries: {TLS}")
    );
    assert!(!message.contains("logged in"), "{message}");
    // Nothing ran a 5th time.
    assert_eq!(script.calls.lock().unwrap().len(), 4);
    assert_eq!(*seen.waits.lock().unwrap(), secs(&[5, 15, 45]));
}

#[test]
fn every_network_word_counts_in_any_case() {
    let words = [
        "TLS handshake timeout",
        "Connection Reset",
        "I/O TIMEOUT",
        "unexpected eof",
        "EOF",
        "connection refused",
        "dial tcp: lookup api.github.com: no such host",
        "timeout awaiting response headers",
        "HTTP 502",
        "HTTP 503",
        "HTTP 504",
    ];
    for word in words {
        let (gh, _, seen) = backend(vec![Err(word), Ok(ISSUES)]);
        assert!(gh.issues_open(REPO).is_ok(), "{word}");
        assert_eq!(*seen.waits.lock().unwrap(), secs(&[5]), "{word}");
    }
}

#[test]
fn an_auth_error_fails_at_once_with_the_hint() {
    for error in [
        "HTTP 401: Bad credentials (https://api.github.com/graphql)",
        "To get started with GitHub CLI, please run:  gh auth login",
        "HTTP 403: Resource not accessible",
    ] {
        let (gh, script, seen) = backend(vec![Err(error), Ok(ISSUES)]);

        let message = format!("{:#}", gh.issues_open(REPO).unwrap_err());

        assert!(
            message.contains("is `gh` installed and logged in? try `gh auth status`"),
            "{message}"
        );
        assert!(message.contains(error), "{message}");
        assert_eq!(script.calls.lock().unwrap().len(), 1, "{error}");
        assert!(seen.waits.lock().unwrap().is_empty());
        assert!(seen.warnings.lock().unwrap().is_empty());
    }
}

#[test]
fn a_network_error_after_a_retry_then_another_error_fails_with_the_hint() {
    let (gh, script, _) = backend(vec![Err(TLS), Err("HTTP 404: Not Found"), Ok(ISSUES)]);

    let message = format!("{:#}", gh.issues_open(REPO).unwrap_err());

    assert!(message.contains("logged in"), "{message}");
    assert!(message.contains("HTTP 404"), "{message}");
    assert_eq!(script.calls.lock().unwrap().len(), 2);
}

#[test]
fn writes_are_never_tried_again() {
    let request = PrRequest {
        head: "issue-3".into(),
        base: "main".into(),
        title: "t".into(),
        body: "b".into(),
        draft: false,
    };
    type Write = Box<dyn Fn(&GhBackend) -> Result<()>>;
    let writes: Vec<(&str, Write)> = vec![
        (
            "pr create",
            Box::new(move |gh| gh.pr_create(REPO, &request).map(drop)),
        ),
        ("pr comment", Box::new(|gh| gh.pr_comment(REPO, 7, "hi"))),
        (
            "api issues comment",
            Box::new(|gh| gh.issue_comment(REPO, 7, "hi").map(drop)),
        ),
        ("issue edit", Box::new(|gh| gh.issue_edit(REPO, 7, "body"))),
        ("pr merge", Box::new(|gh| gh.pr_disarm_auto_merge(REPO, 7))),
        ("run rerun", Box::new(|gh| gh.run_rerun_failed(REPO, 9))),
    ];
    for (name, write) in writes {
        let (gh, script, seen) = backend(vec![Err(TLS), Ok("")]);

        let message = format!("{:#}", write(&gh).unwrap_err());

        assert!(message.contains(TLS), "{name}: {message}");
        assert_eq!(script.calls.lock().unwrap().len(), 1, "{name}");
        assert!(seen.waits.lock().unwrap().is_empty(), "{name}");
        assert!(seen.warnings.lock().unwrap().is_empty(), "{name}");
    }
}

#[test]
fn network_words_in_a_successful_output_change_nothing() {
    let (gh, script, seen) = backend(vec![Ok(ISSUES)]);

    let issues = gh.issues_open(REPO).unwrap();

    assert_eq!(issues[0].title, "Fix the 503 page");
    assert_eq!(script.calls.lock().unwrap().len(), 1);
    assert!(seen.waits.lock().unwrap().is_empty());
    assert!(seen.warnings.lock().unwrap().is_empty());
}

// --- the loop's queue read ---

/// Plays `gh` for a loop over an empty queue: the first `gh issue list` fails with a network
/// error, every later one lists nothing.
#[derive(Default)]
struct Flaky {
    calls: Mutex<Vec<Vec<String>>>,
}

impl GhRunner for &'static Flaky {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(args.to_vec());
        let lists = calls.iter().filter(|c| c[..2] == ["issue", "list"]).count();
        match (args[0].as_str(), args[1].as_str()) {
            ("issue", "list") if lists == 1 => bail!("{TLS}"),
            ("api", "user") => Ok("me\n".into()),
            ("repo", "view") => Ok(r#"{"defaultBranchRef":{"name":"main"}}"#.into()),
            _ => Ok("[]".into()),
        }
    }
}

/// Phases the loop must never reach: the queue is empty.
struct NoSteps;

impl Steps for NoSteps {
    fn run(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of #{issue}"))
    }
    fn resume(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of #{issue}"))
    }
    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of #{issue}"))
    }
    fn file_findings(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected file-findings of #{issue}"))
    }
    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resolve of PR #{pr}"))
    }
    fn fix_check(&self, pr: u32, _: u64, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }
    fn rm_merged(&self, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }
    fn room(&self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn the_loop_goes_on_after_one_transient_queue_read_failure() {
    let env = TestEnv::new();
    let repo_root: PathBuf = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let flaky: &'static Flaky = Box::leak(Box::default());
    let seen = Arc::new(Seen::default());
    let (waits, warnings) = (Arc::clone(&seen), Arc::clone(&seen));
    let gh = GhBackend::with_runner(Box::new(flaky)).retrying_with(
        move |wait| waits.waits.lock().unwrap().push(wait),
        move |line| warnings.warnings.lock().unwrap().push(line.to_owned()),
    );
    let loop_env = Env {
        now: &|| 1_793_613_600,
        sleep: &|_: Duration| {},
        stop_requested: &|| false,
    };
    let opts = Options {
        repo_root,
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let mut out = Vec::new();

    let result = task_loop::run(
        &env.config_dir(),
        &opts,
        &NoSteps,
        &gh,
        &Probe,
        &loop_env,
        &mut out,
        &mut Vec::new(),
    );

    let out = String::from_utf8(out).unwrap();
    result.unwrap();
    assert!(out.contains("queue is empty"), "{out}");
    let lists = flaky
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c[..2] == ["issue", "list"])
        .count();
    assert!(lists >= 2, "the failed list was tried again: {lists}");
    assert_eq!(*seen.waits.lock().unwrap(), secs(&[5]));
    assert_eq!(
        *seen.warnings.lock().unwrap(),
        ["gh issue list: network error, trying again in 5 s (1 of 3)"]
    );
}
