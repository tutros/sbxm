//! Issue 550 (decision 316(4), answer 3): `sbxm task loop --pause-medium "<why>"` records a break
//! after a medium auto-merge that the loop can't see: it sets the medium pause, which stops the
//! arming of medium-risk PRs only, and posts a break comment on the open `medium-trial` issue.

use crate::common;

use common::Env as TestEnv;
use sbxm::commands::task_loop;
use sbxm::github::Issue;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::loop_pause::{self, MediumState};
use sbxm::task::trial;

const REPO: &str = "o/r";
const NOW: u64 = 1_793_613_600;

fn trial_issue() -> Issue {
    Issue {
        number: 568,
        open: true,
        title: "Medium trial".into(),
        labels: vec![trial::LABEL.into()],
        body: String::new(),
    }
}

struct Ran {
    result: anyhow::Result<()>,
    out: String,
    warn: String,
}

fn pause(env: &TestEnv, gh: &FakeGitHub, why: &str) -> Ran {
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    let now = || NOW;
    let result = task_loop::pause_medium(
        &env.config_dir(),
        &env.tmp.path().join("repo"),
        Some(REPO),
        why,
        gh,
        &now,
        &mut out,
        &mut warn,
    );
    Ran {
        result,
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

#[test]
fn it_sets_the_medium_pause_and_posts_a_break_comment() {
    let env = TestEnv::new();
    let gh = FakeGitHub::default().with_open_issues(vec![trial_issue()]);

    let ran = pause(&env, &gh, "PR #9 needed a fix");

    ran.result.unwrap();
    let MediumState::Set(set) = loop_pause::read_medium(&env.base_dir()) else {
        panic!("not set: {}", ran.out);
    };
    assert_eq!(set.why, "PR #9 needed a fix");
    assert_eq!(
        gh.issue_comments_now(568),
        [trial::break_comment("PR #9 needed a fix")]
    );
    assert!(
        ran.out
            .contains("medium auto-merge paused: PR #9 needed a fix"),
        "{}",
        ran.out
    );
    assert!(ran.out.contains("--clear-pause"), "{}", ran.out);
    // The red-main pause isn't set: low risk keeps arming.
    assert!(!loop_pause::read(&env.base_dir()).paused());
}

#[test]
fn with_no_trial_issue_it_pauses_and_warns() {
    let env = TestEnv::new();
    let gh = FakeGitHub::default();

    let ran = pause(&env, &gh, "a break");

    ran.result.unwrap();
    assert!(loop_pause::read_medium(&env.base_dir()).paused());
    assert!(
        ran.warn.contains("no open issue is labelled medium-trial"),
        "{}",
        ran.warn
    );
}

#[test]
fn a_comment_that_fails_still_leaves_the_pause() {
    let env = TestEnv::new();
    let gh = FakeGitHub::default()
        .with_open_issues(vec![trial_issue()])
        .failing_issue_comments_on(568);

    let ran = pause(&env, &gh, "a break");

    ran.result.unwrap();
    assert!(loop_pause::read_medium(&env.base_dir()).paused());
    assert!(ran.warn.contains("could not be posted"), "{}", ran.warn);
}

#[test]
fn an_empty_reason_is_refused_before_anything_is_written() {
    let env = TestEnv::new();
    let gh = FakeGitHub::default().with_open_issues(vec![trial_issue()]);

    let ran = pause(&env, &gh, "  ");

    let e = ran.result.unwrap_err();
    assert!(format!("{e:#}").contains("--pause-medium"), "{e:#}");
    assert_eq!(loop_pause::read_medium(&env.base_dir()), MediumState::None);
    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
}

#[test]
fn a_second_pause_replaces_the_first_and_says_so() {
    let env = TestEnv::new();
    let gh = FakeGitHub::default().with_open_issues(vec![trial_issue()]);
    pause(&env, &gh, "first").result.unwrap();

    let ran = pause(&env, &gh, "second");

    ran.result.unwrap();
    let MediumState::Set(set) = loop_pause::read_medium(&env.base_dir()) else {
        panic!("not set");
    };
    assert_eq!(set.why, "second");
    assert!(
        ran.out.contains("replaces the medium pause: first"),
        "{}",
        ran.out
    );
    assert_eq!(gh.issue_comments_now(568).len(), 2);
    let posted = gh
        .calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::IssueComment(..)))
        .count();
    assert_eq!(posted, 2);
}
