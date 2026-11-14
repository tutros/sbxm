//! Issue 548 (answer 4): the notice of a medium-risk PR's veto window, once per PR head when the
//! window starts: a PR comment saying when it will be armed, and a toast whose click opens the
//! PR, both shown once through `notified.json`'s `window:<pr>:<head>` key. Nothing calls it
//! before #550.

use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::notify::{FakeNotifier, Shown};
use sbxm::task::window::{self, Notice, Pr, Window, Zone};

const REPO: &str = "o/r";
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const PUSHED: &str = "1111111111111111111111111111111111111111";
const URL: &str = "https://github.com/o/r/pull/12";
/// 2026-11-10 21:40 UTC: 1:40 pm in PST.
const UNTIL: u64 = 1_794_346_800;

fn pst() -> Zone {
    Zone {
        offset_secs: -8 * 3_600,
        abbreviation: "PST".into(),
    }
}

fn pr(head: &str) -> Pr<'_> {
    Pr {
        number: 12,
        url: URL,
        head,
    }
}

struct Run {
    out: String,
    warn: String,
}

fn announce(
    gh: &FakeGitHub,
    notifier: &FakeNotifier,
    shown: &mut Shown,
    pr: &Pr,
    window: &Window,
) -> Run {
    let zone = pst();
    let notice = Notice {
        github: gh,
        repo: REPO,
        notifier,
        zone: &zone,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    notice
        .announce(shown, pr, window, &mut out, &mut warn)
        .unwrap();
    Run {
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

fn comments(gh: &FakeGitHub) -> Vec<String> {
    gh.calls()
        .into_iter()
        .filter_map(|c| match c {
            GhCall::PrComment(_, 12, body) => Some(body),
            _ => None,
        })
        .collect()
}

fn waiting() -> Window {
    Window::Waiting { until: UNTIL }
}

// --- The times ---

#[test]
fn a_time_is_given_in_the_local_zone_with_its_abbreviation_then_in_utc() {
    assert_eq!(window::time_text(UNTIL, &pst()), "1:40 pm PST (21:40 UTC)");
}

#[test]
fn midnight_and_noon_are_twelve() {
    let utc = Zone {
        offset_secs: 0,
        abbreviation: "UTC".into(),
    };
    // 2026-11-11 00:05 UTC, then 12:05.
    let midnight = 1_794_355_500;
    assert_eq!(
        window::time_text(midnight, &utc),
        "12:05 am UTC (00:05 UTC)"
    );
    assert_eq!(
        window::time_text(midnight + 12 * 3_600, &utc),
        "12:05 pm UTC (12:05 UTC)"
    );
}

#[test]
fn a_zone_ahead_of_utc_crosses_midnight() {
    let jst = Zone {
        offset_secs: 9 * 3_600,
        abbreviation: "JST".into(),
    };
    assert_eq!(window::time_text(UNTIL, &jst), "6:40 am JST (21:40 UTC)");
}

// --- The notice ---

#[test]
fn first_sight_posts_the_comment_and_the_toast() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::default());

    let run = announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());

    assert_eq!(
        comments(&gh),
        [
            "Medium risk: auto-merge will be armed at 1:40 pm PST (21:40 UTC) unless labelled \
          `hold`. Label `go` to arm now."
        ]
    );
    let toasts = notifier.shown();
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert_eq!(toasts[0].title, "PR #12 arms at 1:40 pm PST (medium)");
    assert_eq!(toasts[0].click, URL);
    assert_eq!(toasts[0].key, format!("window:12:{HEAD}"));
    assert!(toasts[0].body.contains("hold"), "{}", toasts[0].body);
    assert!(shown.contains(&format!("window:12:{HEAD}")));
    assert!(run.warn.is_empty(), "{}", run.warn);
    assert!(run.out.is_empty(), "{}", run.out);
}

#[test]
fn the_same_head_again_posts_nothing_even_after_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::default());
    announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());

    announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());
    let mut restarted = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    announce(&gh, &notifier, &mut restarted, &pr(HEAD), &waiting());

    assert_eq!(comments(&gh).len(), 1);
    assert_eq!(notifier.shown().len(), 1);
}

#[test]
fn a_new_head_posts_again() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::default());
    announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());

    announce(&gh, &notifier, &mut shown, &pr(PUSHED), &waiting());

    assert_eq!(comments(&gh).len(), 2);
    let toasts = notifier.shown();
    assert_eq!(toasts.len(), 2);
    assert_eq!(toasts[1].key, format!("window:12:{PUSHED}"));
}

#[test]
fn a_pr_already_held_at_window_start_gets_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::default());

    announce(&gh, &notifier, &mut shown, &pr(HEAD), &Window::Held);

    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    assert!(notifier.shown().is_empty());
    assert!(!shown.contains(&format!("window:12:{HEAD}")));
}

#[test]
fn only_a_waiting_pr_gets_a_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::default());

    for window in [
        Window::Go,
        Window::Over,
        Window::NotEligible("no review".into()),
        Window::Refused(vec!["size L".into()]),
    ] {
        announce(&gh, &notifier, &mut shown, &pr(HEAD), &window);
    }

    assert!(gh.calls().is_empty(), "{:?}", gh.calls());
    assert!(notifier.shown().is_empty());
}

#[test]
fn a_comment_that_fails_to_post_is_a_warning_and_the_toast_still_shows() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let gh = FakeGitHub::default().failing_comments("HTTP 502");
    let notifier = FakeNotifier::default();

    let run = announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());

    assert!(run.warn.starts_with("warning: "), "{}", run.warn);
    assert!(run.warn.contains("PR #12"), "{}", run.warn);
    assert!(run.warn.contains("HTTP 502"), "{}", run.warn);
    assert_eq!(run.warn.lines().count(), 1, "{}", run.warn);
    assert_eq!(notifier.shown().len(), 1);
    // Once per head all the same: the window runs without its comment.
    announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());
    assert_eq!(comments(&gh).len(), 1);
}

#[test]
fn a_toast_that_fails_to_show_is_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let mut shown = Shown::load(tmp.path(), &mut Vec::new()).unwrap();
    let (gh, notifier) = (FakeGitHub::default(), FakeNotifier::failing());

    let run = announce(&gh, &notifier, &mut shown, &pr(HEAD), &waiting());

    assert_eq!(comments(&gh).len(), 1);
    assert!(run.warn.contains("could not be shown"), "{}", run.warn);
    assert!(shown.contains(&format!("window:12:{HEAD}")));
}
