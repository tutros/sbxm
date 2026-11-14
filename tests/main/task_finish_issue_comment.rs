//! Issue 430: `task finish` posts one comment on each issue the PR closes, summing up the in-task
//! review and fix rounds from `task.json` and the saved `review-<n>.md` files; a later finish
//! edits the comment `task.json` recorded. Finding titles are inert, and a failed post is a
//! warning, never a failed finish.

use crate::common;

use std::fs;
use std::path::{Path, PathBuf};

use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, ready_spec_task, worked_task,
};
use sbxm::commands::task_finish::{self, Target};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::finish::{finish, finish_spec_pr};
use sbxm::task::pipeline::Prepared;
use sbxm::task::record::{
    self, FixRun, IssueComment, Kind, NewTask, Process, Record, RunInfo, Stage, Status, Stopped,
};
use sbxm::task::summary::{TITLE_CAP, inert, issue_comment};

// --- The comment's text --------------------------------------------------------------------------

fn record_of(kind: Kind) -> Record {
    Record::new(
        &NewTask {
            kind,
            number: 41,
            repo: "o/r",
            title: "Fix 41",
            base: "main",
            branch: "issue-41",
            config_hash: "h",
            id: None,
        },
        0,
        Process::new(1, 0),
    )
}

fn review(must_fix: &[(&str, &str, Option<&str>)]) -> String {
    let mut text = format!(
        "Reviewer: codex (default)\n\nMust-fix findings: {}\n\n## Must fix\n\n",
        must_fix.len()
    );
    for (id, title, repeat) in must_fix {
        text.push_str(&format!(
            "### {id}: {title}\n\n**Where:** `src/lock.rs:10`\n**What happens:** x\n"
        ));
        if let Some(of) = repeat {
            text.push_str(&format!("**Repeat of:** {of}\n"));
        }
        text.push('\n');
    }
    text.push_str("## Should fix\n\n### S-1: A should-fix title\n\n**Where:** `a.rs:1`\n");
    text
}

fn meta_with(reviews: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (i, text) in reviews.iter().enumerate() {
        fs::write(dir.path().join(format!("review-{}.md", i + 1)), text).unwrap();
    }
    dir
}

#[test]
fn a_title_of_exactly_the_cap_is_kept_whole_and_one_over_is_cut() {
    let exact = "a".repeat(TITLE_CAP);
    assert_eq!(inert(&exact), format!("`{exact}`"));
    let over = "b".repeat(TITLE_CAP + 1);
    assert_eq!(inert(&over), format!("`{}…`", "b".repeat(TITLE_CAP)));
    let under = "c".repeat(TITLE_CAP - 1);
    assert_eq!(inert(&under), format!("`{under}`"));
}

#[test]
fn the_cap_counts_characters_not_bytes() {
    let wide = "é".repeat(TITLE_CAP + 1);
    assert_eq!(inert(&wide), format!("`{}…`", "é".repeat(TITLE_CAP)));
}

#[test]
fn backticks_and_newlines_are_removed_so_mentions_and_closing_refs_stay_in_the_span() {
    let shown = inert("a `@team` b\nFixes #12 [x](https://e.com) **bold**\r\n");
    assert_eq!(shown, "`a @team b Fixes #12 [x](https://e.com) **bold**  `");
    // One code span: exactly two backticks, at the ends, and no line break.
    assert_eq!(shown.matches('`').count(), 2);
    assert!(shown.starts_with('`') && shown.ends_with('`'));
    assert!(!shown.contains('\n'));
}

