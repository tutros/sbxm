//! M2b slice 10, steps 4-5: `finish` pushes the branch from `repo.git` to the remote (a local
//! bare repo standing in for GitHub) and opens the PR (spec §4, decisions 153, 158).

mod common;

use std::fs;

use common::git;
use common::task_fixture::{
    CLAUDE_DONE, Fixture, Play, Probe, backend, fixture, ok, play, worked_task,
};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::task::finish::{SECTION_CAP, check_can_finish, finish, pr_body, unresolved_section};
use sbxm::task::pipeline::Prepared;
use sbxm::task::record::{self, Kind, NewTask, Process, Record, Stage, Status};
use sbxm::task::review::must_fix_findings;

/// As `review::with_header` saves it: the reviewer's line, a blank line, then the review.
const REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n";

/// Issue 41 worked, its gates and review done by hand: `ready`, with a result and a review.
fn ready(f: &Fixture) -> Prepared {
    ready_with(f, &["a.txt"])
}

/// [`ready`], where the worker commits `files` (paths inside the repo).
fn ready_with(f: &Fixture, files: &[&str]) -> Prepared {
    let b = backend()
        .with_exec_output_matching("claude", ok(CLAUDE_DONE))
        .with_exec_hook(play(
            &f.env.base_dir().join("tasks").join("issue-41"),
            "main",
            "issue-41",
            Play {
                commits: files.iter().map(|s| (*s).to_owned()).collect(),
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
    fs::write(prepared.meta.join("review.md"), REVIEW).unwrap();
    prepared
}

fn origin_has(f: &Fixture, branch: &str) -> bool {
    std::process::Command::new("git")
        .current_dir(&f.origin)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .unwrap()
        .success()
}

fn reread(f: &Fixture) -> record::Record {
    Prepared::open(&f.env.base_dir(), "issue-41")
        .unwrap()
        .record
}

fn run(f: &Fixture, github: &FakeGitHub) -> anyhow::Result<sbxm::task::finish::Finished> {
    finish(&f.env.base_dir(), "issue-41", github, &Probe)
}

#[test]
fn a_ready_task_is_pushed_and_its_pr_is_opened_with_the_result_and_review() {
    let f = fixture();
    let prepared = ready(&f);
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    // The branch really went to the remote, and it is the commits of repo.git.
    assert!(origin_has(&f, "issue-41"));
    assert_eq!(
        git(&f.origin, &["rev-parse", "issue-41"]),
        git(&prepared.meta.join("repo.git"), &["rev-parse", "issue-41"])
    );
    let calls = github.calls();
    let [GhCall::PrCreate(repo, request)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    assert_eq!(repo, "o/r");
    assert_eq!(
        (request.head.as_str(), request.base.as_str()),
        ("issue-41", "main")
    );
    assert_eq!(request.title, "Fix 41");
    assert!(request.body.starts_with("Fixes #41\n"), "{}", request.body);
    assert!(
        request.body.contains("## Result\n\ndone\n"),
        "{}",
        request.body
    );
    assert!(
        request.body.contains("## Review\n\nReviewer: codex"),
        "{}",
        request.body
    );
    // A clean task gets a normal PR, as before issue 120.
    assert!(!request.draft);
    assert!(!done.draft);
    assert!(!request.body.contains("## Unresolved"), "{}", request.body);
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Finished, Status::Ok));
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
}

const REVIEW_WITH_MUST_FIX: &str = "Reviewer: codex (default)\n\nMust-fix findings: 2\n\n\
## Must fix\n\n\
### M-1: The lock is never released\n\n\
**Where:** `src/lock.rs:10`\n\
**What happens:** x\n\
**Why it matters:** y\n\
**Fix:** z\n\n\
### M-2: The count is off by one\n\n\
**Where:** `src/count.rs:3`\n\
**What happens:** x\n\
**Why it matters:** y\n\
**Fix:** z\n\n\
## Should fix\n\n\
### S-1: A name is unclear\n\n\
**Where:** `src/name.rs:1`\n\
**What happens:** x\n\
**Why it matters:** y\n\
**Fix:** z\n";

/// The PR request `finish` made, after checking it made exactly one.
fn only_pr_request(github: &FakeGitHub) -> sbxm::github::PrRequest {
    let calls = github.calls();
    let [GhCall::PrCreate(_, request)] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    request.clone()
}

#[test]
fn a_ready_task_with_must_fix_left_opens_a_draft_that_lists_the_findings() {
    let f = fixture();
    let prepared = ready(&f);
    fs::write(prepared.meta.join("review.md"), REVIEW_WITH_MUST_FIX).unwrap();
    let github = FakeGitHub::default();

    let done = run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    assert!(done.draft);
    let body = &request.body;
    assert!(body.starts_with("Fixes #41\n"), "{body}");
    let unresolved = body.find("## Unresolved").expect(body);
    assert!(unresolved < body.find("## Result").unwrap(), "{body}");
    assert!(body.contains("2 must-fix finding(s) left"), "{body}");
    assert!(
        body.contains("- M-1: The lock is never released (`src/lock.rs:10`)"),
        "{body}"
    );
    assert!(
        body.contains("- M-2: The count is off by one (`src/count.rs:3`)"),
        "{body}"
    );
    // Only must-fix findings are the reason for the draft; the rest is in the review below.
    let section = &body[unresolved..body.find("## Result").unwrap()];
    assert!(!section.contains("S-1"), "{section}");
    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
}

#[test]
fn a_stopped_task_opens_a_draft_that_names_the_reason() {
    let f = fixture();
    let mut prepared = ready(&f);
    fs::write(prepared.meta.join("review.md"), REVIEW_WITH_MUST_FIX).unwrap();
    prepared.record.stopped = Some(record::Stopped::RepeatFinding);
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    assert!(request.body.contains("repeat-finding"), "{}", request.body);
    assert!(request.body.contains("- M-1: The lock"), "{}", request.body);
}

#[test]
fn a_stopped_task_with_no_must_fix_left_is_still_a_draft() {
    let f = fixture();
    let mut prepared = ready(&f);
    prepared.record.stopped = Some(record::Stopped::RoundsExhausted);
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    assert!(
        request.body.contains("rounds-exhausted"),
        "{}",
        request.body
    );
    assert!(request.body.contains("## Unresolved"), "{}", request.body);
}

#[test]
fn the_recorded_review_decides_the_draft_not_an_edited_review_md() {
    // Decision 177(f): `finish` reads the review result recorded in task.json, so a review.md
    // edited to say 0 can't turn a task with must-fix left into a normal PR.
    let f = fixture();
    let mut prepared = ready(&f);
    prepared.record.review = Some(record::ReviewResult {
        round: 1,
        must_fix: 1,
        full: true,
        repeat: false,
    });
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    assert!(
        request.body.contains("1 must-fix finding(s) left"),
        "{}",
        request.body
    );
}

#[test]
fn a_review_md_without_a_must_fix_count_and_no_recorded_review_is_a_draft() {
    let f = fixture();
    let prepared = ready(&f);
    fs::write(
        prepared.meta.join("review.md"),
        "Reviewer: codex (default)\n\nLooks fine.\n",
    )
    .unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    assert!(
        request.body.contains("no must-fix count"),
        "{}",
        request.body
    );
}

#[test]
fn a_task_that_is_not_ready_is_refused_before_anything_is_pushed_or_opened() {
    let f = fixture();
    let b = backend().with_exec_output_matching("claude", ok(CLAUDE_DONE));
    worked_task(&f, &b);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("not ready; run `sbxm task review --issue 41`"),
        "{message}"
    );
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_task_with_a_pr_is_refused_and_nothing_is_pushed_again() {
    let f = fixture();
    ready(&f);
    let github = FakeGitHub::default();
    run(&f, &github).unwrap();
    let before = git(&f.origin, &["rev-parse", "issue-41"]);

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("already has a PR"), "{err:#}");
    assert_eq!(github.calls().len(), 1);
    assert_eq!(git(&f.origin, &["rev-parse", "issue-41"]), before);
}

#[test]
fn a_pr_that_cannot_be_opened_leaves_a_ready_task_with_a_note_and_a_rerun_opens_it() {
    let f = fixture();
    ready(&f);
    let down = FakeGitHub::default().failing("gh: HTTP 502");

    let err = run(&f, &down).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("pushed issue-41 but could not open the PR"),
        "{message}"
    );
    assert!(message.contains("gh: HTTP 502"), "{message}");
    assert!(origin_has(&f, "issue-41"));
    let record = reread(&f);
    assert_eq!((record.stage, record.status), (Stage::Ready, Status::Ok));
    assert!(record.pr.is_none());
    assert_eq!(record.notes.len(), 1, "{:?}", record.notes);

    let up = FakeGitHub::default();
    let done = run(&f, &up).unwrap();

    let record = reread(&f);
    assert_eq!(record.stage, Stage::Finished);
    assert_eq!(record.pr.as_deref(), Some(done.url.as_str()));
    assert!(record.notes.is_empty(), "{:?}", record.notes);
}

