//! Issue 550 (decision 316(4)): the medium-risk auto-merge trial's record on GitHub, the open
//! issue labelled `medium-trial`: its merge comments counted (10 stops medium arming), a PR's
//! `hold` labellings counted from its label history, and the comments the loop posts. Also the
//! medium pause (`task loop --pause-medium`), which `--clear-pause` lifts with the red-main one.

use sbxm::github::LabelEvent;
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{GitHubBackend, Issue};
use sbxm::task::loop_pause::{self, MediumPause, Pause};
use sbxm::task::trial::{self, Armed, How, Merge, Trial};

const REPO: &str = "o/r";

fn merge_comment(pr: u32) -> String {
    trial::merge_comment(&Merge {
        pr,
        armed: Armed {
            how: How::WindowEnd,
            late_min: 3,
            review_at: 0,
        },
        holds: 0,
        merged_at: 600,
    })
}

fn bodies(merges: usize, breaks: usize) -> Vec<String> {
    let mut all: Vec<String> = (0..merges).map(|i| merge_comment(200 + i as u32)).collect();
    all.extend((0..breaks).map(|i| trial::break_comment(&format!("break {i}"))));
    all.push("a note by hand".to_owned());
    all
}

// --- The count ---

#[test]
fn nine_merge_comments_leave_room() {
    assert_eq!(trial::merges(&bodies(9, 0)), 9);
    assert!(!trial::full(9));
}

#[test]
fn ten_merge_comments_end_the_trial() {
    assert_eq!(trial::merges(&bodies(10, 0)), 10);
    assert!(trial::full(10));
}

#[test]
fn eleven_merge_comments_posted_by_hand_still_end_it() {
    let mut all = bodies(10, 0);
    all.push(merge_comment(999));
    assert_eq!(trial::merges(&all), 11);
    assert!(trial::full(11));
}

#[test]
fn break_comments_dont_count_toward_the_ten() {
    assert_eq!(trial::merges(&bodies(9, 3)), 9);
    assert!(!trial::full(trial::merges(&bodies(9, 3))));
}

#[test]
fn a_merge_mark_inside_a_comment_doesnt_count() {
    let quoted = format!("> {}", merge_comment(5));
    assert_eq!(trial::merges(&[quoted]), 0);
}

// --- Holds ---

fn event(label: &str, added: bool) -> LabelEvent {
    LabelEvent {
        label: label.to_owned(),
        added,
        at: 100,
    }
}

#[test]
fn no_hold_counts_zero() {
    assert_eq!(trial::holds(&[event("go", true), event("size:M", true)]), 0);
}

#[test]
fn one_hold_counts_one() {
    assert_eq!(
        trial::holds(&[event("hold", true), event("hold", false)]),
        1
    );
}

#[test]
fn a_hold_added_removed_and_added_again_counts_two() {
    assert_eq!(
        trial::holds(&[
            event("hold", true),
            event("hold", false),
            event("Hold", true),
        ]),
        2
    );
}

#[test]
fn go_is_read_at_its_latest_labelling() {
    let mut events = vec![event("go", true), event("go", false), event("GO", true)];
    events[2].at = 900;
    assert_eq!(trial::go_at(&events), Some(900));
    assert_eq!(trial::go_at(&[event("hold", true)]), None);
}

// --- The comments ---

#[test]
fn a_merge_comment_names_the_pr_how_holds_and_minutes() {
    let text = trial::merge_comment(&Merge {
        pr: 120,
        armed: Armed {
            how: How::Go,
            late_min: 7,
            review_at: 1_000,
        },
        holds: 2,
        merged_at: 1_000 + 95 * 60 + 30,
    });
    assert_eq!(
        text,
        "Medium trial: merge of PR #120\n\n- armed by: `go`, 7 min after it was labelled\n- \
         holds: 2\n- clean review to merge: 95 min\n"
    );
    assert_eq!(trial::merges(&[text]), 1);
}

#[test]
fn a_window_end_merge_comment_says_so() {
    let text = merge_comment(4);
    assert!(
        text.contains("- armed by: the window's end, 3 min after it ended"),
        "{text}"
    );
}

#[test]
fn a_break_comment_names_why() {
    assert_eq!(
        trial::break_comment("PR #9 needed a fix"),
        "Medium trial: break\n\nPR #9 needed a fix\n"
    );
}

// --- Reading the trial from GitHub ---

fn trial_issue(number: u32) -> Issue {
    Issue {
        number,
        open: true,
        title: "Medium trial".into(),
        labels: vec![trial::LABEL.into()],
        body: String::new(),
    }
}

#[test]
fn no_open_trial_issue_reads_as_none() {
    let gh = FakeGitHub::default();
    assert_eq!(trial::read(&gh, REPO), Ok(Trial::NoIssue));
}

