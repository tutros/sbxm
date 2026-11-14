//! Issue 640 (milestone 5 slice 6, decision 317(9), (10)): an issue whose **Related:** partner
//! has a task record waits for it instead of being refused for the run, and a child's usage limit
//! holds only the new tasks and jobs that would use its provider.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use sbxm::backend::{FakeBackend, Location};
use sbxm::commands::status::{self, LoopSource};
use sbxm::commands::task_loop::{self, Env, Options, Phase, PhaseEnd, Started, Steps};
use sbxm::github::fake::FakeGitHub;
use sbxm::github::{Issue, PrInfo, PrState};
use sbxm::task::outcome::{Kind, Outcome};
use sbxm::task::record::{self, NewTask, Process, Record, ReviewResult, Stage, Status};

use common::task_fixture::{Probe, issue_text, open_issue};

const REPO: &str = "o/r";
const READY: &str = "ready-for-worker";
/// 2026-11-02T10:00:00Z.
const START: u64 = 1_793_613_600;
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// The default roles: a claude worker, a codex reviewer.
const CLAUDE_CODEX: &str = "";
/// A codex worker, a claude reviewer.
const CODEX_CLAUDE: &str = "[worker]\nharness = \"codex\"\n\n[reviewer]\nharness = \"claude\"\n\n";

/// How a played phase ends.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Reaches `ready`, clean.
    Ready,
    /// Stops on a usage limit of this provider (`None`: not named), the record at `working`.
    UsageLimit(Option<&'static str>),
}

/// Plays each issue's phases as children that end after a number of polls, writing the record a
/// real child would; `events` says what started and ended, in order.
struct Play<'a> {
    base: PathBuf,
    script: RefCell<HashMap<u32, VecDeque<(u32, Act)>>>,
    events: RefCell<Vec<String>>,
    /// Run as each phase ends, with its issue and how: GitHub or the queue changing meanwhile.
    on_end: Box<dyn Fn(u32, Act) + 'a>,
    /// Whether a finish removes the task's record, as `task rm --merged` does once its PR merged.
    gone_after_finish: bool,
}

impl<'a> Play<'a> {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_owned(),
            script: RefCell::default(),
            events: RefCell::default(),
            on_end: Box::new(|_, _| {}),
            gone_after_finish: false,
        }
    }

    fn plays(self, issue: u32, phases: &[(u32, Act)]) -> Self {
        self.script
            .borrow_mut()
            .insert(issue, phases.iter().copied().collect());
        self
    }

    fn on_end(mut self, f: impl Fn(u32, Act) + 'a) -> Self {
        self.on_end = Box::new(f);
        self
    }

    fn gone_after_finish(mut self) -> Self {
        self.gone_after_finish = true;
        self
    }

    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }

    /// The index of `event` among the events; panics when it is not there.
    fn at(&self, event: &str) -> usize {
        self.events()
            .iter()
            .position(|e| e == event)
            .unwrap_or_else(|| panic!("no {event:?} in {:?}", self.events()))
    }

    /// The index of the first event that starts with `prefix`; panics when there is none.
    fn first(&self, prefix: &str) -> usize {
        self.events()
            .iter()
            .position(|e| e.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?}... in {:?}", self.events()))
    }

    fn has(&self, prefix: &str) -> bool {
        self.events().iter().any(|e| e.starts_with(prefix))
    }

    fn next(&self, issue: u32) -> (u32, Act) {
        self.script
            .borrow_mut()
            .get_mut(&issue)
            .and_then(VecDeque::pop_front)
            .unwrap_or((0, Act::Ready))
    }
}

fn new_record(issue: u32, location: Location) -> Record {
    let mut rec = Record::new(
        &NewTask {
            kind: record::Kind::Issue,
            number: issue,
            repo: REPO,
            title: &format!("Fix {issue}"),
            base: "main",
            branch: &format!("sbxm/issue-{issue}"),
            config_hash: "h",
            id: None,
        },
        START,
        Process::new(1, 0),
    );
    rec.location = location;
    rec
}