#[test]
fn a_failed_push_opens_no_pr_and_leaves_the_record_as_it_was() {
    let f = fixture();
    let prepared = ready(&f);
    let before = fs::read_to_string(prepared.meta.join("task.json")).unwrap();
    // The remote is gone.
    fs::remove_dir_all(&f.origin).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(
        format!("{err:#}").contains("cannot push issue-41"),
        "{err:#}"
    );
    assert!(github.calls().is_empty());
    assert_eq!(
        fs::read_to_string(prepared.meta.join("task.json")).unwrap(),
        before
    );
}

#[test]
fn a_task_without_commits_beyond_the_base_is_refused() {
    let f = fixture();
    let prepared = ready(&f);
    let repo_git = prepared.meta.join("repo.git");
    git(&repo_git, &["branch", "-f", "issue-41", "main"]);
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(format!("{err:#}").contains("no commits"), "{err:#}");
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_secret_in_review_or_result_stops_the_publish_without_echoing_it() {
    let f = fixture();
    let prepared = ready(&f);
    fs::write(
        prepared.meta.join("review.md"),
        "Must-fix findings: 0\nkey: ghp_abcdefghijklmnopqrstuvwxyz0123456789\n",
    )
    .unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(
        message.contains("review.md line 2 looks like a secret"),
        "{message}"
    );
    assert!(!message.contains("ghp_abc"), "{message}");
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

#[test]
fn a_huge_file_is_cut_to_the_cap_and_the_body_says_so() {
    let big = "x".repeat(SECTION_CAP * 2);

    let (body, cuts) = pr_body(7, None, Some(&big), None);

    assert!(body.chars().count() < 65_536, "{}", body.len());
    assert!(
        body.contains("(cut: result.md has 50000 characters"),
        "{body}"
    );
    assert!(body.contains("(no review.md)"), "{body}");
    assert_eq!(cuts.len(), 1);
}

#[test]
fn a_record_that_names_a_hostile_branch_is_refused() {
    let f = fixture();
    let mut prepared = ready(&f);
    prepared.record.branch = "--upload-pack=x".into();
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    assert!(
        format!("{err:#}").contains("isn't a usable branch name"),
        "{err:#}"
    );
    assert!(github.calls().is_empty());
}

// ---- Slice 10 review, must-fix 2: the agent's commits must not start CI before anyone looks ----

#[test]
fn a_branch_that_changes_a_workflow_is_refused_before_anything_is_pushed() {
    for path in [
        ".github/workflows/ci.yml",
        ".github/actions/setup/action.yml",
        // GitHub reads these paths as written; sbxm must not be fooled by letter case.
        ".GitHub/Workflows/ci.yml",
    ] {
        let f = fixture();
        ready_with(&f, &["a.txt", path]);
        let before =
            fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json"))
                .unwrap();
        let github = FakeGitHub::default();

        let message = format!("{:#}", run(&f, &github).unwrap_err());

        assert!(message.contains(path), "{path}: {message}");
        assert!(
            message.contains("secrets") || message.contains("run"),
            "{message}"
        );
        assert!(
            !origin_has(&f, "issue-41"),
            "{path}: the branch must not have been pushed"
        );
        assert!(github.calls().is_empty(), "{path}: no PR may be opened");
        let after =
            fs::read_to_string(record::task_dir(&f.env.base_dir(), "issue-41").join("task.json"))
                .unwrap();
        assert_eq!(before, after, "{path}: the task stays ready and unchanged");
    }
}

#[test]
fn only_the_workflow_files_are_named_in_the_refusal() {
    let f = fixture();
    ready_with(&f, &["src/a.rs", ".github/workflows/ci.yml", "README.md"]);

    let message = format!("{:#}", run(&f, &FakeGitHub::default()).unwrap_err());

    assert!(message.contains(".github/workflows/ci.yml"), "{message}");
    assert!(
        !message.contains("src/a.rs") && !message.contains("README.md"),
        "{message}"
    );
}

/// Issue 143, decision 174(e): a spec task can be finished once `ready` (through its sink, not a
/// PR); before that the refusal names the spec form of the review command. The issue 142 version
/// of this test refused it at every stage, since no sink existed yet.
#[test]
fn a_spec_task_is_refused_until_ready_and_may_be_finished_once_ready() {
    let mut record = Record::new(
        &NewTask {
            kind: Kind::Spec,
            number: 0,
            repo: "o/r",
            title: "idea.md",
            base: "main",
            branch: "spec-idea-abc123",
            config_hash: "h",
            id: Some("spec-idea-abc123"),
        },
        0,
        Process::new(1, 0),
    );

    let message = format!("{:#}", check_can_finish(&record).unwrap_err());
    assert!(
        message.contains("spec-idea-abc123") && message.contains("not ready"),
        "{message}"
    );
    assert!(message.contains("task review --spec"), "{message}");
    assert!(!message.contains("--issue"), "{message}");

    for (stage, done) in [
        (Stage::Working, Status::Completed),
        (Stage::Gating, Status::Passed),
        (Stage::Reviewing, Status::Completed),
    ] {
        record.advance(stage, 0, Process::new(1, 0)).unwrap();
        record.finish(done).unwrap();
    }
    record.advance(Stage::Ready, 0, Process::new(1, 0)).unwrap();
    check_can_finish(&record).unwrap();
}

/// The issue path never publishes a spec task as a PR, even a ready one.
#[test]
fn the_pr_path_refuses_a_spec_task() {
    let f = fixture();
    let spec = f.env.tmp.path().join("idea.md");
    fs::write(&spec, "Build a thing.\n").unwrap();
    let b = backend();
    let github = FakeGitHub::default();
    let source = common::task_fixture::source(&f);
    let prepared = sbxm::task::pipeline::prepare_spec(
        &common::task_fixture::ctx(&f, &source, &b, &github),
        &spec,
    )
    .unwrap();

    let result = finish(&f.env.base_dir(), &prepared.record.id, &github, &Probe);

    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("never a PR"), "{message}");
    assert!(github.calls().is_empty());
}

#[test]
fn other_files_under_dot_github_and_look_alike_paths_are_fine() {
    let f = fixture();
    ready_with(
        &f,
        &[
            ".github/ISSUE_TEMPLATE/bug.md",
            ".github/workflows-notes.md",
            "docs/.github/workflows/example.yml",
        ],
    );
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    assert!(origin_has(&f, "issue-41"));
}

#[test]
fn a_long_list_of_must_fix_findings_is_cut_and_counted() {
    let mut review = "Must-fix findings: 200\n\n## Must fix\n\n".to_owned();
    for n in 1..=200 {
        review.push_str(&format!(
            "### M-{n}: {}\n\n**Where:** `src/a.rs:{n}`\n\n",
            "t".repeat(100)
        ));
    }

    let section = unresolved_section(&["why".to_owned()], &must_fix_findings(&review, 0));

    assert!(section.chars().count() < 6_000, "{}", section.len());
    assert!(section.contains("- M-1: "), "{section}");
    assert!(!section.contains("- M-200: "), "{section}");
    assert!(section.trim_end().ends_with("more"), "{section}");
}

#[test]
fn one_overlong_must_fix_finding_is_cut_to_the_cap() {
    let review = format!(
        "Must-fix findings: 1\n\n## Must fix\n\n### M-1: {}\n\n**Where:** `{}`\n\n",
        "t".repeat(6_000),
        "w".repeat(6_000)
    );

    let section = unresolved_section(&["why".to_owned()], &must_fix_findings(&review, 0));

    assert!(section.chars().count() <= 5_000, "{}", section.len());
    assert!(section.contains("- M-1: ttt"), "{section}");
    assert!(section.contains("(cut)"), "{section}");
}

#[test]
fn every_multi_finding_unresolved_section_obeys_the_five_thousand_character_cap() {
    // Review S-1, round 2: a first finding that fits, then one that doesn't, must leave room for
    // the `and N more` line, whatever the first one's length.
    for len in 4_700..5_000 {
        let review = format!(
            "Must-fix findings: 2\n\n## Must fix\n\n### M-1: {}\n\n### M-2: second\n\n",
            "t".repeat(len)
        );

        let section = unresolved_section(&["why".to_owned()], &must_fix_findings(&review, 0));

        let chars = section.chars().count();
        assert!(chars <= 5_000, "title length {len} produced {chars} chars");
        assert!(section.contains("- M-1: "), "{len}");
        assert!(
            section.contains("- M-2: ") || section.contains("- and 1 more"),
            "title length {len} lost M-2 without counting it"
        );
    }
}

/// Round 2's narrow review, which repeats only M-1 of round 1's M-1 and M-2.
const NARROW_REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 1\n\n## Must fix\n\n\
    ### M-1: First problem remains\n\n**Where:** `src/a.rs:5`\n\n**Repeat of:** M-1\n";

/// [`ready`], stopped by a narrow review that repeated M-1, with round 1's M-2 still open.
fn stopped_after_a_narrow_review(f: &Fixture) -> Prepared {
    let mut prepared = ready(f);
    prepared.record.stopped = Some(record::Stopped::RepeatFinding);
    prepared.record.review = Some(record::ReviewResult {
        round: 2,
        must_fix: 1,
        full: false,
        repeat: true,
    });
    prepared.record.open_findings = Some(vec![
        record::OpenFinding {
            round: 1,
            id: "M-2".into(),
            title: "Second problem".into(),
            place: Some("`src/b.rs:9`".into()),
        },
        record::OpenFinding {
            round: 2,
            id: "M-1".into(),
            title: "First problem remains".into(),
            place: Some("`src/a.rs:5`".into()),
        },
    ]);
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review.md"), NARROW_REVIEW).unwrap();
    prepared
}

#[test]
fn a_stopped_narrow_review_lists_every_finding_still_open_from_the_full_review() {
    let f = fixture();
    stopped_after_a_narrow_review(&f);
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    let section = request.body.split("## Result").next().unwrap();
    assert!(
        section.contains("- M-1 (review 2): First problem remains (`src/a.rs:5`)"),
        "{section}"
    );
    assert!(
        section.contains("- M-2 (review 1): Second problem (`src/b.rs:9`)"),
        "{section}"
    );
}

#[test]
fn an_edited_or_deleted_review_md_cannot_change_the_recorded_open_findings() {
    for edit in [
        Some(
            "Reviewer: codex (default)\n\nMust-fix findings: 1\n\n## Must fix\n\n### M-7: Invented\n",
        ),
        Some("Reviewer: codex (default)\n\nMust-fix findings: 0\n\nNothing found.\n"),
        None,
    ] {
        let f = fixture();
        let prepared = stopped_after_a_narrow_review(&f);
        match edit {
            Some(text) => fs::write(prepared.meta.join("review.md"), text).unwrap(),
            None => fs::remove_file(prepared.meta.join("review.md")).unwrap(),
        }
        let github = FakeGitHub::default();

        run(&f, &github).unwrap();

        let request = only_pr_request(&github);
        assert!(request.draft, "{edit:?}");
        let section = request.body.split("## Result").next().unwrap();
        assert!(
            section.contains("M-2 (review 1): Second problem"),
            "{section}"
        );
        assert!(
            section.contains("M-1 (review 2): First problem remains"),
            "{section}"
        );
        assert!(!section.contains("M-7"), "{section}");
        assert!(!section.contains("review.md has no"), "{section}");
    }
}

#[test]
fn a_secret_in_a_recorded_open_finding_stops_the_publish_without_echoing_it() {
    // Review round 3, M-1: the draft's section comes from task.json, not from a checked file, so
    // a deleted or benign review.md must not let a recorded secret-looking title through.
    for review in [None, Some(REVIEW)] {
        let f = fixture();
        let mut prepared = stopped_after_a_narrow_review(&f);
        prepared.record.open_findings.as_mut().unwrap()[0].title =
            "key ghp_abcdefghijklmnopqrstuvwxyz0123456789".into();
        record::write(&prepared.meta, &prepared.record).unwrap();
        match review {
            Some(text) => fs::write(prepared.meta.join("review.md"), text).unwrap(),
            None => fs::remove_file(prepared.meta.join("review.md")).unwrap(),
        }
        let github = FakeGitHub::default();

        let err = run(&f, &github).unwrap_err();

        let message = format!("{err:#}");
        assert!(message.contains("looks like a secret"), "{message}");
        assert!(message.contains("open findings"), "{message}");
        assert!(!message.contains("ghp_abc"), "{message}");
        assert!(github.calls().is_empty(), "{review:?}");
        assert!(!origin_has(&f, "issue-41"), "{review:?}");
    }
}

#[test]
fn a_secret_past_the_cut_of_a_long_recorded_finding_still_stops_the_publish() {
    // PR #165 review, M-1: a finding longer than the section's cap is cut, so the check must read
    // the recorded finding itself, not only the cut section.
    let f = fixture();
    let mut prepared = stopped_after_a_narrow_review(&f);
    prepared.record.open_findings.as_mut().unwrap()[0].title = format!(
        "{} ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "x".repeat(4_843)
    );
    record::write(&prepared.meta, &prepared.record).unwrap();
    let github = FakeGitHub::default();

    let err = run(&f, &github).unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("looks like a secret"), "{message}");
    assert!(!message.contains("ghp_abc"), "{message}");
    assert!(github.calls().is_empty());
    assert!(!origin_has(&f, "issue-41"));
}

