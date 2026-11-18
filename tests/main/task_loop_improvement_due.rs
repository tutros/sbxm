//! Issue 715 (decision 330(2) and (3)): an idle `sbxm task loop --wait` queues an improvement
//! review once 20 PRs merged since the date of the newest `sdlc/reviews/*-improvement-*.md` (the
//! day after it being the first counted, UTC), as a spec job of the area of
//! `sdlc/reviews/areas.md` whose newest review is oldest. No second job while one is queued,
//! running or its PR is open; an `areas.md` that can't be used logs one line and queues nothing;
//! the merged review is one toast. The repo files and merged PRs are `FakeGitHub`'s.

use crate::common;

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use common::Env as TestEnv;
use common::task_fixture::{Probe, open_issue};
use sbxm::backend::Location;
use sbxm::commands::task_loop::{self, Env, Options, Steps};
use sbxm::github::fake::{FakeGitHub, GhCall};
use sbxm::github::{MergedPr, PrInfo, PrState};
use sbxm::notify::FakeNotifier;
use sbxm::run::results::parse_timestamp;
use sbxm::task::improvement::{self, Area};
use sbxm::task::jobs::{self, Job};
use sbxm::task::loop_log;
use sbxm::task::queue::{self, Row};
use sbxm::task::record::{self, Kind, NewTask, Process, Record};

const REPO: &str = "o/r";
const HOUR: u64 = 60 * 60;
const LOG_ISSUE: u32 = 50;

/// Three areas, in this order.
const AREAS: &str = "# Improvement review areas (decision 330)\n\n| Slug | Paths |\n|---|---|\n\
| alpha | `src/a/` |\n| beta | `src/b/`, `src/c.rs` |\n| gamma | `src/g/` |\n";

fn at(stamp: &str) -> u64 {
    parse_timestamp(stamp).unwrap()
}

/// When the loop runs.
fn start() -> u64 {
    at("2026-11-18T10:00:00Z")
}

/// The day the loop runs, as a spec file names it.
const TODAY: &str = "2026-11-18";

/// The first second counted after a review dated 2026-11-16.
fn after_newest() -> u64 {
    at("2026-11-17T00:00:00Z")
}

struct NoSteps;

impl Steps for NoSteps {
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

struct Fixture {
    env: TestEnv,
    repo_root: PathBuf,
}

/// A fixture with `specs_dir` set to an empty folder.
fn fixture() -> Fixture {
    let env = TestEnv::new();
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        "[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n",
    )
    .unwrap();
    let f = Fixture { env, repo_root };
    fs::create_dir_all(f.specs()).unwrap();
    let value = toml::Value::String(f.specs().to_str().unwrap().to_owned());
    f.env.set_config(&format!("specs_dir = {value}"));
    f
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }

    fn specs(&self) -> PathBuf {
        self.env.tmp.path().join("specs")
    }

    fn opts(&self) -> Options {
        Options {
            repo_root: self.repo_root.clone(),
            issues: Vec::new(),
            cap: None,
            repo: Some(REPO.into()),
            wait: true,
        }
    }

    /// The spec jobs waiting in the queue, by file.
    fn queued(&self) -> Vec<String> {
        jobs::pending(&self.base())
            .unwrap()
            .iter()
            .filter_map(|path| jobs::parse(&fs::read_to_string(path).unwrap()).ok())
            .filter_map(|job| match job {
                Job::Spec { file } => Some(file),
                _ => None,
            })
            .collect()
    }

    /// The loop's own log lines that name the improvement review.
    fn notes(&self) -> Vec<String> {
        fs::read_to_string(loop_log::path(&self.base(), start()))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("improvement"))
            .map(str::to_owned)
            .collect()
    }

    /// Queues `job` as `task queue` would have, before the loop starts.
    fn queue_job(&self, dir: &Path, file: &str) {
        fs::create_dir_all(dir).unwrap();
        let job = serde_json::to_string(&Job::Spec { file: file.into() }).unwrap();
        fs::write(dir.join("2026-11-17T090000Z-spec-x.json"), job).unwrap();
    }
}

/// `n` PRs merged at `when`, numbered from `from`.
fn merged_from(from: u32, n: u32, when: u64) -> Vec<MergedPr> {
    (0..n)
        .map(|i| MergedPr {
            number: from + i,
            merged_at: when,
        })
        .collect()
}