/// Issue `issue`'s record as a child leaves it after `act`, in `location`.
fn write_record(base: &Path, issue: u32, act: Act, location: Location) {
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec =
        record::read(&dir.join("task.json")).unwrap_or_else(|_| new_record(issue, location));
    match act {
        Act::Ready => {
            rec.stage = Stage::Ready;
            rec.status = Status::Ok;
            rec.review = Some(ReviewResult {
                round: 1,
                must_fix: 0,
                not_run: Some(Vec::new()),
                ..ReviewResult::default()
            });
        }
        Act::UsageLimit(_) => {
            rec.stage = Stage::Working;
            rec.status = Status::Failed;
        }
    }
    record::write(&dir, &rec).unwrap();
}

/// A finished task of issue `issue` whose PR `pr` is open: its record stays until it merges.
fn finished_task(base: &Path, issue: u32, pr: u32) {
    write_record(base, issue, Act::Ready, Location::Local);
    let dir = record::task_dir(base, &format!("issue-{issue}"));
    let mut rec = record::read(&dir.join("task.json")).unwrap();
    rec.stage = Stage::Finished;
    rec.pr = Some(format!("https://github.com/{REPO}/pull/{pr}"));
    record::write(&dir, &rec).unwrap();
}

struct Playing<'p, 'a> {
    play: &'p Play<'a>,
    issue: u32,
    polls: u32,
    act: Act,
    location: Location,
}

impl Started for Playing<'_, '_> {
    fn poll(&mut self, _out: &mut dyn Write) -> Option<PhaseEnd> {
        if self.polls > 0 {
            self.polls -= 1;
            return None;
        }
        write_record(&self.play.base, self.issue, self.act, self.location);
        self.play
            .events
            .borrow_mut()
            .push(format!("end {} {:?}", self.issue, self.act));
        (self.play.on_end)(self.issue, self.act);
        Some(match self.act {
            Act::Ready => PhaseEnd::Outcome(Outcome::new(Kind::Ready, None, "")),
            Act::UsageLimit(provider) => PhaseEnd::Outcome(Outcome::new(
                Kind::UsageLimit,
                provider.map(str::to_owned),
                "usage limit reached",
            )),
        })
    }
}

impl Steps for Play<'_> {
    fn start_task(
        &self,
        issue: u32,
        phase: Phase,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Box<dyn Started + '_> {
        let location = match phase {
            Phase::Run(location) => location.unwrap_or_default(),
            Phase::Resume | Phase::ShouldFix => {
                let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
                record::read(&dir.join("task.json")).unwrap().location
            }
        };
        let what = match phase {
            Phase::Run(_) => "run",
            Phase::Resume => "resume",
            Phase::ShouldFix => "should-fix",
        };
        self.events
            .borrow_mut()
            .push(format!("start {issue} {what} {}", location.as_str()));
        let (polls, act) = self.next(issue);
        Box::new(Playing {
            play: self,
            issue,
            polls,
            act,
            location,
        })
    }

    // The loop with one task at a time runs each phase to its end here.
    fn run_at(
        &self,
        issue: u32,
        location: Option<Location>,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        let at = location.unwrap_or_default();
        self.events
            .borrow_mut()
            .push(format!("start {issue} run {}", at.as_str()));
        let (_, act) = self.next(issue);
        write_record(&self.base, issue, act, at);
        self.events
            .borrow_mut()
            .push(format!("end {issue} {act:?}"));
        match act {
            Act::Ready => Ok(()),
            Act::UsageLimit(_) => Err(anyhow!("played failure")),
        }
    }

    fn run(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.run_at(issue, None, out, warn)
    }

    fn resume(&self, issue: u32, out: &mut dyn Write, warn: &mut dyn Write) -> Result<()> {
        self.run_at(issue, None, out, warn)
    }

    fn finish(&self, issue: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.events.borrow_mut().push(format!("finish {issue}"));
        let dir = record::task_dir(&self.base, &format!("issue-{issue}"));
        if self.gone_after_finish {
            fs::remove_dir_all(&dir)?;
            return Ok(());
        }
        let mut rec = record::read(&dir.join("task.json"))?;
        rec.stage = Stage::Finished;
        rec.status = Status::Ok;
        rec.pr = Some(format!("https://github.com/{REPO}/pull/{}", 100 + issue));
        record::write(&dir, &rec)
    }

    fn file_findings(
        &self,
        _issue: u32,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Ok(())
    }

    fn fix_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.events.borrow_mut().push(format!("resolve {pr}"));
        Ok(())
    }

    fn fix_check(
        &self,
        pr: u32,
        _run: u64,
        _out: &mut dyn Write,
        _warn: &mut dyn Write,
    ) -> Result<()> {
        Err(anyhow!("unexpected log-fix of PR #{pr}"))
    }

    fn review_pr(&self, pr: u32, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        self.events.borrow_mut().push(format!("review {pr}"));
        Ok(())
    }

    fn rm_merged(&self, _out: &mut dyn Write, _warn: &mut dyn Write) -> Result<()> {
        Ok(())
    }

    fn room(&self) -> Result<()> {
        Ok(())
    }
}