#[test]
fn the_trial_issue_and_its_merges_are_read() {
    let all = bodies(4, 1);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let gh = FakeGitHub::default()
        .with_open_issues(vec![trial_issue(568)])
        .with_issue_comments(568, &refs);
    assert_eq!(
        trial::read(&gh, REPO),
        Ok(Trial::Open {
            issue: 568,
            merges: 4
        })
    );
}

#[test]
fn two_open_trial_issues_read_the_oldest() {
    let gh = FakeGitHub::default()
        .with_open_issues(vec![trial_issue(700), trial_issue(568)])
        .with_issue_comments(700, &[&merge_comment(1)]);
    assert_eq!(
        trial::read(&gh, REPO),
        Ok(Trial::Open {
            issue: 568,
            merges: 0
        })
    );
}

#[test]
fn comments_that_cant_be_read_are_an_error() {
    let gh = FakeGitHub::default()
        .with_open_issues(vec![trial_issue(568)])
        .failing_issue_comment_reads("HTTP 502");
    let e = trial::read(&gh, REPO).unwrap_err();
    assert!(e.contains("HTTP 502"), "{e}");
    assert!(e.contains("#568"), "{e}");
}

#[test]
fn issues_that_cant_be_listed_are_an_error() {
    let gh = FakeGitHub::default().failing("HTTP 502");
    assert!(trial::read(&gh, REPO).is_err());
}

#[test]
fn posting_a_comment_goes_to_the_trial_issue() {
    let gh = FakeGitHub::default().with_open_issues(vec![trial_issue(568)]);
    let posted = trial::post(&gh, REPO, "hello").unwrap();
    assert_eq!(posted, Some(568));
    assert!(
        gh.calls()
            .contains(&GhCall::IssueComment(REPO.into(), 568, "hello".into()))
    );
    assert_eq!(gh.issue_comments_now(568), ["hello"]);
}

#[test]
fn posting_with_no_trial_issue_posts_nothing() {
    let gh = FakeGitHub::default();
    assert_eq!(trial::post(&gh, REPO, "hello").unwrap(), None);
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::IssueComment(..)))
    );
    let _ = gh.issues_open(REPO);
}

// --- The medium pause ---

fn red() -> Pause {
    Pause {
        commit: "abc".into(),
        run_id: 1,
        run_url: "u".into(),
        workflow: "ci".into(),
        merged_by: "me".into(),
        at: "2026-11-10 10:00".into(),
    }
}

#[test]
fn a_medium_pause_is_written_and_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        loop_pause::read_medium(tmp.path()),
        loop_pause::MediumState::None
    );
    let pause = MediumPause {
        why: "PR #9 needed a fix".into(),
        at: "2026-11-10 10:00".into(),
    };
    loop_pause::write_medium(tmp.path(), &pause).unwrap();
    assert_eq!(
        loop_pause::read_medium(tmp.path()),
        loop_pause::MediumState::Set(pause)
    );
    // The red-main pause is untouched by it.
    assert_eq!(loop_pause::read(tmp.path()), loop_pause::State::None);
}

#[test]
fn a_damaged_medium_pause_counts_as_paused() {
    let tmp = tempfile::tempdir().unwrap();
    let path = loop_pause::medium_path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{").unwrap();
    assert!(loop_pause::read_medium(tmp.path()).paused());
}

#[test]
fn clear_pause_lifts_both_pauses_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    loop_pause::write(tmp.path(), &red()).unwrap();
    loop_pause::write_medium(
        tmp.path(),
        &MediumPause {
            why: "PR #9 needed a fix".into(),
            at: "2026-11-10 10:00".into(),
        },
    )
    .unwrap();
    let mut out = Vec::new();
    loop_pause::clear(tmp.path(), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("cleared the pause: main's commit abc"),
        "{out}"
    );
    assert!(
        out.contains("cleared the medium pause: PR #9 needed a fix"),
        "{out}"
    );
    assert_eq!(loop_pause::read(tmp.path()), loop_pause::State::None);
    assert_eq!(
        loop_pause::read_medium(tmp.path()),
        loop_pause::MediumState::None
    );
}

#[test]
fn clear_pause_lifts_a_medium_pause_alone() {
    let tmp = tempfile::tempdir().unwrap();
    loop_pause::write_medium(
        tmp.path(),
        &MediumPause {
            why: "w".into(),
            at: "t".into(),
        },
    )
    .unwrap();
    let mut out = Vec::new();
    loop_pause::clear(tmp.path(), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("there was no pause to clear"), "{out}");
    assert!(out.contains("cleared the medium pause: w"), "{out}");
    assert!(!loop_pause::read_medium(tmp.path()).paused());
}

#[test]
fn clear_pause_with_neither_says_so_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    loop_pause::clear(tmp.path(), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.matches("no pause to clear").count(), 1, "{out}");
}
