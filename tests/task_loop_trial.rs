//! Issue 550 (decision 316(3) and (4)): `task loop` arms a medium-risk PR once its veto window
//! ends or it is labelled `go`, through the step that re-checks waiting PRs between tasks; `hold`
//! never arms and disarms one armed; the medium pause (`--pause-medium`) stops medium only; the
//! `medium-trial` issue's 10 merge comments end the arming; a medium PR the loop armed that merges
//! gets a merge comment there (how it was armed, holds, minutes), a red `main` after one a break
//! comment. The same step arms a PR held only for its real check once it passes (#547 answer 3).
//! No task runs: the PRs are earlier loop tasks', the phases are scripted and the PRs are
//! `FakeGitHub`'s.

mod common;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, issue_text};
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{
    CommitStatus, GitHubBackend, Issue, LabelEvent, PostedStatus, PrFile, PrFiles, PrInfo, PrState,
    RunStatus, StatusState, WorkflowRun,
};
use sbxm::notify::FakeNotifier;
use sbxm::task::loop_pause::{self, MediumPause};
use sbxm::task::record::{self, Kind, NewTask, Process, Record, ReviewResult, Stage, Status};
use sbxm::task::risk::Level;
use sbxm::task::trial::{self, Armed, How, Merge};

const REPO: &str = "o/r";
const START: u64 = 1_793_613_600;
/// The PRs' heads, the commit every review covered.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// A PR's head after a push no review covered.
const PUSHED: &str = "1111111111111111111111111111111111111111";
const MAIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MERGED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WINDOW: u64 = 2 * 3_600;
const TRIAL: u32 = 568;
/// The medium PR of the tests, issue 20's.
const PR: u32 = 120;

/// No task runs in these tests: the queue is empty.
struct Script;