struct Fixture {
    env: common::Env,
    repo_root: PathBuf,
}

impl Fixture {
    fn base(&self) -> PathBuf {
        self.env.base_dir()
    }
}

/// `sandbox_slots` unset (one local), `cloud_slots` as given, and `roles` at the top of
/// `sbxm-task.toml`.
fn fixture(cloud_slots: Option<u32>, roles: &str) -> Fixture {
    let env = common::Env::new();
    if let Some(n) = cloud_slots {
        common::cloud_slot::set_cloud_slots(&env, n);
    }
    let repo_root = env.tmp.path().join("repo");
    fs::create_dir_all(&repo_root).unwrap();
    fs::write(
        repo_root.join("sbxm-task.toml"),
        format!(
            "{roles}[sandbox]\nprofile = \"default\"\n\n[gates]\nsandbox = []\n\n[loop]\n\
             usage_wait = \"10m\"\nusage_wait_total = \"1h\"\n"
        ),
    )
    .unwrap();
    Fixture { env, repo_root }
}

/// Issue `number` labeled ready and sized S, with `extra` labels and `body`.
fn ready(number: u32, extra: &[&str], body: &str) -> Issue {
    let mut labels = vec![READY, "size:S"];
    labels.extend_from_slice(extra);
    open_issue(number, &labels, body)
}

/// GitHub with these issues open, each one's text scripted, and PRs 101 to 109 open.
fn github(issues: Vec<Issue>) -> FakeGitHub {
    let mut gh = FakeGitHub::default()
        .with_user("me")
        .with_default_branch("main")
        .with_branch_head("main", HEAD);
    for n in 1..=9 {
        gh = gh.with_issue_text(issue_text(n)).with_pr(pr(100 + n));
    }
    gh.with_open_issues(issues)
}

fn pr(number: u32) -> PrInfo {
    PrInfo {
        number,
        head_ref: format!("sbxm/pr-{number}"),
        head_oid: HEAD.into(),
        is_cross_repository: false,
        head_repo: String::new(),
        state: PrState::Open,
        title: format!("PR {number}"),
        body: String::new(),
        closing_issues: Vec::new(),
        base_moved: false,
    }
}

/// Queues `name` with `json` in the job queue.
fn queue_job(base: &Path, name: &str, json: &str) {
    let queue = sbxm::task::jobs::queue_dir(base);
    fs::create_dir_all(&queue).unwrap();
    fs::write(queue.join(name), json).unwrap();
}

struct Run {
    result: Result<()>,
    out: String,
}

/// Runs the loop over `issues` (none: the labeled queue) to its stop, its clock moving with its
/// sleeps.
fn run_loop(f: &Fixture, steps: &dyn Steps, gh: &FakeGitHub, issues: &[u32]) -> Run {
    let clock = Cell::new(START);
    let env = Env {
        now: &|| clock.get(),
        sleep: &|d: Duration| clock.set(clock.get() + d.as_secs()),
        stop_requested: &|| false,
    };
    let mut out = Vec::new();
    let result = task_loop::run(
        &f.env.config_dir(),
        &Options {
            repo_root: f.repo_root.clone(),
            issues: issues.to_vec(),
            cap: None,
            repo: Some(REPO.into()),
            wait: false,
        },
        steps,
        gh,
        &Probe,
        &env,
        &mut out,
        &mut Vec::new(),
    );
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
    }
}

// --- a Related clash waits (317(9)) ---

/// Boundary cases 1 and 2: the partner runs as a child, then its task is finished with its PR
/// open; the related issue waits all along, said once, and is never refused.
#[test]
fn an_issue_related_to_a_running_task_waits_said_once_and_its_finished_partner_still_holds_it() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![ready(1, &[], ""), ready(2, &[], "**Related:** #1\n")]);
    let play = Play::new(&f.base()).plays(1, &[(400, Act::Ready)]);

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    let line = "skipped #2 Fix 2: waits for #1 (related)";
    assert_eq!(run.out.matches(line).count(), 1, "{}", run.out);
    assert!(!play.has("start 2"), "{:?}\n{}", play.events(), run.out);
    play.at("finish 1");
    assert!(!run.out.contains("refused"), "{}", run.out);
}

