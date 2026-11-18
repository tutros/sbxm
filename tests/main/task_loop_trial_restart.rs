//! Issue 712: `task loop` records each medium arming in the PR's `task.json` (`medium_armed`), so a
//! loop started later still posts the trial's merge comment (once, with the arming loop's
//! `late_min` and `review_at`), watches the merge for a break, and counts toward the trial's 10.
//! A PR armed by hand has no such record and isn't counted. Most tests run two loops one after
//! the other on the same GitHub, as a restart does; the helpers are `task_loop_trial`'s. Until
//! the loop is done with the merge, `task rm --merged` leaves the task alone (review M-1), so the
//! record outlives the loop's own cleanup of merged tasks.

use crate::common;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use common::fix_task::yes_on_a_terminal;
use common::task_fixture::{Probe, backend};
use sbxm::commands::task_loop::Steps;
use sbxm::commands::task_rm;
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{RunStatus, WorkflowRun};
use sbxm::notify::FakeNotifier;
use sbxm::task::loop_pause;
use sbxm::task::merged;
use sbxm::task::record::{self, Record};
use sbxm::task::risk::Level;
use sbxm::task::trial::{self, Armed, How, Merge};

use super::task_loop_trial::{
    Fixture, HEAD, MERGED, PR, Ran, START, Script, TRIAL, WINDOW, armed, arms, fixture, github,
    run_from, run_once, run_passes,
};

/// When PR #120's clean review was posted: its window ended 10 min before the first loop.
const REVIEW_AT: u64 = START - WINDOW - 600;

/// When the second loop starts: 3 h after the first, so an arming of its own would differ.
const LATER: u64 = START + 3 * 3_600;

/// What the first loop records of PR #120, armed 10 min after its window ended.
const ARMED: Armed = Armed {
    how: How::WindowEnd,
    late_min: 10,
    review_at: REVIEW_AT,
};

/// The second loop, started at [`LATER`], for `passes` steps; `between(n)` after the `n`-th.
fn restarted(f: &Fixture, gh: &FakeGitHub, passes: usize, between: &dyn Fn(usize)) -> Ran {
    run_from(
        LATER,
        f,
        gh,
        &FakeNotifier::default(),
        passes,
        between,
        &Script,
    )
}

fn task_json(f: &Fixture, issue: u32) -> Record {
    record::read(&record::task_dir(&f.base(), &format!("issue-{issue}")).join("task.json")).unwrap()
}

/// Writes `armed` into issue `issue`'s task record, as a loop that armed its PR earlier did.
fn set_medium_armed(f: &Fixture, issue: u32, armed: Armed) {
    let dir = record::task_dir(&f.base(), &format!("issue-{issue}"));
    let mut rec = task_json(f, issue);
    rec.medium_armed = Some(armed);
    record::write(&dir, &rec).unwrap();
}

/// PR #120's merge comment, merged at `merged_at` after the first loop armed it.
fn merge_comment(merged_at: u64) -> String {
    trial::merge_comment(&Merge {
        pr: PR,
        armed: ARMED,
        holds: 0,
        merged_at,
    })
}

fn run(id: u64, attempt: u32, status: RunStatus) -> Vec<WorkflowRun> {
    vec![WorkflowRun {
        id,
        workflow: "ci".into(),
        attempt,
        status,
        url: format!("https://run/{id}"),
        actor: "me".into(),
    }]
}

#[test]
fn task_loop_trial_restart_the_arming_is_recorded_in_the_prs_task_json() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);

    run_once(&f, &gh).result.unwrap();

    assert_eq!(arms(&gh), [armed(PR)]);
    assert_eq!(task_json(&f, 20).medium_armed, Some(ARMED));
}

#[test]
fn task_loop_trial_restart_a_pr_merged_under_the_next_loop_gets_one_merge_comment() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    run_once(&f, &gh).result.unwrap();

    let ran = restarted(&f, &gh, 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, LATER + 600);
        }
    });

    ran.result.unwrap();
    assert_eq!(
        gh.issue_comments_now(TRIAL),
        [merge_comment(LATER + 600)],
        "{}",
        ran.out
    );
    assert_eq!(arms(&gh), [armed(PR)], "armed by the first loop only");
}

#[test]
fn task_loop_trial_restart_a_pr_merged_between_the_loops_gets_its_merge_comment() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    run_once(&f, &gh).result.unwrap();
    gh.set_merged(PR, MERGED);
    gh.set_merged_at(PR, START + 600);

    let ran = restarted(&f, &gh, 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(
        gh.issue_comments_now(TRIAL),
        [merge_comment(START + 600)],
        "{}",
        ran.out
    );
}