fn merged(n: u32) -> Vec<MergedPr> {
    merged_from(600, n, after_newest())
}

/// The review files: `sdlc/reviews/<date>-improvement-<slug>.md`, each with one finding.
fn reviews(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(date, slug)| {
            (
                format!("sdlc/reviews/{date}-improvement-{slug}.md"),
                "# Improvement review\n\n### IMP-1 - x\n".to_owned(),
            )
        })
        .collect()
}

/// GitHub whose `main` has `areas` (when given) and `reviews`, with `prs` merged and an open
/// `loop-log` issue.
fn github(areas: Option<&str>, reviews: &[(String, String)], prs: Vec<MergedPr>) -> FakeGitHub {
    let mut files: Vec<(&str, &str)> = reviews
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    if let Some(areas) = areas {
        files.push(("sdlc/reviews/areas.md", areas));
    }
    files.push(("sdlc/reviews/2026-11-01-other.md", "a code review"));
    FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .with_open_issues(vec![open_issue(LOG_ISSUE, &["loop-log"], "")])
        .with_repo_files(&files)
        .with_merged_prs(prs)
}

struct Ran {
    out: String,
    warn: String,
}

/// Runs a `--wait` loop on an empty queue from [`start`], stopped after `waits` idle waits.
fn run_loop(f: &Fixture, gh: &FakeGitHub, notifier: &FakeNotifier, waits: usize) -> Ran {
    run_loop_between(f, gh, notifier, waits, &|_| {})
}

/// [`run_loop`], `between` running at each sleep with the number of sleeps so far.
fn run_loop_between(
    f: &Fixture,
    gh: &FakeGitHub,
    notifier: &FakeNotifier,
    waits: usize,
    between: &dyn Fn(usize),
) -> Ran {
    let clock = Cell::new(start());
    let sleeps = Cell::new(0usize);
    let now = || clock.get();
    let sleep = |d: Duration| {
        sleeps.set(sleeps.get() + 1);
        clock.set(clock.get() + d.as_secs());
        between(sleeps.get());
    };
    let stop = || sleeps.get() >= waits;
    let env = Env {
        now: &now,
        sleep: &sleep,
        stop_requested: &stop,
    };
    let (mut out, mut warn) = (Vec::new(), Vec::new());
    task_loop::run_notifying(
        &f.env.config_dir(),
        &f.opts(),
        &NoSteps,
        gh,
        &Probe,
        &env,
        notifier,
        &mut out,
        &mut warn,
    )
    .unwrap();
    Ran {
        out: String::from_utf8(out).unwrap(),
        warn: String::from_utf8(warn).unwrap(),
    }
}

/// Waits enough for two checks an hour apart.
const TWO_CHECKS: usize = (HOUR / task_loop::PAUSE_POLL.as_secs()) as usize + 1;

fn spec_name(slug: &str) -> String {
    format!("improvement-{slug}-{TODAY}.md")
}

/// alpha reviewed on 2026-11-16, beta and gamma earlier: beta is the oldest.
fn all_reviewed() -> Vec<(String, String)> {
    reviews(&[
        ("2026-11-16", "alpha"),
        ("2026-11-10", "beta"),
        ("2026-11-12", "gamma"),
    ])
}

// --- The threshold ---

#[test]
fn task_loop_improvement_due_nineteen_merged_prs_are_not_due() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(19));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, 1);

    assert!(f.queued().is_empty(), "{}", ran.out);
    assert!(!f.specs().join(spec_name("beta")).exists());
    assert!(f.notes().is_empty(), "{:?}", f.notes());
}

#[test]
fn task_loop_improvement_due_twenty_merged_prs_queue_one_spec_job() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));
    let notifier = FakeNotifier::default();

    let ran = run_loop(&f, &gh, &notifier, 1);

    assert_eq!(f.queued(), [spec_name("beta")], "{}\n{}", ran.out, ran.warn);
    let spec = fs::read_to_string(f.specs().join(spec_name("beta"))).unwrap();
    assert!(
        spec.contains("sdlc/reviews/2026-11-18-improvement-beta.md"),
        "{spec}"
    );
    assert!(spec.contains("`src/b/`, `src/c.rs`"), "{spec}");
    let notes = f.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("20 PRs merged since 2026-11-16"),
        "{notes:?}"
    );
    assert!(!ran.warn.contains("improvement"), "{}", ran.warn);
}