/// Boundary case 3: the partner's record is removed between two passes; the related issue starts
/// at the next pass.
#[test]
fn an_issue_whose_partners_record_is_gone_starts_at_the_next_pass() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![ready(1, &[], ""), ready(2, &[], "**Related:** #1\n")]);
    let play = Play::new(&f.base())
        .plays(1, &[(400, Act::Ready)])
        .gone_after_finish();

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out.contains("skipped #2 Fix 2: waits for #1 (related)"),
        "{}",
        run.out
    );
    assert!(play.at("finish 1") < play.first("start 2 run"));
}

/// Boundary case 4: Related written on the partner's body only is read both ways.
#[test]
fn related_written_only_on_the_partners_body_waits_too() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![ready(1, &[], "**Related:** #2\n"), ready(2, &[], "")]);
    let play = Play::new(&f.base())
        .plays(1, &[(400, Act::Ready)])
        .gone_after_finish();

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out.contains("skipped #2 Fix 2: waits for #1 (related)"),
        "{}",
        run.out
    );
    assert!(play.at("finish 1") < play.first("start 2 run"));
}

/// Boundary case 5: a closed issue with no task record holds nothing.
#[test]
fn related_to_a_closed_issue_with_no_task_does_not_wait() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![ready(2, &[], "**Related:** #9\n")]);
    let play = Play::new(&f.base());

    let run = run_loop(&f, &play, &gh, &[2]);

    run.result.unwrap();
    play.at("finish 2");
    assert!(!run.out.contains("waits for"), "{}", run.out);
}

/// Boundary case 6: two issues related to one running task both wait, then start in queue order
/// once its record is gone.
#[test]
fn two_issues_related_to_one_task_both_wait_then_start_in_queue_order() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![
        ready(1, &[], ""),
        ready(2, &[], "**Related:** #1\n"),
        ready(3, &[], "**Related:** #1\n"),
    ]);
    let play = Play::new(&f.base())
        .plays(1, &[(400, Act::Ready)])
        .gone_after_finish();

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    for n in [2, 3] {
        let line = format!("skipped #{n} Fix {n}: waits for #1 (related)");
        assert!(run.out.contains(&line), "{}", run.out);
    }
    let finished = play.at("finish 1");
    let second = play.first("start 2 run");
    let third = play.first("start 3 run");
    assert!(finished < second && second < third, "{:?}", play.events());
}

/// Boundary case 7: an explicit `--issue` list waits the same way, never refuses.
#[test]
fn a_listed_issue_with_a_related_clash_waits_instead_of_being_refused() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = github(vec![ready(1, &[], ""), ready(2, &[], "**Related:** #1\n")]);
    let play = Play::new(&f.base())
        .plays(1, &[(400, Act::Ready)])
        .gone_after_finish();

    let run = run_loop(&f, &play, &gh, &[1, 2]);

    run.result.unwrap();
    assert!(
        run.out.contains("skipped #2 Fix 2: waits for #1 (related)"),
        "{}",
        run.out
    );
    assert!(!run.out.contains("refused"), "{}", run.out);
    assert!(play.at("finish 1") < play.first("start 2 run"));
}

/// The loop with one task at a time: a partner's task left from before (its PR open) makes the
/// issue wait, listed as skipped, not refused.
#[test]
fn one_task_at_a_time_a_partner_left_from_before_makes_the_issue_wait() {
    let f = fixture(None, CLAUDE_CODEX);
    finished_task(&f.base(), 1, 101);
    let gh = github(vec![ready(2, &[], "**Related:** #1\n")]);
    let play = Play::new(&f.base());

    let run = run_loop(&f, &play, &gh, &[2]);

    run.result.unwrap();
    assert!(
        run.out.contains("skipped #2 Fix 2: waits for #1 (related)"),
        "{}",
        run.out
    );
    assert!(!run.out.contains("refused"), "{}", run.out);
    assert!(!play.has("start 2"), "{:?}", play.events());
}