#[test]
fn task_loop_trial_restart_a_red_main_after_the_merge_is_the_trials_break_and_pauses() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    run_once(&f, &gh).result.unwrap();

    let ran = restarted(&f, &gh, 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, LATER + 600);
            gh.set_runs(MERGED, run(77, 2, RunStatus::Failed));
        }
    });

    ran.result.unwrap();
    assert!(loop_pause::read(&f.base()).paused(), "{}", ran.out);
    let comments = gh.issue_comments_now(TRIAL);
    assert_eq!(comments.len(), 2, "{comments:?}");
    assert_eq!(comments[0], merge_comment(LATER + 600));
    assert!(
        comments[1].starts_with(
            "Medium trial: break\n\nmain went red after the medium auto-merge of PR #120"
        ),
        "{comments:?}"
    );
}

#[test]
fn task_loop_trial_restart_a_merge_already_commented_gets_no_second_comment() {
    let f = fixture();
    f.medium_pr();
    set_medium_armed(&f, 20, ARMED);
    let gh = github(&[20], HEAD, 0, REVIEW_AT).with_issue_comments(TRIAL, &[&merge_comment(START)]);
    gh.set_merged(PR, MERGED);
    gh.set_merged_at(PR, START);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL), [merge_comment(START)]);
}

#[test]
fn task_loop_trial_restart_the_record_is_dropped_once_the_trial_is_done_with_the_merge() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, START + 600);
            gh.set_runs(MERGED, run(78, 1, RunStatus::Passed));
        }
    });
    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL).len(), 1, "{}", ran.out);
    assert_eq!(task_json(&f, 20).medium_armed, None, "{}", ran.out);
    // Even with the merge comment gone, a later loop has nothing left to record.
    gh.set_runs(MERGED, run(79, 2, RunStatus::Failed));

    let ran = restarted(&f, &gh, 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL).len(), 1, "{}", ran.out);
}

#[test]
fn task_loop_trial_restart_the_record_stays_while_the_merge_commit_is_watched() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    let ran = run_passes(&f, &gh, &FakeNotifier::default(), 2, &|pass| {
        if pass == 1 {
            gh.set_merged(PR, MERGED);
            gh.set_merged_at(PR, START + 600);
            gh.set_runs(MERGED, run(78, 1, RunStatus::Going));
        }
    });
    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL).len(), 1, "{}", ran.out);
    assert_eq!(task_json(&f, 20).medium_armed, Some(ARMED));

    let ran = restarted(&f, &gh, 1, &|_| {});

    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL).len(), 1, "{}", ran.out);
}

#[test]
fn task_loop_trial_restart_the_record_is_dropped_when_the_loop_disarms_the_pr() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    run_once(&f, &gh).result.unwrap();
    gh.set_pr_labels(PR, &["hold"]);

    let ran = restarted(&f, &gh, 1, &|_| {});

    ran.result.unwrap();
    assert!(
        ran.out
            .contains("PR #120 (Fix 20): auto-merge disarmed: labelled `hold`"),
        "{}",
        ran.out
    );
    assert_eq!(task_json(&f, 20).medium_armed, None);
}

#[test]
fn task_loop_trial_restart_a_pr_armed_by_hand_isnt_counted() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT).with_auto_merge(PR);
    gh.set_merged(PR, MERGED);
    gh.set_merged_at(PR, START);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert!(gh.issue_comments_now(TRIAL).is_empty(), "{}", ran.out);
}

#[test]
fn task_loop_trial_restart_the_tenth_merge_counted_after_a_restart_arms_no_eleventh() {
    let f = fixture();
    f.medium_pr();
    f.loop_pr(21, Level::Medium, "Do it too.\n");
    // An earlier loop armed #120 with 9 merges in; it merged before this loop started.
    set_medium_armed(&f, 20, ARMED);
    let gh = github(&[20, 21], HEAD, 9, REVIEW_AT);
    gh.set_merged(PR, MERGED);
    gh.set_merged_at(PR, START);

    let ran = run_once(&f, &gh);

    ran.result.unwrap();
    assert_eq!(
        trial::merges(&gh.issue_comments_now(TRIAL)),
        10,
        "{}",
        ran.out
    );
    assert!(arms(&gh).is_empty(), "{}", ran.out);
}

// --- With the loop's real cleanup of merged tasks (review M-1) ---

/// The phases of [`Script`], but the loop's cleanup of merged tasks is the real `task rm
/// --merged --yes`.
struct Cleaning<'a> {
    config_dir: PathBuf,
    repo_root: PathBuf,
    gh: &'a FakeGitHub,
}

impl Cleaning<'_> {
    fn of<'a>(f: &Fixture, gh: &'a FakeGitHub) -> Cleaning<'a> {
        Cleaning {
            config_dir: f.env.config_dir(),
            repo_root: f.repo_root.clone(),
            gh,
        }
    }
}

impl Steps for Cleaning<'_> {
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
    fn rm_merged(&self, out: &mut dyn Write, _: &mut dyn Write) -> Result<()> {
        task_rm::run(
            &self.config_dir,
            &task_rm::Options {
                target: task_rm::Target::Merged,
                yes: true,
                force: false,
                repo_root: self.repo_root.clone(),
                dry_run: false,
                forget_ambiguous: false,
            },
            &backend(),
            self.gh,
            &Probe,
            &yes_on_a_terminal(),
            out,
        )
    }
    fn room(&self) -> Result<()> {
        Ok(())
    }
}