impl Steps for Script {
    fn run(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected run of {issue}"))
    }
    fn resume(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected resume of {issue}"))
    }
    fn finish(&self, issue: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected finish of {issue}"))
    }
    fn file_findings(&self, _: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Ok(())
    }
    fn fix_pr(&self, pr: u32, _: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        Err(anyhow!("unexpected fix of PR #{pr}"))
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

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

fn fixture() -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    Fixture { env, repo_root }
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn log_text(&self) -> String {
        let dir = self.base().join(".sbxm/loop-logs");
        fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|e| fs::read_to_string(e.ok()?.path()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A task of issue `issue` a loop finished into PR `100 + issue`, its clean review of
    /// [`HEAD`] at `risk`, its issue text `body`.
    fn loop_pr(&self, issue: u32, risk: Level, body: &str) {
        let id = format!("issue-{issue}");
        let dir = record::task_dir(&self.base(), &id);
        let mut rec = Record::new(
            &NewTask {
                kind: Kind::Issue,
                number: issue,
                repo: REPO,
                title: &format!("Fix {issue}"),
                base: "main",
                branch: &format!("sbxm/{id}"),
                config_hash: "h",
                id: None,
            },
            START,
            Process::new(1, 0),
        );
        rec.stage = Stage::Finished;
        rec.status = Status::Ok;
        rec.review = Some(ReviewResult {
            round: 1,
            commit: Some(HEAD.to_owned()),
            must_fix: 0,
            risk,
            not_run: Some(Vec::new()),
            ..ReviewResult::default()
        });
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        rec.hooks = serde_json::json!({ task_loop::LOOP_HOOK: true });
        record::write(&dir, &rec).unwrap();
        fs::write(
            dir.join("issue.md"),
            format!("title:\tFix {issue}\nstate:\tOPEN\nlabels:\tsize:M\n--\n{body}"),
        )
        .unwrap();
    }

    fn medium_pr(&self) {
        self.loop_pr(PR - 100, Level::Medium, "Do it.\n");
    }
}

fn pr(number: u32, head: &str) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("b{number}"),
        head_oid: head.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "t".into(),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

fn trial_issue() -> Issue {
    Issue {
        number: TRIAL,
        open: true,
        title: "Medium trial".into(),
        labels: vec![trial::LABEL.into()],
        body: String::new(),
    }
}

/// A clean `sbxm/review` status posted at `at`.
fn reviewed_at(at: u64) -> Vec<PostedStatus> {
    vec![PostedStatus {
        context: "sbxm/review".into(),
        state: "success".into(),
        at,
    }]
}

/// GitHub with PR `100 + issue` open at `head` for each of `issues` (each size M, changing one
/// source file), the trial's issue open with `merges` merge comments, and the review of [`HEAD`]
/// posted at `review_at`.
fn github(issues: &[u32], head: &str, merges: usize, review_at: u64) -> FakeGitHub {
    let files = PrFiles {
        changed_files: 1,
        files: vec![PrFile {
            filename: "src/a.rs".into(),
            previous_filename: None,
        }],
    };
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", MAIN)
        .with_open_issues(vec![trial_issue()])
        .with_statuses(HEAD, reviewed_at(review_at));
    for &n in issues {
        let mut text = issue_text(n);
        text.text = format!("title:\tFix {n}\nstate:\tOPEN\nlabels:\tsize:M\n--\nbody\n");
        gh = gh
            .with_issue_text(text)
            .with_pr(pr(100 + n, head))
            .with_pr_files(100 + n, files.clone());
    }
    let comments: Vec<String> = (0..merges)
        .map(|i| {
            trial::merge_comment(&Merge {
                pr: 1 + i as u32,
                armed: Armed {
                    how: How::Go,
                    late_min: 0,
                    review_at: 0,
                },
                holds: 0,
                merged_at: 0,
            })
        })
        .collect();
    let refs: Vec<&str> = comments.iter().map(String::as_str).collect();
    gh.with_issue_comments(TRIAL, &refs)
}

struct Ran {
    result: Result<()>,
    out: String,
    warn: String,
}

/// Runs the loop with `--wait` for `passes` steps; `between(n)` runs after the `n`-th pass
/// (1-based), as the world moves meanwhile.
fn run_passes(
    f: &Fixture,
    gh: &FakeGitHub,
    notifier: &FakeNotifier,
    passes: usize,
    between: &dyn Fn(usize),
) -> Ran {
    let clock = Cell::new(START);
    let sleeps = Cell::new(0usize);
    let now = || clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        clock.set(clock.get() + d.as_secs());
        between(sleeps.get());
    };
    let stop = || sleeps.get() >= passes;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let opts = Options {
        repo_root: f.repo_root.clone(),
        issues: Vec::new(),
        cap: None,
        repo: Some(REPO.into()),
        wait: true,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let result = task_loop::run_notifying(
        &f.env.config_dir(),
        &opts,
        &Script,
        gh,
        &Probe,
        &env,
        notifier,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

/// One step of the loop.
fn run_once(f: &Fixture, gh: &FakeGitHub) -> Ran {
    run_passes(f, gh, &FakeNotifier::default(), 1, &|_| {})
}

fn arms(gh: &FakeGitHub) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrArmAutoMerge(..)))
        .collect()
}

fn armed(number: u32) -> GhCall {
    GhCall::PrArmAutoMerge(REPO.into(), number, HEAD.into())
}

fn label(gh: &FakeGitHub, labels: &[&str]) {
    gh.set_pr_labels(PR, labels);
}

// --- The window ---

#[test]
fn one_second_before_the_windows_end_doesnt_arm() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW + 1);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .contains("PR #120 (Fix 20) awaits you: risk medium; its veto window ends at"),
        "{}",
        ran.out
    );
}

#[test]
fn at_the_windows_end_it_arms_on_the_reviewed_head() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(PR)], "{}", ran.out);
    assert!(
        ran.out.contains(&format!(
            "PR #120 (Fix 20): auto-merge armed on {HEAD} (medium, its veto window ended)"
        )),
        "{}",
        ran.out
    );
    assert!(
        f.log_text()
            .contains("auto-merge armed (medium, its veto window ended)"),
        "{}",
        f.log_text()
    );
}

#[test]
fn after_the_windows_end_it_arms() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW - 3_600);

    run_once(&f, &gh).result.unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

#[test]
fn go_arms_before_the_windows_end() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 60);
    label(&gh, &["go"]);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(PR)], "{}", ran.out);
    assert!(ran.out.contains("(medium, labelled `go`)"), "{}", ran.out);
}

#[test]
fn hold_never_arms_even_after_the_windows_end() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW);
    label(&gh, &["hold"]);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out.contains("awaits you: risk medium; labelled `hold`"),
        "{}",
        ran.out
    );
}

#[test]
fn hold_and_go_together_hold() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 60);
    label(&gh, &["Go", "HOLD"]);

    run_once(&f, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

#[test]
fn hold_disarms_a_medium_pr_already_armed() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW).with_auto_merge(PR);
    label(&gh, &["hold"]);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(
        gh.calls()
            .contains(&GhCall::PrDisarmAutoMerge(REPO.into(), PR)),
        "{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("PR #120 (Fix 20): auto-merge disarmed: labelled `hold`"),
        "{}",
        ran.out
    );
    assert!(arms(&gh).is_empty());
}