/// Boundary case 8: `task loop --check` and `sbxm status` say it waits, not a refusal.
#[test]
fn check_and_status_say_a_related_issue_waits() {
    let f = fixture(None, CLAUDE_CODEX);
    finished_task(&f.base(), 1, 101);
    let gh = github(vec![ready(2, &[], "**Related:** #1\n")]);

    let opts = Options {
        repo_root: f.repo_root.clone(),
        issues: vec![2],
        cap: None,
        repo: Some(REPO.into()),
        wait: false,
    };
    let mut out = Vec::new();
    task_loop::check(&f.env.config_dir(), &opts, &gh, &Probe, &mut out).unwrap();
    let check = String::from_utf8(out).unwrap();
    assert!(
        check.contains("#2 Fix 2: skip: waits for #1 (related)\n"),
        "{check}"
    );

    let source = LoopSource {
        github: &gh,
        repo: Ok(REPO.into()),
    };
    let env = status::Env {
        now: Box::new(|| START),
        sleep: Box::new(|_| {}),
        terminal: false,
        width: 120,
        iterations: None,
        monotonic: Box::new(|| Duration::ZERO),
        slots: None,
        cloud_slots: None,
        disk: sbxm::status::footer::Disk {
            free_space: |_| Ok(40 * 1024 * 1024 * 1024),
            same_drive: |_, _| true,
            state_dir: Some(PathBuf::from("/state")),
            min_free_disk: sbxm::disk::DEFAULT_MIN_FREE,
        },
    };
    let mut out = Vec::new();
    status::run_with(
        &f.base(),
        &FakeBackend::default(),
        &Probe,
        &env,
        None,
        Some(&source),
        &mut out,
    )
    .unwrap();
    let status = String::from_utf8(out).unwrap();
    assert!(
        status.contains("#2: Fix 2 (size S) — blocked: waits for #1 (related)"),
        "{status}"
    );
}

// --- the usage pause holds per provider (317(10)) ---

/// PR 150, opened by an earlier task of issue 50, open on GitHub.
fn pr_150(f: &Fixture, gh: FakeGitHub) -> FakeGitHub {
    finished_task(&f.base(), 50, 150);
    gh.with_pr(pr(150))
}

/// Boundary cases 9 and 12: `anthropic` limited; a review (codex) starts meanwhile, an issue
/// task (claude and codex) waits and starts at the pass after the limit clears.
#[test]
fn an_anthropic_limit_lets_a_codex_review_start_and_holds_an_issue_task_until_it_clears() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = pr_150(&f, github(vec![ready(1, &["run-cloud"], "")]));
    let base = f.base();
    let play = Play::new(&base)
        .plays(
            1,
            &[(0, Act::UsageLimit(Some("anthropic"))), (400, Act::Ready)],
        )
        .on_end(|issue, act| {
            if issue == 1 && matches!(act, Act::UsageLimit(_)) {
                queue_job(&base, "review-150.json", r#"{"kind": "review", "pr": 150}"#);
                gh.set_open_issues(vec![ready(1, &["run-cloud"], ""), ready(2, &[], "")]);
            }
        });

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out
            .contains("no new task on anthropic starts while #1 (Fix 1) waits out its usage limit"),
        "{}",
        run.out
    );
    let resumed = play.at("start 1 resume cloud");
    assert!(play.at("review 150") < resumed, "{:?}", play.events());
    // It starts once the limited child's next phase started, not when that phase ends.
    let second = play.first("start 2 run");
    assert!(resumed < second, "{:?}", play.events());
    assert!(second < play.at("end 1 Ready"), "{:?}", play.events());
}

/// Boundary case 10: `openai` limited; an issue task waits (its reviewer is codex), and a job on
/// `anthropic` only (a review by a claude reviewer) starts.
#[test]
fn an_openai_limit_holds_an_issue_task_and_lets_a_claude_review_start() {
    let f = fixture(Some(2), CODEX_CLAUDE);
    let gh = pr_150(&f, github(vec![ready(1, &["run-cloud"], "")]));
    let base = f.base();
    let play = Play::new(&base)
        .plays(
            1,
            &[(0, Act::UsageLimit(Some("openai"))), (400, Act::Ready)],
        )
        .on_end(|issue, act| {
            if issue == 1 && matches!(act, Act::UsageLimit(_)) {
                queue_job(&base, "review-150.json", r#"{"kind": "review", "pr": 150}"#);
                gh.set_open_issues(vec![ready(1, &["run-cloud"], ""), ready(2, &[], "")]);
            }
        });

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out
            .contains("no new task on openai starts while #1 (Fix 1) waits out its usage limit"),
        "{}",
        run.out
    );
    let resumed = play.at("start 1 resume cloud");
    assert!(play.at("review 150") < resumed, "{:?}", play.events());
    assert!(resumed < play.first("start 2 run"), "{:?}", play.events());
}