#[test]
fn task_loop_improvement_due_a_pr_merged_on_the_reviews_day_doesnt_count() {
    let f = fixture();
    let mut prs = merged(19);
    prs.extend(merged_from(700, 1, after_newest() - 1));
    let gh = github(Some(AREAS), &all_reviewed(), prs);

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
}

// The loop drains its queue before it waits, so in a loop with one task at a time a job is
// queued or running at a check only in a loop with children: the rule is checked as it is.

#[test]
fn task_loop_improvement_due_twenty_one_with_a_job_queued_queue_no_second() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(21));
    assert!(!improvement::in_flight(&f.base(), REPO, &gh).unwrap());

    f.queue_job(
        &jobs::queue_dir(&f.base()),
        "improvement-alpha-2026-11-17.md",
    );

    assert!(improvement::in_flight(&f.base(), REPO, &gh).unwrap());
}

#[test]
fn task_loop_improvement_due_a_job_running_is_in_flight() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(21));
    f.queue_job(
        &jobs::running_dir(&f.base()),
        "improvement-alpha-2026-11-17.md",
    );

    assert!(improvement::in_flight(&f.base(), REPO, &gh).unwrap());
}

#[test]
fn task_loop_improvement_due_another_spec_job_queued_isnt_in_flight() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(21));
    f.queue_job(&jobs::queue_dir(&f.base()), "retro.md");

    assert!(!improvement::in_flight(&f.base(), REPO, &gh).unwrap());
}

/// A live spec task of an improvement job, with PR 800.
fn improvement_task(f: &Fixture) {
    let id = "spec-improvement-alpha-2026-11-17-abcdef";
    let mut task = Record::new(
        &NewTask {
            kind: Kind::Spec,
            number: 0,
            repo: REPO,
            title: "improvement-alpha-2026-11-17.md",
            base: "main",
            branch: id,
            config_hash: "h",
            id: Some(id),
        },
        at("2026-11-17T09:00:00Z"),
        Process::new(1, at("2026-11-17T09:00:00Z")),
    );
    task.location = Location::Local;
    task.pr = Some("https://github.com/o/r/pull/800".into());
    record::write(&record::task_dir(&f.base(), id), &task).unwrap();
}

fn pr800(state: PrState) -> PrInfo {
    PrInfo {
        number: 800,
        head_ref: "spec-improvement-alpha-2026-11-17-abcdef".into(),
        head_oid: "b".repeat(40),
        is_cross_repository: false,
        head_repo: REPO.into(),
        state,
        title: "Improvement review alpha".into(),
        body: String::new(),
        closing_issues: vec![],
        base_moved: false,
    }
}

#[test]
fn task_loop_improvement_due_twenty_one_with_its_pr_open_queue_no_second() {
    let f = fixture();
    improvement_task(&f);
    let gh = github(Some(AREAS), &all_reviewed(), merged(21)).with_pr(pr800(PrState::Open));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
    assert!(gh.calls().contains(&GhCall::Pr(REPO.into(), 800)));
}

#[test]
fn task_loop_improvement_due_a_closed_pr_of_an_earlier_job_doesnt_hold_a_new_one() {
    let f = fixture();
    improvement_task(&f);
    let gh = github(Some(AREAS), &all_reviewed(), merged(21)).with_pr(pr800(PrState::Closed));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert_eq!(f.queued(), [spec_name("beta")]);
}

#[test]
fn task_loop_improvement_due_queues_once_over_two_checks() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), TWO_CHECKS);

    // The loop took the job at its next pass; the second check queued none.
    let done: Vec<_> = fs::read_dir(jobs::done_dir(&f.base()))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(done.len(), 1, "{done:?}\n{}", ran.out);
    assert!(
        done[0].contains("spec-improvement-beta-2026-11-18"),
        "{done:?}"
    );
    assert!(f.queued().is_empty());
    assert_eq!(
        ran.out.matches("improvement review of beta queued").count(),
        1
    );
}

// --- No review yet, and the area picked ---

#[test]
fn task_loop_improvement_due_no_review_at_all_is_due_and_takes_the_first_area() {
    let f = fixture();
    let gh = github(Some(AREAS), &[], Vec::new());

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert_eq!(f.queued(), [spec_name("alpha")], "{}", ran.warn);
    // No review: nothing to count from, so the merged PRs aren't read.
    assert!(
        !gh.calls()
            .iter()
            .any(|c| matches!(c, GhCall::PrsMergedSince(..)))
    );
}