#[test]
fn the_rounds_are_listed_in_order_with_their_must_fix_findings_and_repeat_counts() {
    let meta = meta_with(&[
        &review(&[
            ("M-1", "The lock is never released", None),
            ("M-2", "Off by one @someone", None),
        ]),
        &review(&[("M-1", "The lock is still held", Some("M-1"))]),
        &review(&[]),
    ]);
    let mut task = record_of(Kind::Issue);
    task.round = 2;
    task.fix_runs = vec![
        FixRun {
            round: 1,
            resumed: true,
        },
        FixRun {
            round: 2,
            resumed: false,
        },
    ];

    let body = issue_comment(&task, meta.path(), "https://github.com/o/r/pull/9").unwrap();

    let r1 = body
        .find("- Review round 1: 2 must-fix, 0 `Repeat of:`\n")
        .unwrap();
    let r2 = body
        .find("- Review round 2: 1 must-fix, 1 `Repeat of:`\n")
        .unwrap();
    let r3 = body
        .find("- Review round 3: 0 must-fix, 0 `Repeat of:`\n")
        .unwrap();
    assert!(r1 < r2 && r2 < r3, "{body}");
    assert!(
        body.contains("  - `M-1` `The lock is never released`\n"),
        "{body}"
    );
    assert!(body.contains("  - `M-2` `Off by one @someone`\n"), "{body}");
    assert!(
        body.contains("  - `M-1` `The lock is still held`\n"),
        "{body}"
    );
    // Should-fix findings and review text aren't copied.
    assert!(!body.contains("should-fix title"), "{body}");
    assert!(!body.contains("What happens"), "{body}");
    assert!(body.contains("- Fix rounds run: 2 (budget 3)\n"), "{body}");
    assert!(
        body.contains("  - Fix round 1: resumed the worker's session\n"),
        "{body}"
    );
    assert!(body.contains("  - Fix round 2: ran fresh"), "{body}");
    assert!(!body.contains("Stopped"), "{body}");
    assert!(
        body.contains("- PR: https://github.com/o/r/pull/9\n"),
        "{body}"
    );
}

#[test]
fn a_round_with_no_findings_and_a_task_with_no_fix_round() {
    let meta = meta_with(&[&review(&[])]);
    let task = record_of(Kind::Issue);

    let body = issue_comment(&task, meta.path(), "https://github.com/o/r/pull/9").unwrap();

    assert!(
        body.contains("- Review round 1: 0 must-fix, 0 `Repeat of:`\n"),
        "{body}"
    );
    assert!(!body.contains("  - `"), "{body}");
    assert!(body.contains("- Fix rounds run: 0 (budget 3)\n"), "{body}");
    assert!(!body.contains("Fix round 1"), "{body}");
}

#[test]
fn a_task_that_stopped_shows_its_stop_reason() {
    let meta = meta_with(&[
        &review(&[("M-1", "Lock", None)]),
        &review(&[("M-1", "Lock again", Some("M-1"))]),
    ]);
    let mut task = record_of(Kind::Issue);
    task.round = 1;
    task.stopped = Some(Stopped::RepeatFinding);

    let body = issue_comment(&task, meta.path(), "u").unwrap();

    assert!(body.contains("- Stopped: repeat-finding\n"), "{body}");
    // An older record has no fix run recorded for the round.
    assert!(body.contains("  - Fix round 1: not recorded\n"), "{body}");
}

#[test]
fn the_worker_and_latest_review_times_and_costs_are_shown() {
    let meta = meta_with(&[&review(&[])]);
    let mut task = record_of(Kind::Issue);
    let agent = |run: Option<RunInfo>| record::Agent {
        harness: "claude".into(),
        model: None,
        sandbox: "s".into(),
        workspace: "w".into(),
        profile: None,
        cpus: None,
        memory: None,
        template: None,
        run,
        session: None,
    };
    task.worker = Some(agent(Some(RunInfo {
        status: "completed".into(),
        usage: serde_json::json!({ "cost_usd": 1.234 }),
        duration_s: 812,
    })));
    task.reviewer = Some(agent(Some(RunInfo {
        status: "completed".into(),
        usage: serde_json::json!({ "cost_usd": null }),
        duration_s: 300,
    })));

    let body = issue_comment(&task, meta.path(), "u").unwrap();

    assert!(body.contains("- Worker: 812 s, $1.23\n"), "{body}");
    assert!(
        body.contains("- Latest review: 300 s, cost not reported\n"),
        "{body}"
    );
}