#[test]
fn hold_leaves_an_armed_low_risk_pr_alone() {
    let f = fixture();
    f.loop_pr(20, Level::Low, "Do it.\n");
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW).with_auto_merge(PR);
    label(&gh, &["hold"]);

    run_once(&f, &gh).result.unwrap();

    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrDisarmAutoMerge(..)))
    );
}

#[test]
fn a_medium_pr_whose_head_moved_after_the_review_has_no_window() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], PUSHED, 0, START - 3 * WINDOW);
    label(&gh, &["go"]);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out.contains(&format!(
            "awaits you: risk medium; its head {PUSHED} moved after its clean review, so it has \
             no veto window"
        )),
        "{}",
        ran.out
    );
}

#[test]
fn a_medium_pr_of_size_l_isnt_armed_at_its_windows_end() {
    let f = fixture();
    f.medium_pr();
    let mut text = issue_text(20);
    text.text = "title:\tFix 20\nstate:\tOPEN\nlabels:\tsize:L\n--\nbody\n".into();
    let gh = github(&[], HEAD, 0, START - 3 * WINDOW)
        .with_issue_text(text)
        .with_pr(pr(PR, HEAD))
        .with_pr_files(
            PR,
            PrFiles {
                changed_files: 1,
                files: vec![PrFile {
                    filename: "src/a.rs".into(),
                    previous_filename: None,
                }],
            },
        );

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(
        ran.out
            .contains("awaits you: risk medium; its issue #20 is size L"),
        "{}",
        ran.out
    );
}

#[test]
fn a_high_risk_pr_is_never_armed() {
    let f = fixture();
    f.loop_pr(20, Level::High, "Do it.\n");
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW);
    label(&gh, &["go"]);

    run_once(&f, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

#[test]
fn a_waiting_window_is_announced_once_on_the_pr_and_by_a_toast() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 60);
    let notifier = FakeNotifier::default();

    let ran = run_passes(&f, &gh, &notifier, 2, &|_| {});

    ran.result.unwrap();
    let comments: Vec<GhCall> = gh
        .calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::PrComment(_, PR, _)))
        .collect();
    assert_eq!(comments.len(), 1, "{comments:?}");
    let titles: Vec<String> = notifier.shown().iter().map(|n| n.title.clone()).collect();
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].starts_with("PR #120 arms at"), "{titles:?}");
}

#[test]
fn the_window_found_over_on_a_later_step_arms_then() {
    let f = fixture();
    f.medium_pr();
    // The review 1 h 50 min ago: the second step, 5 min later... is still in it; the third,
    // 10 min after the first, is after its end.
    let gh = github(&[20], HEAD, 0, START - WINDOW + 600);

    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 3, &|_| {});

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(PR)], "{}", ran.out);
}

// --- The trial's count ---

#[test]
fn nine_merge_comments_arm() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 9, START - WINDOW);

    run_once(&f, &gh).result.unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

#[test]
fn ten_merge_comments_dont_arm_and_say_so() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 10, START - WINDOW);
    let notifier = FakeNotifier::default();

    let ran = run_passes(&f, &gh, &notifier, 1, &|_| {});

    ran.result.unwrap();
    assert!(arms(&gh).is_empty(), "{}", ran.out);
    assert!(
        ran.out
            .contains("awaits you: risk medium; the medium trial has its 10 auto-merges"),
        "{}",
        ran.out
    );
    let shown = notifier.shown();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].title, "PR #120 awaits your merge (medium)");
    assert!(
        shown[0]
            .body
            .contains("the medium trial has its 10 auto-merges"),
        "{}",
        shown[0].body
    );
}

#[test]
fn eleven_merge_comments_dont_arm() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 11, START - WINDOW);

    run_once(&f, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

#[test]
fn break_comments_dont_count_toward_the_ten() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 9, START - WINDOW).with_issue_comments(
        TRIAL,
        &[&trial::break_comment("a"), &trial::break_comment("b")],
    );

    run_once(&f, &gh).result.unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

#[test]
fn a_trial_whose_comments_cant_be_read_doesnt_arm() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW).failing_issue_comment_reads("HTTP 502");

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(arms(&gh).is_empty());
    assert!(ran.out.contains("can't be read"), "{}", ran.out);
}

#[test]
fn with_no_trial_issue_it_arms_and_warns_once() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW);
    gh.set_open_issues(Vec::new());

    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|_| {});

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(PR)]);
    assert_eq!(
        ran.warn
            .matches("no open issue is labelled medium-trial")
            .count(),
        1,
        "{}",
        ran.warn
    );
}

// --- The merge comment, with its holds ---