#[test]
fn task_loop_improvement_due_two_areas_never_reviewed_go_in_table_order() {
    let f = fixture();
    let gh = github(
        Some(AREAS),
        &reviews(&[("2026-11-16", "alpha")]),
        merged(20),
    );

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert_eq!(f.queued(), [spec_name("beta")]);
}

fn areas() -> Vec<Area> {
    improvement::areas(AREAS).unwrap()
}

fn names(list: &[(&str, &str)]) -> Vec<String> {
    list.iter()
        .map(|(date, slug)| format!("{date}-improvement-{slug}.md"))
        .collect()
}

#[test]
fn task_loop_improvement_due_areas_are_read_in_table_order() {
    let slugs: Vec<String> = areas().into_iter().map(|a| a.slug).collect();
    assert_eq!(slugs, ["alpha", "beta", "gamma"]);
    assert_eq!(areas()[1].paths, "`src/b/`, `src/c.rs`");
}

#[test]
fn task_loop_improvement_due_two_never_reviewed_pick_the_first_in_table_order() {
    let areas = areas();
    let picked = improvement::pick(&areas, &names(&[("2026-11-16", "alpha")])).unwrap();
    assert_eq!(picked.slug, "beta");
}

#[test]
fn task_loop_improvement_due_all_reviewed_pick_the_oldest() {
    let areas = areas();
    let names = names(&[
        ("2026-11-16", "alpha"),
        ("2026-11-12", "gamma"),
        ("2026-11-14", "beta"),
        // An older review of alpha doesn't make it the oldest: its newest counts.
        ("2026-11-01", "alpha"),
    ]);
    assert_eq!(improvement::pick(&areas, &names).unwrap().slug, "gamma");
}

#[test]
fn task_loop_improvement_due_a_tie_on_the_date_goes_in_table_order() {
    let areas = areas();
    let names = names(&[
        ("2026-11-16", "alpha"),
        ("2026-11-10", "gamma"),
        ("2026-11-10", "beta"),
    ]);
    assert_eq!(improvement::pick(&areas, &names).unwrap().slug, "beta");
}

// --- areas.md that can't be used ---

fn assert_one_note_and_no_job(areas: Option<&str>, says: &str) {
    let f = fixture();
    let gh = github(areas, &all_reviewed(), merged(20));

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), TWO_CHECKS);

    assert!(f.queued().is_empty());
    let notes = f.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("areas.md"), "{notes:?}");
    assert!(notes[0].contains(says), "{notes:?}");
    assert!(ran.warn.contains("areas.md"), "{}", ran.warn);
    assert!(
        fs::read_dir(f.specs()).unwrap().next().is_none(),
        "no spec written"
    );
}

#[test]
fn task_loop_improvement_due_a_missing_areas_md_logs_one_line_and_queues_nothing() {
    assert_one_note_and_no_job(None, "is missing");
}

#[test]
fn task_loop_improvement_due_an_empty_areas_md_logs_one_line_and_queues_nothing() {
    assert_one_note_and_no_job(Some(""), "has no area");
}

#[test]
fn task_loop_improvement_due_a_row_with_no_paths_logs_one_line_and_queues_nothing() {
    let areas = "| Slug | Paths |\n|---|---|\n| alpha | `src/a/` |\n| beta |  |\n";
    assert_one_note_and_no_job(Some(areas), "no paths for the area beta");
}

#[test]
fn task_loop_improvement_due_a_row_with_no_paths_cell_is_refused() {
    let err = improvement::areas("| Slug | Paths |\n|---|---|\n| alpha |\n").unwrap_err();
    assert!(err.contains("no paths"), "{err}");
}

// --- A review of a slug not in areas.md ---

#[test]
fn task_loop_improvement_due_an_unknown_slug_counts_for_the_newest_review() {
    let f = fixture();
    let mut list = reviews(&[("2026-11-01", "alpha"), ("2026-11-02", "beta")]);
    list.extend(reviews(&[
        ("2026-11-03", "gamma"),
        ("2026-11-16", "old-area"),
    ]));
    // 19 since the unknown slug's review, 25 since the newest review of a listed area.
    let mut prs = merged(19);
    prs.extend(merged_from(700, 6, at("2026-11-05T00:00:00Z")));
    let gh = github(Some(AREAS), &list, prs);

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
}