#[test]
fn an_older_record_without_the_new_fields_still_loads() {
    let task = record_of(Kind::Issue);
    let mut json: serde_json::Value = serde_json::to_value(&task).unwrap();
    let object = json.as_object_mut().unwrap();
    assert!(!object.contains_key("issue_comments"));
    assert!(!object.contains_key("fix_runs"));
    let back: Record = serde_json::from_value(json).unwrap();
    assert!(back.issue_comments.is_empty() && back.fix_runs.is_empty());
}

// --- An issue task's finish ----------------------------------------------------------------------

const REVIEW_CLEAN: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done by hand: `ready`, with `review-1.md` saved.
fn ready(f: &Fixture) -> Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: vec!["a.txt".to_owned()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let mut prepared = worked_task(f, &b);
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW_CLEAN).unwrap();
    fs::write(prepared.meta.join("review-1.md"), REVIEW_CLEAN).unwrap();
    prepared
}

fn reread(meta: &Path) -> Record {
    record::read(&meta.join("task.json")).unwrap()
}

fn comments(github: &FakeGitHub) -> Vec<GhCall> {
    github
        .calls()
        .into_iter()
        .filter(|c| matches!(c, GhCall::IssueComment(..) | GhCall::IssueCommentEdit(..)))
        .collect()
}

#[test]
fn an_issue_task_posts_one_comment_on_its_issue_and_records_its_id() {
    let f = fixture();
    let prepared = ready(&f);
    let github = FakeGitHub::default().with_next_comment_id(900);

    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();

    let calls = comments(&github);
    let [GhCall::IssueComment(repo, 41, body)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert_eq!(repo, "o/r");
    assert!(body.contains("task `issue-41`"), "{body}");
    assert!(
        body.contains("- Review round 1: 0 must-fix, 0 `Repeat of:`\n"),
        "{body}"
    );
    assert!(body.contains(&format!("- PR: {}\n", done.url)), "{body}");
    // The comment comes after the PR is open.
    let all = github.calls();
    let created = all
        .iter()
        .position(|c| matches!(c, GhCall::PrCreate(..)))
        .unwrap();
    let commented = all
        .iter()
        .position(|c| matches!(c, GhCall::IssueComment(..)))
        .unwrap();
    assert!(created < commented, "{all:?}");
    assert_eq!(
        reread(&prepared.meta).issue_comments,
        [IssueComment { issue: 41, id: 900 }]
    );
    assert!(done.warnings.is_empty(), "{:?}", done.warnings);
}

#[test]
fn a_recorded_comment_is_edited_rather_than_posted_again() {
    let f = fixture();
    let prepared = ready(&f);
    let mut task = reread(&prepared.meta);
    task.issue_comments = vec![IssueComment { issue: 41, id: 77 }];
    record::write(&prepared.meta, &task).unwrap();
    let github = FakeGitHub::default();

    finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();

    let calls = comments(&github);
    let [GhCall::IssueCommentEdit(repo, 77, body)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert_eq!(repo, "o/r");
    assert!(body.contains("task `issue-41`"), "{body}");
    assert_eq!(
        reread(&prepared.meta).issue_comments,
        [IssueComment { issue: 41, id: 77 }]
    );
}

#[test]
fn a_failing_edit_is_a_warning_and_the_finish_succeeds() {
    let f = fixture();
    let prepared = ready(&f);
    let mut task = reread(&prepared.meta);
    task.issue_comments = vec![IssueComment { issue: 41, id: 77 }];
    record::write(&prepared.meta, &task).unwrap();
    let github = FakeGitHub::default().failing_issue_comment_edits();

    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();

    assert!(
        done.warnings.iter().any(
            |w| w.contains("issue #41 (comment 77) couldn't be updated: ") && !w.contains("  ")
        ),
        "{:?}",
        done.warnings
    );
    assert_eq!(reread(&prepared.meta).stage, Stage::Finished);
}

#[test]
fn an_issue_task_whose_post_fails_warns_and_still_opens_the_pr() {
    let f = fixture();
    let prepared = ready(&f);
    let github = FakeGitHub::default().failing_issue_comments_on(41);

    let mut out = Vec::new();
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: Target::Issue(41),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        &github,
        &Probe,
        &mut out,
    )
    .unwrap();

    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("warning: the summary of the review rounds couldn't be posted on issue #41"),
        "{out}"
    );
    assert!(
        github
            .calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrCreate(..))),
        "{:?}",
        github.calls()
    );
    let task = reread(&prepared.meta);
    assert_eq!(task.stage, Stage::Finished);
    assert!(task.pr.is_some());
    assert!(task.issue_comments.is_empty());
}