/// Boundary case 11: two children limited on two providers; each hold applies, and the one that
/// clears first lifts only its own.
#[test]
fn two_limits_on_two_providers_each_hold_and_clearing_one_lifts_only_its_own() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = pr_150(
        &f,
        github(vec![
            ready(1, &["run-cloud"], ""),
            ready(3, &["run-cloud"], ""),
        ]),
    );
    let base = f.base();
    // #3's openai limit first; #1's anthropic limit about 400 s later.
    let play = Play::new(&base)
        .plays(
            1,
            &[(200, Act::UsageLimit(Some("anthropic"))), (400, Act::Ready)],
        )
        .plays(
            3,
            &[(0, Act::UsageLimit(Some("openai"))), (400, Act::Ready)],
        )
        .on_end(|issue, act| {
            if issue == 3 && matches!(act, Act::UsageLimit(_)) {
                queue_job(&base, "review-150.json", r#"{"kind": "review", "pr": 150}"#);
                gh.set_open_issues(vec![
                    ready(1, &["run-cloud"], ""),
                    ready(2, &[], ""),
                    ready(3, &["run-cloud"], ""),
                ]);
            }
        });

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out.contains(
            "job review #150: waits for #3's usage limit on openai to clear; tried again at a \
             later pass"
        ),
        "{}",
        run.out
    );
    let three = play.at("start 3 resume cloud");
    let one = play.at("start 1 resume cloud");
    let review = play.at("review 150");
    let second = play.first("start 2 run");
    assert!(three < review && review < one, "{:?}", play.events());
    assert!(one < second, "{:?}", play.events());
}

/// Boundary case 13: a limit whose provider isn't known holds every new task and job.
#[test]
fn a_limit_with_no_provider_holds_every_task_and_job() {
    unknown_provider_holds_everything(None);
}

/// Boundary case 14: a provider name sbxm doesn't know holds them all too.
#[test]
fn a_limit_of_an_unknown_provider_holds_every_task_and_job() {
    unknown_provider_holds_everything(Some("mistral"));
}

fn unknown_provider_holds_everything(provider: Option<&'static str>) {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = pr_150(&f, github(vec![ready(1, &["run-cloud"], "")]));
    let base = f.base();
    let play = Play::new(&base)
        .plays(1, &[(0, Act::UsageLimit(provider)), (400, Act::Ready)])
        .on_end(|issue, act| {
            if issue == 1 && matches!(act, Act::UsageLimit(_)) {
                queue_job(&base, "review-150.json", r#"{"kind": "review", "pr": 150}"#);
                gh.set_open_issues(vec![ready(1, &["run-cloud"], ""), ready(2, &[], "")]);
            }
        });

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    assert!(
        run.out
            .contains("no new task starts while #1 (Fix 1) waits out a usage limit"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains(
            "job review #150: waits for #1's usage limit to clear; tried again at a later pass"
        ),
        "{}",
        run.out
    );
    let resumed = play.at("start 1 resume cloud");
    assert!(resumed < play.at("review 150"), "{:?}", play.events());
    assert!(resumed < play.first("start 2 run"), "{:?}", play.events());
}

/// Boundary case 15: no child limited, nothing waits on a usage limit.
#[test]
fn with_no_child_limited_nothing_waits_for_a_usage_limit() {
    let f = fixture(Some(2), CLAUDE_CODEX);
    let gh = pr_150(&f, github(vec![ready(1, &["run-cloud"], "")]));
    let base = f.base();
    let play = Play::new(&base)
        .plays(1, &[(0, Act::Ready)])
        .on_end(|issue, _| {
            if issue == 1 {
                queue_job(&base, "review-150.json", r#"{"kind": "review", "pr": 150}"#);
            }
        });

    let run = run_loop(&f, &play, &gh, &[]);

    run.result.unwrap();
    play.at("review 150");
    assert!(!run.out.contains("usage limit"), "{}", run.out);
}