/// Arms PR #120 at its window's end, 10 min late, then GitHub merges it 30 min after the
/// arming with the label history `events`; returns the comments posted on the trial's issue.
fn merged_with(events: Vec<LabelEvent>) -> (Ran, Vec<String>) {
    let f = fixture();
    f.medium_pr();
    let review_at = START - WINDOW - 600;
    let gh = github(&[20], HEAD, 0, review_at);
    gh.set_label_events(PR, events);
    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, START + 1_800);
        }
    });
    (ran, gh.issue_comments_now(TRIAL))
}

fn hold(added: bool) -> LabelEvent {
    LabelEvent {
        label: "hold".into(),
        added,
        at: START - 1_000,
    }
}

#[test]
fn a_merged_medium_pr_gets_one_merge_comment_with_no_holds() {
    let (ran, comments) = merged_with(Vec::new());
    ran.result.unwrap();
    assert_eq!(
        comments,
        [trial::merge_comment(&Merge {
            pr: PR,
            armed: Armed {
                how: How::WindowEnd,
                late_min: 10,
                review_at: START - WINDOW - 600,
            },
            holds: 0,
            merged_at: START + 1_800,
        })],
        "{}",
        ran.out
    );
    // Clean review to merge: 2 h 10 min before the arming, then 30 min.
    assert!(comments[0].contains("- clean review to merge: 160 min"));
}

#[test]
fn one_hold_is_counted() {
    let (_, comments) = merged_with(vec![hold(true), hold(false)]);
    assert!(comments[0].contains("- holds: 1\n"), "{comments:?}");
}

#[test]
fn a_hold_added_removed_and_added_again_counts_two() {
    let (_, comments) = merged_with(vec![hold(true), hold(false), hold(true), hold(false)]);
    assert!(comments[0].contains("- holds: 2\n"), "{comments:?}");
}

#[test]
fn a_go_arming_records_minutes_from_the_go_label() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 60);
    label(&gh, &["go"]);
    gh.set_label_events(
        PR,
        vec![LabelEvent {
            label: "go".into(),
            added: true,
            at: START - 420,
        }],
    );

    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, START + 600);
        }
    });

    ran.result.unwrap();
    let comments = gh.issue_comments_now(TRIAL);
    assert_eq!(comments.len(), 1, "{}", ran.out);
    assert!(
        comments[0].contains("- armed by: `go`, 7 min after it was labelled"),
        "{comments:?}"
    );
}

#[test]
fn a_red_main_after_a_medium_auto_merge_posts_a_break_and_pauses() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - WINDOW);
    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, START + 600);
            gh.set_runs(
                MERGED,
                vec![WorkflowRun {
                    id: 77,
                    workflow: "ci".into(),
                    attempt: 2,
                    status: RunStatus::Failed,
                    url: "https://run/77".into(),
                    actor: "me".into(),
                }],
            );
        }
    });

    ran.result.unwrap();
    assert!(loop_pause::read(&f.base()).paused(), "{}", ran.out);
    let comments = gh.issue_comments_now(TRIAL);
    assert_eq!(comments.len(), 2, "{comments:?}");
    assert!(
        comments[1].starts_with(
            "Medium trial: break\n\nmain went red after the medium auto-merge of PR #120"
        ),
        "{comments:?}"
    );
    assert_eq!(trial::merges(&comments), 1);
}

#[test]
fn a_red_main_after_a_low_risk_merge_posts_no_break() {
    let f = fixture();
    f.loop_pr(20, Level::Low, "**Real check:** run it\n");
    let gh = github(&[20], HEAD, 0, START - WINDOW);
    gh.commit_status(
        REPO,
        HEAD,
        &CommitStatus {
            state: StatusState::Success,
            context: "sbxm/real-check".into(),
            description: "ok".into(),
        },
    )
    .unwrap();
    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_runs(
                MERGED,
                vec![WorkflowRun {
                    id: 77,
                    workflow: "ci".into(),
                    attempt: 2,
                    status: RunStatus::Failed,
                    url: "https://run/77".into(),
                    actor: "me".into(),
                }],
            );
        }
    });

    ran.result.unwrap();
    assert!(loop_pause::read(&f.base()).paused(), "{}", ran.out);
    assert!(gh.issue_comments_now(TRIAL).is_empty());
}

// --- The pauses ---

fn real_check_passed(gh: &FakeGitHub, sha: &str) {
    gh.commit_status(
        REPO,
        sha,
        &CommitStatus {
            state: StatusState::Success,
            context: "sbxm/real-check".into(),
            description: "ok".into(),
        },
    )
    .unwrap();
}