// --- Secret-looking text is checked before it is cut (review M-2) ------------------------------

/// A synthetic token assignment whose closing quote is character `TITLE_CAP + 1` of the title, so
/// the cut drops the quote and keeps the whole value. Built from parts so no file holds it whole.
fn title_hiding_a_secret() -> (String, &'static str) {
    let value = "SYNTHETIC_SECRET_VALUE";
    let suffix = [" token ", "= '", value, "'"].concat();
    let title = format!(
        "{}{suffix}",
        "x".repeat(TITLE_CAP + 1 - suffix.chars().count())
    );
    assert_eq!(title.chars().count(), TITLE_CAP + 1);
    assert!(sbxm::task::findings::secret_kind(&title).is_some());
    // Cut as the comment cuts it, it no longer looks like a secret: only the uncut check sees it.
    assert!(sbxm::task::findings::secret_kind(&inert(&title)).is_none());
    (title, value)
}

#[test]
fn a_title_whose_secret_survives_the_cut_is_refused_without_its_value() {
    let (title, value) = title_hiding_a_secret();
    let meta = meta_with(&[&review(&[("M-1", &title, None)]), &review(&[])]);

    let why = issue_comment(&record_of(Kind::Issue), meta.path(), "u").unwrap_err();

    assert!(why.contains("review-1.md"), "{why}");
    assert!(!why.contains(value), "{why}");
}

#[test]
fn a_finding_id_that_looks_like_a_secret_is_refused() {
    let id = ["ghp_", &"a".repeat(36)].concat();
    let meta = meta_with(&[&review(&[(&id, "A title", None)])]);

    let why = issue_comment(&record_of(Kind::Issue), meta.path(), "u").unwrap_err();

    assert!(!why.contains(&id), "{why}");
}

#[test]
fn a_secret_in_a_finding_left_out_for_length_is_still_refused() {
    let (title, _) = title_hiding_a_secret();
    let mut findings: Vec<(String, String)> = (1..=800)
        .map(|n| (format!("M-{n}"), "y".repeat(90)))
        .collect();
    findings.push(("M-801".into(), title));
    let listed: Vec<(&str, &str, Option<&str>)> = findings
        .iter()
        .map(|(id, title)| (id.as_str(), title.as_str(), None))
        .collect();
    let task = record_of(Kind::Issue);
    // Without the secret, the list is too long to show whole: the last findings are counted.
    let clean = meta_with(&[&review(&listed[..800])]);
    let body = issue_comment(&task, clean.path(), "u").unwrap();
    assert!(
        body.contains("more must-fix finding(s), not listed here"),
        "{body}"
    );

    let meta = meta_with(&[&review(&listed)]);
    assert!(issue_comment(&task, meta.path(), "u").is_err());
}