#[test]
fn task_loop_improvement_due_an_unknown_slug_counts_for_no_area() {
    let f = fixture();
    let mut list = reviews(&[("2026-11-01", "alpha"), ("2026-11-02", "beta")]);
    list.extend(reviews(&[
        ("2026-11-03", "gamma"),
        ("2026-11-16", "old-area"),
    ]));
    let gh = github(Some(AREAS), &list, merged(20));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert_eq!(f.queued(), [spec_name("alpha")]);
}

// --- Errors ---

#[test]
fn task_loop_improvement_due_a_failed_read_is_tried_again_in_an_hour() {
    let f = fixture();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));
    gh.set_retro_reads_failing(Some("gh is down"));

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), TWO_CHECKS - 1);

    assert!(f.queued().is_empty());
    let notes = f.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("tried again in an hour"), "{notes:?}");
    assert!(
        ran.warn.contains("improvement review check"),
        "{}",
        ran.warn
    );
}

#[test]
fn task_loop_improvement_due_without_specs_dir_queues_nothing_and_says_so() {
    let f = fixture();
    let config = f.env.config_dir().join("config.toml");
    let text = fs::read_to_string(&config).unwrap();
    let text: String = text
        .lines()
        .filter(|l| !l.starts_with("specs_dir"))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&config, text).unwrap();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
    let notes = f.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("specs_dir"), "{notes:?}");
}

#[test]
fn task_loop_improvement_due_never_writes_over_a_spec_file() {
    let f = fixture();
    fs::write(f.specs().join(spec_name("beta")), "the user's own").unwrap();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
    assert_eq!(
        fs::read_to_string(f.specs().join(spec_name("beta"))).unwrap(),
        "the user's own"
    );
    assert_eq!(f.notes().len(), 1, "{:?}", f.notes());
}

// --- The toast on merge ---

#[test]
fn task_loop_improvement_due_a_merged_review_the_loop_queued_is_one_toast() {
    let f = fixture();
    // The loop's spec of beta, written yesterday; its review is now on main.
    fs::write(f.specs().join("improvement-beta-2026-11-17.md"), "the spec").unwrap();
    let mut list = all_reviewed();
    list.push((
        "sdlc/reviews/2026-11-17-improvement-beta.md".into(),
        "# R\n\n## Should fix\n\n### IMP-1 - a\n\n### IMP-2 - b\n\n## Questions\n\n### IMP-3 - c\n"
            .into(),
    ));
    let gh = github(Some(AREAS), &list, Vec::new());
    let notifier = FakeNotifier::default();

    run_loop(&f, &gh, &notifier, TWO_CHECKS);

    let shown: Vec<_> = notifier
        .shown()
        .into_iter()
        .filter(|n| n.key.starts_with("improvement"))
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0].body,
        "Improvement review beta merged: sort its 3 findings"
    );
}

// --- The run data in the spec ---

/// An issue task of `REPO` started at `started`.
fn task_started(f: &Fixture, issue: u32, started: &str) {
    let task = Record::new(
        &NewTask {
            kind: Kind::Issue,
            number: issue,
            repo: REPO,
            title: "t",
            base: "main",
            branch: "b",
            config_hash: "h",
            id: None,
        },
        at(started),
        Process::new(1, at(started)),
    );
    record::write(&record::task_dir(&f.base(), &task.id), &task).unwrap();
}

/// A loop log of `repo` started at `started`, with one row for `issue`.
fn loop_log_with(f: &Fixture, repo: &str, started: &str, issue: u32) {
    let path = loop_log::path(&f.base(), at(started));
    let row = Row {
        issue,
        title: format!("issue {issue}"),
        size: Some("S".into()),
        slot: None,
        started: at(started) + 60,
        ready: None,
        worker_s: Some(100),
        reviewer_s: None,
        rounds: None,
        must_fix: Vec::new(),
        outcome: "merged".into(),
        stops: Vec::new(),
        idle_before: None,
    };
    queue::append_row(&path, repo, &row).unwrap();
    written_at(&path, at(started) + 120);
}