#[test]
fn the_medium_pause_holds_medium_while_low_still_arms_in_the_same_step() {
    let f = fixture();
    f.medium_pr();
    f.loop_pr(21, Level::Low, "**Real check:** run it\n");
    loop_pause::write_medium(
        &f.base(),
        &MediumPause {
            why: "PR #9 needed a fix".into(),
            at: "t".into(),
        },
    )
    .unwrap();
    let gh = github(&[20, 21], HEAD, 0, START - 3 * WINDOW);
    real_check_passed(&gh, HEAD);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(121)], "{}", ran.out);
    assert!(
        ran.out.contains(
            "PR #120 (Fix 20) awaits you: risk medium; medium auto-merge is paused (PR #9 \
             needed a fix) until `sbxm task loop --clear-pause`"
        ),
        "{}",
        ran.out
    );
}

#[test]
fn after_clear_pause_medium_arms_again() {
    let f = fixture();
    f.medium_pr();
    loop_pause::write_medium(
        &f.base(),
        &MediumPause {
            why: "w".into(),
            at: "t".into(),
        },
    )
    .unwrap();
    task_loop::clear_pause(&f.env.config_dir(), &mut Vec::new()).unwrap();
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW);

    run_once(&f, &gh).result.unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

#[test]
fn a_red_main_pause_holds_medium_and_low() {
    let f = fixture();
    f.medium_pr();
    f.loop_pr(21, Level::Low, "**Real check:** run it\n");
    loop_pause::write(
        &f.base(),
        &loop_pause::Pause {
            commit: MAIN.into(),
            run_id: 1,
            run_url: "u".into(),
            workflow: "ci".into(),
            merged_by: "me".into(),
            at: "t".into(),
        },
    )
    .unwrap();
    let gh = github(&[20, 21], HEAD, 0, START - 3 * WINDOW);
    real_check_passed(&gh, HEAD);

    run_once(&f, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

// --- Arming later, for a real check ---

#[test]
fn a_low_pr_held_for_its_real_check_arms_once_it_passes() {
    let f = fixture();
    f.loop_pr(21, Level::Low, "**Real check:** run it\n");
    let gh = github(&[21], HEAD, 0, START);
    real_check_passed(&gh, HEAD);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(arms(&gh), [armed(121)], "{}", ran.out);
    assert!(ran.out.contains("(its real check passed)"), "{}", ran.out);
}

#[test]
fn a_low_pr_whose_real_check_failed_or_has_none_isnt_armed() {
    for state in [Some(StatusState::Failure), None] {
        let f = fixture();
        f.loop_pr(21, Level::Low, "**Real check:** run it\n");
        let gh = github(&[21], HEAD, 0, START);
        if let Some(state) = state {
            gh.commit_status(
                REPO,
                HEAD,
                &CommitStatus {
                    state,
                    context: "sbxm/real-check".into(),
                    description: "x".into(),
                },
            )
            .unwrap();
        }

        run_once(&f, &gh).result.unwrap();

        assert!(arms(&gh).is_empty(), "{state:?}");
    }
}

#[test]
fn a_low_pr_with_no_real_check_isnt_armed_by_the_step() {
    // Not armed when it finished (an arming that failed, say): the step leaves it to the user.
    let f = fixture();
    f.loop_pr(21, Level::Low, "Do it.\n");
    let gh = github(&[21], HEAD, 0, START);

    run_once(&f, &gh).result.unwrap();

    assert!(arms(&gh).is_empty());
}

#[test]
fn a_medium_pr_held_for_its_real_check_arms_only_once_it_passes() {
    let f = fixture();
    f.loop_pr(20, Level::Medium, "**Real check:** run it\n");
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW);

    run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            assert!(arms(&gh).is_empty());
            real_check_passed(&gh, HEAD);
        }
    })
    .result
    .unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

#[test]
fn the_step_doesnt_arm_the_same_head_twice_after_the_user_disarmed_it() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, START - 3 * WINDOW);

    run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.pr_disarm_auto_merge(REPO, PR).unwrap();
        }
    })
    .result
    .unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
}

// --- Toasts ---

#[test]
fn a_medium_pr_in_its_window_gets_only_the_windows_toast_after_its_real_check_passed() {
    let f = fixture();
    f.loop_pr(20, Level::Medium, "**Real check:** run it\n");
    let gh = github(&[20], HEAD, 0, START - 60);
    real_check_passed(&gh, HEAD);
    let notifier = FakeNotifier::default();

    run_passes(&f, &gh, &notifier, 1, &|_| {}).result.unwrap();

    let titles: Vec<String> = notifier.shown().iter().map(|n| n.title.clone()).collect();
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].starts_with("PR #120 arms at"), "{titles:?}");
}