#[test]
fn a_finish_whose_earlier_review_hides_a_secret_title_posts_nothing_and_warns() {
    let (title, value) = title_hiding_a_secret();
    let f = fixture();
    let prepared = ready(&f);
    fs::write(
        prepared.meta.join("review-1.md"),
        review(&[("M-1", &title, None)]),
    )
    .unwrap();
    fs::write(prepared.meta.join("review-2.md"), REVIEW_CLEAN).unwrap();
    let github = FakeGitHub::default();

    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();

    assert!(comments(&github).is_empty(), "{:?}", github.calls());
    assert!(
        github
            .calls()
            .iter()
            .all(|c| !format!("{c:?}").contains(value)),
        "{:?}",
        github.calls()
    );
    let [warning] = done.warnings.as_slice() else {
        panic!("{:?}", done.warnings)
    };
    assert!(
        warning.contains("no summary of the review rounds was posted"),
        "{warning}"
    );
    assert!(!warning.contains(value), "{warning}");
    assert_eq!(reread(&prepared.meta).stage, Stage::Finished);
}

// --- A spec task's finish ------------------------------------------------------------------------

fn meta(f: &Fixture, id: &str) -> PathBuf {
    record::task_dir(&f.env.base_dir(), id)
}

/// A ready spec task whose spec, as its start saved it, is `text`.
fn ready_spec(f: &Fixture, text: &str) -> String {
    let (_, id) = ready_spec_task(f);
    fs::write(meta(f, &id).join("source.md"), text).unwrap();
    id
}

fn finish_spec(f: &Fixture, id: &str, github: &FakeGitHub) -> sbxm::task::finish::Finished {
    finish_spec_pr(&f.env.base_dir(), id, "--spec idea.md", github, &Probe).unwrap()
}

#[test]
fn a_spec_task_with_one_fixes_issue_posts_on_it() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #435\n");
    let github = FakeGitHub::default().with_next_comment_id(10);

    finish_spec(&f, &id, &github);

    let calls = comments(&github);
    let [GhCall::IssueComment(_, 435, body)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert!(body.contains(&format!("task `{id}`")), "{body}");
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [IssueComment { issue: 435, id: 10 }]
    );
}

#[test]
fn a_spec_task_with_two_fixes_issues_posts_on_each_in_the_lines_order() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #436, #435\n");
    let github = FakeGitHub::default().with_next_comment_id(10);

    finish_spec(&f, &id, &github);

    let calls = comments(&github);
    let [
        GhCall::IssueComment(_, 436, first),
        GhCall::IssueComment(_, 435, second),
    ] = calls.as_slice()
    else {
        panic!("{calls:?}")
    };
    assert_eq!(first, second);
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [
            IssueComment { issue: 436, id: 10 },
            IssueComment { issue: 435, id: 11 }
        ]
    );
}

#[test]
fn a_spec_task_without_a_fixes_line_posts_nothing() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nSee #426.\n");
    let github = FakeGitHub::default();

    finish_spec(&f, &id, &github);

    assert!(comments(&github).is_empty(), "{:?}", github.calls());
    assert!(reread(&meta(&f, &id)).issue_comments.is_empty());
}

#[test]
fn a_failing_post_to_the_second_issue_is_a_warning_and_the_first_comment_stays() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #436, #435\n");
    let github = FakeGitHub::default()
        .with_next_comment_id(10)
        .failing_issue_comments_on(435);

    let done = finish_spec(&f, &id, &github);

    assert_eq!(comments(&github).len(), 2);
    assert!(
        done.warnings.iter().any(|w| w.contains("issue #435")),
        "{:?}",
        done.warnings
    );
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [IssueComment { issue: 436, id: 10 }]
    );
}

#[test]
fn a_failing_post_to_the_first_issue_doesnt_stop_the_second() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #436, #435\n");
    let github = FakeGitHub::default()
        .with_next_comment_id(10)
        .failing_issue_comments_on(436);

    let done = finish_spec(&f, &id, &github);

    assert_eq!(comments(&github).len(), 2);
    assert!(
        done.warnings.iter().any(|w| w.contains("issue #436")),
        "{:?}",
        done.warnings
    );
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [IssueComment { issue: 435, id: 10 }]
    );
}