fn task_dir(f: &Fixture, issue: u32) -> PathBuf {
    record::task_dir(&f.base(), &format!("issue-{issue}"))
}

#[test]
fn task_loop_trial_restart_task_rm_merged_leaves_a_task_whose_arming_is_kept() {
    let f = fixture();
    f.medium_pr();
    set_medium_armed(&f, 20, ARMED);
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    gh.set_merged(PR, MERGED);

    let plan = merged::plan(&f.base(), &gh, &Probe).unwrap();

    assert!(plan.candidates.is_empty(), "{}", plan.listing());
    assert_eq!(plan.left_alone.len(), 1, "{}", plan.listing());
    assert_eq!(plan.left_alone[0].id, "issue-20");
    assert!(
        plan.left_alone[0].reason.contains("medium trial"),
        "{}",
        plan.left_alone[0].reason
    );
}

#[test]
fn task_loop_trial_restart_a_tenth_merge_left_pending_survives_the_cleanup_and_a_restart() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20, 21], HEAD, 9, REVIEW_AT);
    let steps = Cleaning::of(&f, &gh);
    // The first loop arms #120; it merges, and the merge comment can't be posted.
    let ran = run_from(
        START,
        &f,
        &gh,
        &FakeNotifier::default(),
        2,
        &|pass| {
            if pass == 1 {
                gh.set_merged(PR, MERGED);
                gh.set_merged_at(PR, START + 600);
                gh.set_issue_comments_failing(TRIAL, true);
            }
        },
        &steps,
    );
    ran.result.unwrap();
    assert_eq!(
        trial::merges(&gh.issue_comments_now(TRIAL)),
        9,
        "{}",
        ran.out
    );
    assert!(task_dir(&f, 20).exists(), "{}", ran.out);
    gh.set_issue_comments_failing(TRIAL, false);
    // #121's task finished meanwhile, its window over.
    f.loop_pr(21, Level::Medium, "Do it too.\n");

    let ran = run_from(LATER, &f, &gh, &FakeNotifier::default(), 1, &|_| {}, &steps);

    ran.result.unwrap();
    let comments = gh.issue_comments_now(TRIAL);
    assert_eq!(trial::merges(&comments), 10, "{}", ran.out);
    assert_eq!(comments[9], merge_comment(START + 600));
    assert_eq!(arms(&gh), [armed(PR)], "{}", ran.out);
}

#[test]
fn task_loop_trial_restart_a_break_after_the_cleanup_and_a_restart_is_posted_once() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    let steps = Cleaning::of(&f, &gh);
    // The first loop arms #120, it merges, the merge comment is posted; its CI still runs.
    let ran = run_from(
        START,
        &f,
        &gh,
        &FakeNotifier::default(),
        3,
        &|pass| {
            if pass == 1 {
                gh.set_merged(PR, MERGED);
                gh.set_merged_at(PR, START + 600);
                gh.set_runs(MERGED, run(77, 1, RunStatus::Going));
            }
        },
        &steps,
    );
    ran.result.unwrap();
    assert_eq!(
        gh.issue_comments_now(TRIAL),
        [merge_comment(START + 600)],
        "{}",
        ran.out
    );
    assert!(task_dir(&f, 20).exists(), "{}", ran.out);

    // Restarted: `main` moved on, and the merge commit's run fails again after its re-run.
    let ran = run_from(
        LATER,
        &f,
        &gh,
        &FakeNotifier::default(),
        2,
        &|pass| {
            if pass == 1 {
                gh.set_branch_head("main", "cccccccccccccccccccccccccccccccccccccccc");
                gh.set_runs(MERGED, run(77, 2, RunStatus::Failed));
            }
        },
        &steps,
    );

    ran.result.unwrap();
    assert!(loop_pause::read(&f.base()).paused(), "{}", ran.out);
    let comments = gh.issue_comments_now(TRIAL);
    assert_eq!(comments.len(), 2, "{comments:?}\n{}", ran.out);
    assert!(
        comments[1].starts_with(
            "Medium trial: break\n\nmain went red after the medium auto-merge of PR #120"
        ),
        "{comments:?}"
    );
}

#[test]
fn task_loop_trial_restart_the_cleanup_removes_the_task_once_the_trial_is_done_with_it() {
    let f = fixture();
    f.medium_pr();
    let gh = github(&[20], HEAD, 0, REVIEW_AT);
    let steps = Cleaning::of(&f, &gh);

    let ran = run_from(
        START,
        &f,
        &gh,
        &FakeNotifier::default(),
        3,
        &|pass| {
            if pass == 1 {
                gh.set_merged(PR, MERGED);
                gh.set_merged_at(PR, START + 600);
                gh.set_runs(MERGED, run(78, 1, RunStatus::Passed));
            }
        },
        &steps,
    );

    ran.result.unwrap();
    assert_eq!(gh.issue_comments_now(TRIAL).len(), 1, "{}", ran.out);
    assert!(!task_dir(&f, 20).exists(), "{}", ran.out);
}