/// Round 1's full review: M-1 and M-2.
const FULL_REVIEW: &str = "Reviewer: codex (default)\n\nMust-fix findings: 2\n\n## Must fix\n\n\
    ### M-1: First problem\n\n**Where:** `src/a.rs:1`\n\n\
    ### M-2: Second problem\n\n**Where:** `src/b.rs:9`\n";

#[test]
fn an_older_record_without_open_findings_lists_them_from_its_saved_reviews() {
    // PR #165 review, M-2: a record from before `open_findings` rebuilds the list from its saved
    // review rounds, so a narrow last review.md doesn't hide round 1's M-2.
    let f = fixture();
    let mut prepared = stopped_after_a_narrow_review(&f);
    prepared.record.open_findings = None;
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review-1.md"), FULL_REVIEW).unwrap();
    fs::write(prepared.meta.join("review-2.md"), NARROW_REVIEW).unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    assert!(request.draft);
    let section = request.body.split("## Result").next().unwrap();
    assert!(
        section.contains("- M-2 (review 1): Second problem (`src/b.rs:9`)"),
        "{section}"
    );
    assert!(
        section.contains("- M-1 (review 2): First problem remains (`src/a.rs:5`)"),
        "{section}"
    );
    assert!(
        !section.contains("First problem (`src/a.rs:1`)"),
        "{section}"
    );
}

#[test]
fn an_older_record_rebuilds_its_findings_when_review_rounds_do_not_start_at_one() {
    // PR #165 review round 2, M-1: gate failures can use fix rounds before the first review, so
    // the saved reviews may start at review-4.md; none of them may be missed.
    let f = fixture();
    let mut prepared = stopped_after_a_narrow_review(&f);
    prepared.record.open_findings = None;
    record::write(&prepared.meta, &prepared.record).unwrap();
    fs::write(prepared.meta.join("review-4.md"), FULL_REVIEW).unwrap();
    fs::write(prepared.meta.join("review-5.md"), NARROW_REVIEW).unwrap();
    let github = FakeGitHub::default();

    run(&f, &github).unwrap();

    let request = only_pr_request(&github);
    let section = request.body.split("## Result").next().unwrap();
    assert!(
        section.contains("- M-2 (review 4): Second problem (`src/b.rs:9`)"),
        "{section}"
    );
    assert!(
        section.contains("- M-1 (review 5): First problem remains (`src/a.rs:5`)"),
        "{section}"
    );
}