#[test]
fn a_spec_task_under_the_local_sink_posts_nothing() {
    let f = fixture();
    let (spec, id) = ready_spec_task(&f);
    fs::write(meta(&f, &id).join("source.md"), "# Batch\n\nFixes: #435\n").unwrap();
    let github = FakeGitHub::default();

    let mut out = Vec::new();
    task_finish::run(
        &f.env.config_dir(),
        &task_finish::Options {
            target: Target::Spec(spec),
            repo_root: f.env.tmp.path().join("target-repo"),
            push_unresolved: false,
        },
        &sbxm::backend::FakeBackend::default(),
        &github,
        &Probe,
        &mut out,
    )
    .unwrap();

    assert!(comments(&github).is_empty(), "{:?}", github.calls());
}

// --- A task that continues an open PR ------------------------------------------------------------

/// Issue 41 continues PR 7 (branch `feature-x` on the remote, at the PR's head): the worker
/// commits on top, and the gates and review are done by hand, so the task is `ready`.
fn ready_continuing(f: &Fixture) -> Prepared {
    use common::task_fixture::{add_pr_head, ctx, source};
    use sbxm::github::{IssueText, PrInfo, PrState};
    let head = add_pr_head(f, 7);
    common::git(&f.origin, &["update-ref", "refs/heads/feature-x", &head]);
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "feature-x",
            Play {
                commits: vec!["b.txt".to_owned()],
                result_md: Some(b"done\n".to_vec()),
                bundle_bytes: None,
            },
        ));
    let github = FakeGitHub::default().with_pr(PrInfo {
        number: 7,
        head_ref: "feature-x".into(),
        head_oid: "0000000000000000000000000000000000000000".into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: "Add x".into(),
        body: "x".into(),
        closing_issues: vec![],
        base_moved: false,
    });
    let source = source(f);
    let ctx = ctx(f, &source, &b, &github);
    let issue = IssueText {
        number: 41,
        title: "Fix 41".into(),
        state: "OPEN".into(),
        text: "title:\tFix 41\nstate:\tOPEN\nlabels:\tmust-fix\n--\nPR: #7\n\ndo it\n".into(),
    };
    let mut prepared = sbxm::task::pipeline::prepare(&ctx, &issue).unwrap();
    sbxm::task::pipeline::run_worker(&ctx, &mut prepared).unwrap();
    let p = || Process::new(1, 0);
    let r = &mut prepared.record;
    r.begin_gating(1, p()).unwrap();
    r.finish(Status::Passed).unwrap();
    r.begin_review(2, p()).unwrap();
    r.finish(Status::Completed).unwrap();
    r.advance(Stage::Ready, 3, p()).unwrap();
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), REVIEW_CLEAN).unwrap();
    fs::write(prepared.meta.join("review-1.md"), REVIEW_CLEAN).unwrap();
    prepared.lock = None;
    prepared
}

#[test]
fn a_task_that_updates_an_open_pr_posts_on_its_issue_after_the_pr_comment() {
    let f = fixture();
    let prepared = ready_continuing(&f);
    let github = FakeGitHub::default().with_next_comment_id(5);

    let done = finish(&f.env.base_dir(), "issue-41", &github, &Probe).unwrap();

    let calls = github.calls();
    let [
        GhCall::PrComment(_, 7, _),
        GhCall::IssueComment(_, 41, body),
    ] = calls.as_slice()
    else {
        panic!("{calls:?}")
    };
    assert!(
        body.contains("- PR: https://github.com/o/r/pull/7\n"),
        "{body}"
    );
    assert_eq!(done.continued, Some(7));
    assert_eq!(
        reread(&prepared.meta).issue_comments,
        [IssueComment { issue: 41, id: 5 }]
    );
}

// --- A second finish of a finished task (review M-1 of PR 465) ----------------------------------

/// The GitHub calls of a second finish that are anything but the issue summary: there must be
/// none, since the retry never pushes, opens, adopts or reviews a PR.
fn not_summaries(github: &FakeGitHub) -> Vec<GhCall> {
    github
        .calls()
        .into_iter()
        .filter(|c| !matches!(c, GhCall::IssueComment(..) | GhCall::IssueCommentEdit(..)))
        .collect()
}