/// Sets `path`'s modified time to `secs`, as when the loop last wrote it.
fn written_at(path: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn task_loop_improvement_due_the_spec_embeds_the_task_facts_and_loop_log_rows_since_the_newest_review()
 {
    let f = fixture();
    task_started(&f, 71, "2026-11-17T08:00:00Z");
    task_started(&f, 72, "2026-11-15T08:00:00Z");
    loop_log_with(&f, REPO, "2026-11-17T09:00:00Z", 81);
    loop_log_with(&f, REPO, "2026-11-15T09:00:00Z", 82);
    loop_log_with(&f, "x/y", "2026-11-17T10:00:00Z", 83);
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    run_loop(&f, &gh, &FakeNotifier::default(), 1);

    let spec = fs::read_to_string(f.specs().join(spec_name("beta"))).unwrap();
    assert!(
        spec.contains("`sbxm task facts --since 2026-11-16`"),
        "{spec}"
    );
    assert!(
        spec.contains("| issue-71 | 71 | 2026-11-17T08:00:00Z |"),
        "{spec}"
    );
    assert!(
        !spec.contains("issue-72"),
        "a task before the window: {spec}"
    );
    assert!(
        spec.contains("| 2026-11-17T09:00:00Z | 2026-11-17 | 1 | #81 issue 81 |"),
        "{spec}"
    );
    assert!(!spec.contains("#82"), "a loop before the window: {spec}");
    assert!(!spec.contains("#83"), "another repository's loop: {spec}");
    assert!(
        spec.contains("All the run data since 2026-11-16 is below."),
        "{spec}"
    );
}

// --- Review M-2 of issue 715: a failed read of the run data ---

/// A posted loop log of `REPO` started 2026-11-17T09:00 that isn't valid UTF-8.
fn broken_log(f: &Fixture) -> PathBuf {
    let path = loop_log::path(&f.base(), at("2026-11-17T09:00:00Z"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = b"# Loop log\n\nRepository: o/r\n\n".to_vec();
    text.push(0xff);
    fs::write(&path, text).unwrap();
    // Posted, so the loop's start doesn't post it late.
    loop_log::mark_posted(&path).unwrap();
    path
}

#[test]
fn task_loop_improvement_due_a_log_that_cant_be_read_queues_nothing_and_names_it() {
    let f = fixture();
    let log = broken_log(&f);
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), 1);

    assert!(f.queued().is_empty());
    assert!(fs::read_dir(f.specs()).unwrap().next().is_none(), "no spec");
    let notes = f.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let name = log.file_name().unwrap().to_str().unwrap();
    assert!(notes[0].contains(name), "{notes:?}");
    assert!(notes[0].contains("tried again in an hour"), "{notes:?}");
    assert!(ran.warn.contains(name), "{}", ran.warn);
}

#[test]
fn task_loop_improvement_due_a_repaired_log_waits_for_the_next_hour_then_queues() {
    let f = fixture();
    let log = broken_log(&f);
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));
    let repair = |sleeps: usize| {
        if sleeps == 1 {
            fs::write(&log, "# Loop log\n\nRepository: o/r\n\n").unwrap();
        }
    };

    let ran = run_loop_between(&f, &gh, &FakeNotifier::default(), TWO_CHECKS - 1, &repair);
    // The loop takes a job it queued at its next pass, so its own line tells.
    assert!(
        !ran.out.contains("improvement review of beta queued"),
        "no check before the hour: {}",
        ran.out
    );

    let f = fixture();
    let log = broken_log(&f);
    let repair = |sleeps: usize| {
        if sleeps == 1 {
            fs::write(&log, "# Loop log\n\nRepository: o/r\n\n").unwrap();
        }
    };
    let ran = run_loop_between(&f, &gh, &FakeNotifier::default(), TWO_CHECKS, &repair);
    assert!(
        ran.out.contains("improvement review of beta queued"),
        "{}\n{}",
        ran.out,
        ran.warn
    );
}

#[test]
fn task_loop_improvement_due_a_record_task_facts_skips_makes_the_spec_say_incomplete() {
    let f = fixture();
    let dir = record::tasks_root(&f.base()).join("issue-99");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("task.json"), "not json").unwrap();
    let gh = github(Some(AREAS), &all_reviewed(), merged(20));

    let ran = run_loop(&f, &gh, &FakeNotifier::default(), 1);

    let spec = fs::read_to_string(f.specs().join(spec_name("beta"))).unwrap();
    assert!(!spec.contains("All the run data"), "{spec}");
    assert!(spec.contains("the run data is incomplete"), "{spec}");
    assert!(spec.contains("issue-99"), "{spec}");
    assert!(ran.warn.contains("skipped 1 task records"), "{}", ran.warn);
}