fn origin_tip(f: &Fixture, branch: &str) -> String {
    common::git(&f.origin, &["rev-parse", branch])
}

#[test]
fn finishing_an_issue_task_twice_edits_its_recorded_comment_and_publishes_nothing_else() {
    let f = fixture();
    let prepared = ready(&f);
    let first = FakeGitHub::default().with_next_comment_id(900);
    let opened = finish(&f.env.base_dir(), "issue-41", &first, &Probe).unwrap();
    let tip = origin_tip(&f, "issue-41");
    let second = FakeGitHub::default().with_next_comment_id(901);

    let done = finish(&f.env.base_dir(), "issue-41", &second, &Probe).unwrap();

    let calls = comments(&second);
    let [GhCall::IssueCommentEdit(repo, 900, body)] = calls.as_slice() else {
        panic!("{:?}", second.calls())
    };
    assert_eq!(repo, "o/r");
    assert!(body.contains(&format!("- PR: {}\n", opened.url)), "{body}");
    assert!(not_summaries(&second).is_empty(), "{:?}", second.calls());
    assert!(done.summaries_only);
    assert_eq!(done.url, opened.url);
    assert_eq!(done.review_posted, None);
    assert_eq!(origin_tip(&f, "issue-41"), tip);
    let task = reread(&prepared.meta);
    assert_eq!(task.issue_comments, [IssueComment { issue: 41, id: 900 }]);
    assert_eq!(task.pr.as_deref(), Some(opened.url.as_str()));
    assert_eq!(task.stage, Stage::Finished);
}

#[test]
fn finishing_a_spec_pr_task_twice_edits_each_recorded_comment_in_the_fixes_order() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #436, #435\n");
    finish_spec(&f, &id, &FakeGitHub::default().with_next_comment_id(10));
    let second = FakeGitHub::default().with_next_comment_id(20);

    let done = finish_spec(&f, &id, &second);

    let calls = comments(&second);
    let [
        GhCall::IssueCommentEdit(_, 10, _),
        GhCall::IssueCommentEdit(_, 11, _),
    ] = calls.as_slice()
    else {
        panic!("{:?}", second.calls())
    };
    assert!(not_summaries(&second).is_empty(), "{:?}", second.calls());
    assert!(done.summaries_only);
    assert_eq!(done.summarized, [436, 435]);
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [
            IssueComment { issue: 436, id: 10 },
            IssueComment { issue: 435, id: 11 }
        ]
    );
}

#[test]
fn finishing_a_task_that_continued_a_pr_twice_edits_its_recorded_comment() {
    let f = fixture();
    let prepared = ready_continuing(&f);
    finish(
        &f.env.base_dir(),
        "issue-41",
        &FakeGitHub::default().with_next_comment_id(5),
        &Probe,
    )
    .unwrap();
    let tip = origin_tip(&f, "feature-x");
    let second = FakeGitHub::default().with_next_comment_id(6);

    let done = finish(&f.env.base_dir(), "issue-41", &second, &Probe).unwrap();

    let calls = comments(&second);
    let [GhCall::IssueCommentEdit(_, 5, body)] = calls.as_slice() else {
        panic!("{:?}", second.calls())
    };
    assert!(
        body.contains("- PR: https://github.com/o/r/pull/7\n"),
        "{body}"
    );
    assert!(not_summaries(&second).is_empty(), "{:?}", second.calls());
    assert!(done.summaries_only);
    assert_eq!(done.continued, Some(7));
    assert_eq!(origin_tip(&f, "feature-x"), tip);
    assert_eq!(
        reread(&prepared.meta).issue_comments,
        [IssueComment { issue: 41, id: 5 }]
    );
}

#[test]
fn a_second_finish_posts_the_summary_a_failed_post_left_out_and_records_it() {
    let f = fixture();
    let prepared = ready(&f);
    finish(
        &f.env.base_dir(),
        "issue-41",
        &FakeGitHub::default().failing_issue_comments_on(41),
        &Probe,
    )
    .unwrap();
    assert!(reread(&prepared.meta).issue_comments.is_empty());
    let second = FakeGitHub::default().with_next_comment_id(902);

    let done = finish(&f.env.base_dir(), "issue-41", &second, &Probe).unwrap();

    let calls = comments(&second);
    let [GhCall::IssueComment(_, 41, _)] = calls.as_slice() else {
        panic!("{:?}", second.calls())
    };
    assert!(not_summaries(&second).is_empty(), "{:?}", second.calls());
    assert!(done.warnings.is_empty(), "{:?}", done.warnings);
    assert_eq!(
        reread(&prepared.meta).issue_comments,
        [IssueComment { issue: 41, id: 902 }]
    );
}

#[test]
fn a_second_spec_finish_posts_the_missing_summary_and_edits_the_one_that_was_posted() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #436, #435\n");
    finish_spec(
        &f,
        &id,
        &FakeGitHub::default()
            .with_next_comment_id(10)
            .failing_issue_comments_on(435),
    );
    let second = FakeGitHub::default().with_next_comment_id(30);

    finish_spec(&f, &id, &second);

    let calls = comments(&second);
    let [
        GhCall::IssueCommentEdit(_, 10, _),
        GhCall::IssueComment(_, 435, _),
    ] = calls.as_slice()
    else {
        panic!("{:?}", second.calls())
    };
    assert!(not_summaries(&second).is_empty(), "{:?}", second.calls());
    assert_eq!(
        reread(&meta(&f, &id)).issue_comments,
        [
            IssueComment { issue: 436, id: 10 },
            IssueComment { issue: 435, id: 30 }
        ]
    );
}

#[test]
fn a_second_finish_still_warns_when_the_summary_still_cant_be_posted() {
    let f = fixture();
    ready(&f);
    let down = FakeGitHub::default().failing_issue_comments_on(41);
    finish(&f.env.base_dir(), "issue-41", &down, &Probe).unwrap();

    let done = finish(&f.env.base_dir(), "issue-41", &down, &Probe).unwrap();

    assert!(
        done.warnings
            .iter()
            .any(|w| w.contains("couldn't be posted on issue #41")),
        "{:?}",
        done.warnings
    );
}

#[test]
fn a_second_spec_finish_whose_saved_spec_is_gone_is_refused_before_any_call() {
    let f = fixture();
    let id = ready_spec(&f, "# Batch\n\nFixes: #435\n");
    finish_spec(&f, &id, &FakeGitHub::default().with_next_comment_id(10));
    fs::remove_file(meta(&f, &id).join("source.md")).unwrap();
    let second = FakeGitHub::default();

    let err =
        finish_spec_pr(&f.env.base_dir(), &id, "--spec idea.md", &second, &Probe).unwrap_err();

    assert!(format!("{err:#}").contains("source.md"), "{err:#}");
    assert!(second.calls().is_empty(), "{:?}", second.calls());
}

#[test]
fn the_command_says_a_second_finish_only_updated_the_summary() {
    let f = fixture();
    ready(&f);
    let run = |github: &FakeGitHub| {
        let mut out = Vec::new();
        task_finish::run(
            &f.env.config_dir(),
            &task_finish::Options {
                target: Target::Issue(41),
                repo_root: f.env.tmp.path().join("target-repo"),
                push_unresolved: false,
            },
            &sbxm::backend::FakeBackend::default(),
            github,
            &Probe,
            &mut out,
        )
        .unwrap();
        String::from_utf8(out).unwrap()
    };
    run(&FakeGitHub::default().with_next_comment_id(900));

    let out = run(&FakeGitHub::default());

    assert!(
        out.contains("issue-41: already finished with https://github.com/o/r/pull/"),
        "{out}"
    );
    assert!(
        out.contains("posted or updated the summary of the review rounds on issue #41"),
        "{out}"
    );
    assert!(!out.contains("pushed issue-41"), "{out}");
}
