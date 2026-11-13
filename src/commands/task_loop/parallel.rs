//! Several tasks at once (issue 615, milestone 5 slice 4; decisions 317(4), (6), 321): with
//! `[resources] cloud_slots` set, the loop keeps up to `sandbox_slots` local and `cloud_slots`
//! cloud children running, each an issue task end to end as [`Loop::task`] runs one, and reacts to
//! each child's end as it happens. With `cloud_slots` unset the loop is [`super::work`], one task
//! at a time, as before (decision 321(5)).
//!
//! - **Routing** (317(4)): the next issue goes to the first free slot, local first, then cloud;
//!   `run-local` and `run-cloud` pin it. An issue whose place is full keeps its place in the
//!   queue, and the first issue in order that fits a free slot starts instead.
//! - **The loop's own steps never wait for a slot:** a resolve, a log-fix or a queued job whose
//!   slot a child holds is postponed to a later pass; once free, they start as job children
//!   ([`super::job_children`], issue 633) before any new issue.
//! - **Pauses:** no new task while any child waits out a usage limit, or while the PRs awaiting
//!   the user plus the running task children and `spec` job children reach [`PR_CAP`] (317(6)).
//! - **Records:** one log row per task as it ends, in finish order, with its slot; children are
//!   finished one at a time, in this process.
//! - **Stop:** a stop request, or a stop that needs the user (a lapsed sign-in, two parked in a
//!   row), starts nothing new; the running children go on to their ends, then the loop stops.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::{
    Cause, End, Halt, Held, Loop, LoopLock, Options, PAUSE_POLL, PR_CAP, Phase, RESUMED_HOOK,
    Retry, Started, first_line, jobs as queued, needs_user, note, sign_in_stop,
};
use crate::backend::Location;
use crate::task::outcome;
use crate::task::queue::{self, Outcome, READY_LABEL, Report, Row, Skipped};
use crate::task::record::{self, Stage};
use crate::task::{jobs, location, lock};

/// How often the running children are looked at.
pub const TICK: Duration = Duration::from_secs(2);

/// How many children may run in each place at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slots {
    pub local: u32,
    pub cloud: u32,
}

impl Slots {
    fn of(self, location: Location) -> u32 {
        match location {
            Location::Local => self.local,
            Location::Cloud => self.cloud,
        }
    }
}

/// Which places have a free slot now.
#[derive(Debug, Clone, Copy)]
pub(super) struct Free {
    pub local: bool,
    pub cloud: bool,
}

impl Free {
    /// Where an issue whose labels pin `pinned` starts now: its pinned place if free, else for an
    /// unpinned one the first free slot, local first; `None` when it doesn't fit.
    pub fn place(self, pinned: Option<Location>) -> Option<Location> {
        match pinned {
            Some(Location::Local) => self.local.then_some(Location::Local),
            Some(Location::Cloud) => self.cloud.then_some(Location::Cloud),
            None if self.local => Some(Location::Local),
            None => self.cloud.then_some(Location::Cloud),
        }
    }

    fn any(self) -> bool {
        self.local || self.cloud
    }
}

/// A task the loop runs as a child, from its start to its end: what [`Loop::task`] keeps on its
/// stack, kept here between passes.
struct Live<'a> {
    issue: u32,
    title: String,
    size: Option<String>,
    started: u64,
    stops: Vec<(String, u64)>,
    /// Where it runs.
    slot: Location,
    /// Its issue still has the loop's label, taken off once its record exists.
    label: bool,
    failures: u32,
    should_fix_ran: bool,
    /// Whether a phase of it ran.
    ran: bool,
    /// Whether its running phase is a resume.
    resuming: bool,
    /// The failure its resume is for, and since when: a stop of its own row.
    pending: Option<(String, u64)>,
    /// Its running phase; `None` while it waits to start the next one.
    phase: Option<Box<dyn Started + 'a>>,
    /// While it waits out a usage limit: when its resume may start, and since when it waits.
    usage: Option<(u64, u64)>,
    /// Its usage-limit waits in a row.
    usage_waited: Duration,
    /// Its last phase ended on a usage limit: no new task starts until it clears (answer 4).
    limited: bool,
    /// Since when it waits for room on the disk.
    room_since: Option<u64>,
    /// For a task tried again: how, whether it was cut off, and the resume it owed.
    retried: Option<(Retry, bool, Option<serde_json::Value>)>,
    idle_before: Option<u64>,
    /// For a queued `run` job's task (issue 633): its job, whose result its end writes instead
    /// of a task's row.
    job: Option<queued::JobFile>,
}

impl Live<'_> {
    fn held(&self, retry: Retry, cut_off: bool) -> Held {
        Held {
            issue: self.issue,
            title: self.title.clone(),
            size: self.size.clone(),
            started: self.started,
            stops: self.stops.clone(),
            retry,
            cut_off,
        }
    }
}

/// A task tried again: running from its next phase, or ended at once, with what it was.
type Again<'a> = std::result::Result<Live<'a>, (Held, std::result::Result<End, Halt>)>;

/// What the loop's pass keeps between passes.
struct Pass {
    tried: HashSet<u32>,
    refused: Vec<Skipped>,
    held: Vec<Held>,
    due: VecDeque<Held>,
    tasks_run: u32,
    parked_in_row: u32,
    /// When the last child ended with none left running: the next start's idle time.
    idle_from: Option<u64>,
    /// The first stop that needs the user, returned once the running children end.
    halt: Option<anyhow::Error>,
    /// When the loop's own steps (resolves, runs, jobs, starts) ran last.
    checked: Option<u64>,
    /// Whether a `--wait` loop said it waits on an empty queue since its last task.
    idle: bool,
}

impl<'a> Loop<'a> {
    /// Whether no slot of `location` is free for a step of the loop's own (issue 615): its
    /// children hold them all. `None` is where `[sandbox] location` runs it. Always `false` for
    /// a loop that runs one task at a time: no child runs while its own steps do.
    pub(super) fn slot_taken(&self, location: Option<Location>) -> bool {
        let Some(slots) = self.slots else {
            return false;
        };
        let at = location.unwrap_or(self.default_location);
        let taken = self.run_jobs.iter().filter(|r| r.slot == at).count();
        let held = self.children.iter().filter(|&&l| l == at).count() + self.jobs_in(at) + taken;
        held >= slots.of(at) as usize
    }

    /// The places with a free slot for a new child.
    fn free(&self) -> Free {
        Free {
            local: !self.slot_taken(Some(Location::Local)),
            cloud: !self.slot_taken(Some(Location::Cloud)),
        }
    }

    /// Whether a running task is to stop short at its next step: a stop request, or a stop of
    /// the loop's that waits for the running children to end.
    pub(super) fn stopping(&self) -> bool {
        self.draining || (self.env.stop_requested)()
    }

    /// Starts `t`'s next phase, or leaves it waiting for room on the disk or out a usage limit.
    /// `Some` when it ended without one.
    fn next_phase(
        &mut self,
        t: &mut Live<'a>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<std::result::Result<End, Halt>>> {
        let now = self.now();
        if let Some((until, since)) = t.usage {
            if self.stopping() {
                t.usage = None;
                t.stops
                    .push(("usage limit".to_owned(), now.saturating_sub(since)));
                return Ok(Some(Ok(self.stopped(t.issue, out)?)));
            }
            if now < until {
                return Ok(None);
            }
            t.usage = None;
            t.stops
                .push(("usage limit".to_owned(), now.saturating_sub(since)));
        }
        if let Err(e) = self.steps.room_for(Some(t.slot)) {
            if self.stopping() {
                return Ok(Some(if t.ran {
                    Ok(self.stopped(t.issue, out)?)
                } else {
                    Err(Halt::Stopped)
                }));
            }
            if t.room_since.is_none() {
                t.room_since = Some(now);
                writeln!(
                    out,
                    "[issue-{} {}] paused: {e:#}; checking again every {}",
                    t.issue,
                    t.slot.as_str(),
                    queue::span(PAUSE_POLL.as_secs())
                )?;
            }
            return Ok(None);
        }
        if let Some(since) = t.room_since.take() {
            let paused = now.saturating_sub(since);
            if paused > 0 {
                t.stops.push(("disk".to_owned(), paused));
            }
        }
        if let Some((cause, since)) = t.pending.take() {
            t.stops.push((cause, now.saturating_sub(since)));
        }
        let issue = t.issue;
        let resuming = self.record(issue).is_some();
        let should_fix = self.should_fix_due(issue);
        t.should_fix_ran |= should_fix;
        let phase = if should_fix {
            Phase::ShouldFix
        } else if resuming {
            Phase::Resume
        } else {
            Phase::Run(Some(t.slot))
        };
        let steps = self.steps;
        let started = if t.label && phase == Phase::Run(Some(t.slot)) {
            let (started, removed) =
                self.run_unlabeling(issue, || steps.start_task(issue, phase, out, warn));
            if self.unlabeled(issue, removed, warn)? {
                t.label = false;
            }
            started
        } else {
            steps.start_task(issue, phase, out, warn)
        };
        t.resuming = resuming;
        t.phase = Some(started);
        Ok(None)
    }

    /// Looks at `t` once: its phase's end, its next phase started, or nothing yet. `Some` when
    /// the task ended.
    fn supervise(
        &mut self,
        t: &mut Live<'a>,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<std::result::Result<End, Halt>>> {
        let Some(phase) = t.phase.as_mut() else {
            return self.next_phase(t, out, warn);
        };
        let polled = phase.poll(out);
        // Its label off as soon as its record exists (decision 4), while its first phase runs.
        if t.label && self.record(t.issue).is_some() && self.unlabeled(t.issue, None, warn)? {
            t.label = false;
        }
        let Some(ended) = polled else {
            return Ok(None);
        };
        t.phase = None;
        match self.phase_ended(t, ended, out, warn)? {
            Some(end) => Ok(Some(end)),
            None => self.next_phase(t, out, warn),
        }
    }

    /// What `t`'s phase's end means, as [`Loop::task`] reads it after each phase: `None` to go on
    /// with its next phase (which may wait), `Some` when the task ended.
    fn phase_ended(
        &mut self,
        t: &mut Live<'a>,
        ended: super::PhaseEnd,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Option<std::result::Result<End, Halt>>> {
        let issue = t.issue;
        let ended = ended.masked();
        let kind = ended.kind();
        t.limited = kind == outcome::Kind::UsageLimit;
        if kind == outcome::Kind::Stopped {
            if !t.ran {
                return Ok(Some(Err(Halt::Stopped)));
            }
            return Ok(Some(Ok(self.stopped(issue, out)?)));
        }
        t.ran = true;
        let ready = self.record(issue).filter(|r| r.stage == Stage::Ready);
        if kind == outcome::Kind::Ready && ready.is_some() {
            t.usage_waited = Duration::ZERO;
            if !t.should_fix_ran && self.should_fix_due(issue) {
                return Ok(None);
            }
            // One finish at a time, here (answer 7).
            return Ok(Some(
                self.finish(issue, None, out, warn).map_err(Halt::Error),
            ));
        }
        let e = ended.error();
        if t.resuming && kind == outcome::Kind::Held {
            writeln!(
                out,
                "{}",
                super::held_line(&self.issue_named(issue), "resume", &e)
            )?;
            return Ok(Some(Ok(End::Held(Retry::Resume {
                failures: t.failures,
            }))));
        }
        if kind == outcome::Kind::MissingSecret {
            return Ok(Some(Err(Halt::Error(needs_user(Cause::Secret, e)))));
        }
        if kind == outcome::Kind::SignedOut {
            t.stops.push(("sign-in".to_owned(), 0));
            return Ok(Some(Err(Halt::SignIn(e))));
        }
        if kind == outcome::Kind::UsageLimit {
            let total = self.settings.usage_wait_total;
            if t.usage_waited >= total {
                return Ok(Some(Err(Halt::UsageLimit(total))));
            }
            let wait = self.settings.usage_wait;
            writeln!(
                out,
                "{issue_named}: usage limit; waiting {} before `task resume` ({} of {} waited in a \
                 row); no new task starts meanwhile",
                queue::span(wait.as_secs()),
                queue::span(t.usage_waited.as_secs()),
                queue::span(total.as_secs()),
                issue_named = self.issue_named(issue)
            )?;
            let now = self.now();
            t.usage = Some((now + wait.as_secs(), now));
            t.usage_waited += wait;
            return Ok(None);
        }
        t.usage_waited = Duration::ZERO;
        let environment = kind == outcome::Kind::Environment;
        let said = matches!(ended, super::PhaseEnd::Outcome(_));
        if self.record(issue).is_none() && said && !environment {
            return Ok(Some(Ok(End::Refused(format!("{e:#}")))));
        }
        t.failures += 1;
        let cause = first_line(&format!("{e:#}"));
        if t.failures >= 2 {
            t.stops.push((cause, 0));
            let stage = self
                .record(issue)
                .map_or("no record".to_owned(), |r| r.stage.name().to_owned());
            writeln!(
                out,
                "{issue_named}: parked at {stage} after a second failure: {e:#}; see `sbxm task \
                 status --issue {issue}`",
                issue_named = self.issue_named(issue)
            )?;
            return Ok(Some(Ok(End::Parked {
                ready: None,
                outcome: format!("parked at {stage}"),
            })));
        }
        writeln!(
            out,
            "{issue_named}: stopped ({e:#}); resuming it once",
            issue_named = self.issue_named(issue)
        )?;
        t.pending = Some((cause, self.now()));
        if self.stopping() {
            return Ok(Some(Ok(self.stopped(issue, out)?)));
        }
        Ok(None)
    }

    /// A task tried again (a held or cut-off one), as [`Loop::again`] decides it from its record
    /// now: ended at once (`Err`), or running from its next phase.
    fn again_live(
        &mut self,
        again: Held,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<Again<'a>> {
        let issue = again.issue;
        let Some(rec) = self.record(issue) else {
            let line = format!(
                "{issue_named}: its task was removed by a command run by hand; the loop leaves it",
                issue_named = self.issue_named(issue)
            );
            return Ok(Err((again, Ok(End::Taken(line)))));
        };
        if rec.pr.is_some() || rec.stage == Stage::Finished {
            let line = super::finished_by_hand(&self.issue_named(issue), rec.pr.as_deref());
            return Ok(Err((again, Ok(End::Taken(line)))));
        }
        let failures = match again.retry {
            Retry::Finish { .. } => 0,
            Retry::Resume { failures } => failures,
        };
        if rec.stage == Stage::Ready && !self.should_fix_due(issue) {
            let ready = match again.retry {
                Retry::Finish { ready } => Some(ready),
                Retry::Resume { .. } => None,
            };
            let end = self.finish(issue, ready, out, warn).map_err(Halt::Error);
            return Ok(Err((again, end)));
        }
        Ok(Ok(Live {
            issue,
            title: again.title,
            size: again.size,
            started: again.started,
            stops: again.stops,
            slot: rec.location,
            label: false,
            failures,
            should_fix_ran: false,
            ran: false,
            resuming: true,
            pending: None,
            phase: None,
            usage: None,
            usage_waited: Duration::ZERO,
            limited: false,
            room_since: None,
            retried: Some((again.retry, again.cut_off, None)),
            idle_before: None,
            job: None,
        }))
    }
}

/// The loop with several children (issue 615): from its first task to its stop; returns the
/// stop's message. `due`: the tasks a dead loop was running, resumed first (decision 310).
pub(super) fn work<'a>(
    lp: &mut Loop<'a>,
    lock: &mut LoopLock,
    opts: &Options,
    due: VecDeque<Held>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<String> {
    let mut pass = Pass {
        tried: HashSet::new(),
        refused: Vec::new(),
        held: Vec::new(),
        due,
        tasks_run: 0,
        parked_in_row: 0,
        idle_from: None,
        halt: None,
        checked: None,
        idle: false,
    };
    let mut live: Vec<Live<'a>> = Vec::new();
    let result = run(lp, lock, opts, &mut pass, &mut live, out, warn);
    // An error of the loop's own with children running: they end the phase they run as they
    // would at a stop, and the loop stops with them.
    match result {
        Ok(stop) => Ok(stop),
        Err(e) if live.is_empty() && lp.jobs.is_empty() => Err(e),
        Err(e) => {
            pass.halt.get_or_insert(e);
            lp.draining = true;
            run(lp, lock, opts, &mut pass, &mut live, out, warn)
        }
    }
}

/// [`work`]'s passes, until the loop stops.
#[allow(clippy::too_many_arguments)]
fn run<'a>(
    lp: &mut Loop<'a>,
    lock: &mut LoopLock,
    opts: &Options,
    pass: &mut Pass,
    live: &mut Vec<Live<'a>>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<String> {
    let left = |pass: &Pass, out: &mut dyn Write| -> Result<()> {
        for h in pass.due.iter().chain(&pass.held) {
            writeln!(out, "{}", h.left())?;
        }
        Ok(())
    };
    loop {
        // Each running child looked at, in the order they started; each end acted on at once.
        let mut ended_any = false;
        let mut i = 0;
        while i < live.len() {
            sync(lp, live);
            let ended = lp.supervise(&mut live[i], out, warn)?;
            match ended {
                None => i += 1,
                Some(end) => {
                    let t = live.remove(i);
                    sync(lp, live);
                    ended_any = true;
                    task_ended(lp, lock, pass, t, end, live.is_empty(), out, warn)?;
                }
            }
        }
        sync(lp, live);
        // The job children too (issue 633), each end acted on at once.
        let (jobs_ended, halt) = lp.supervise_jobs(out, warn)?;
        ended_any |= jobs_ended;
        if let Some(e) = halt {
            lp.draining = true;
            pass.halt.get_or_insert(e);
        }
        let busy = !live.is_empty() || !lp.jobs.is_empty();
        if !busy {
            if let Some(e) = pass.halt.take() {
                left(pass, out)?;
                return Err(e);
            }
            if (lp.env.stop_requested)() {
                writeln!(out, "stopped on request")?;
                left(pass, out)?;
                return Ok("stopped on request".to_owned());
            }
            if opts.cap.is_some_and(|cap| pass.tasks_run >= cap) {
                let stop = format!("--cap {} reached", opts.cap.unwrap_or_default());
                writeln!(out, "{stop}")?;
                left(pass, out)?;
                return Ok(stop);
            }
        }
        let stopping = lp.stopping()
            || opts
                .cap
                .is_some_and(|cap| pass.tasks_run + tasks(live) >= cap);
        // The loop's own steps and new starts: at the first pass, whenever a child ended or none
        // runs, and every [`PAUSE_POLL`] meanwhile.
        let now = lp.now();
        let due_check = !busy
            || ended_any
            || pass
                .checked
                .is_none_or(|at| now.saturating_sub(at) >= PAUSE_POLL.as_secs());
        if due_check && !stopping {
            pass.checked = Some(now);
            if let Some(stop) = steps_and_starts(lp, lock, opts, pass, live, out, warn)? {
                return Ok(stop);
            }
            continue_or_sleep(lp, live, false);
            continue;
        }
        continue_or_sleep(lp, live, ended_any);
    }
}

/// How many of the running task children are the loop's own tasks: a queued `run` job's isn't
/// one, for `--cap` (issue 633).
fn tasks(live: &[Live]) -> u32 {
    live.iter().filter(|t| t.job.is_none()).count() as u32
}

/// What the loop's own steps read of the running children: their slots and a usage hold.
fn sync(lp: &mut Loop, live: &[Live]) {
    lp.children = live.iter().map(|t| t.slot).collect();
    lp.usage_hold = live.iter().find(|t| t.limited).map(|t| t.issue);
}

/// Whether a new task may start now, checked before every start, a held or owed task's too
/// (issue 615): not while the loop stops, a child waits out a usage limit (answer 4), `--cap`
/// is reached, or the PRs awaiting the user plus the running children reach [`PR_CAP`]
/// (317(6)); the stop is asked again after those reads. Its slot is checked by the caller.
fn may_start(
    lp: &mut Loop,
    opts: &Options,
    pass: &Pass,
    live: &[Live],
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<bool> {
    if lp.stopping() {
        return Ok(false);
    }
    if let Some(t) = live.iter().find(|t| t.limited) {
        let line = format!(
            "no new task starts while {} waits out a usage limit",
            lp.issue_named(t.issue)
        );
        lp.say_once(out, line)?;
        return Ok(false);
    }
    if opts
        .cap
        .is_some_and(|cap| pass.tasks_run + tasks(live) >= cap)
    {
        return Ok(false);
    }
    // A stop asked during the reads that counted the awaiting PRs: nothing starts. A job child
    // that may open a PR counts with the tasks (issue 633, answer 5).
    let running = live.len() + lp.jobs.iter().filter(|j| j.opens_pr()).count();
    Ok(lp.awaiting_room(running, out, warn)? && !lp.stopping())
}

/// Sleeps one [`TICK`] while children run and none ended in this pass.
fn continue_or_sleep(lp: &Loop, live: &[Live], ended_any: bool) {
    if (!live.is_empty() || !lp.jobs.is_empty()) && !ended_any {
        (lp.env.sleep)(TICK);
    }
}

/// The loop's own steps between tasks, then new children while slots are free and work is
/// ready. `Some` is the loop's stop: nothing runs and nothing is left to start.
#[allow(clippy::too_many_arguments)]
fn steps_and_starts<'a>(
    lp: &mut Loop<'a>,
    lock: &mut LoopLock,
    opts: &Options,
    pass: &mut Pass,
    live: &mut Vec<Live<'a>>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<Option<String>> {
    lp.clean_merged(out, warn)?;
    let go_on = lp.resolve_conflicts(out, warn)? && {
        lp.check_runs(out, warn)?;
        lp.recheck_waiting(out, warn)?;
        let drained = lp.drain_jobs(out, warn);
        // The `run` jobs it took start as task children, whatever came after them.
        start_run_jobs(lp, lock, pass, live, out, warn)?;
        drained? && {
            lp.notify_awaiting(out, warn)?;
            true
        }
    };
    if !go_on {
        return Ok(None);
    }
    // The PRs awaiting the user (317(6)): with no child running, waited for as before; with
    // some, counted with them before each start ([`Loop::awaiting_room`]).
    if live.is_empty() && lp.jobs.is_empty() && !lp.wait_for_prs(out, warn)? {
        return Ok(None);
    }
    let mut started_any = false;
    // The tasks tried again first: held or cut off, each in the slot its record names.
    let mut later = VecDeque::new();
    while let Some(again) = pass.due.pop_front() {
        let fits = lp
            .record(again.issue)
            .is_none_or(|rec| !lp.slot_taken(Some(rec.location)));
        if !fits || !may_start(lp, opts, pass, live, out, warn)? {
            later.push_back(again);
            continue;
        }
        if start_again(lp, lock, pass, live, again, out, warn)? {
            started_any = true;
        }
        sync(lp, live);
    }
    pass.due = later;
    let mut open = None;
    while lp.free().any() && may_start(lp, opts, pass, live, out, warn)? {
        let open = match &open {
            Some(open) => open,
            None => open.insert(lp.github.issues_open(&lp.repo)?),
        };
        let free = lp.free();
        let (next, waiting) = lp.next_fitting(
            open,
            &opts.issues,
            &mut pass.tried,
            &mut pass.refused,
            Some(free),
            out,
        )?;
        let Some(issue) = next else {
            if !live.is_empty() || !lp.jobs.is_empty() || started_any || !pass.due.is_empty() {
                break;
            }
            if opts.wait {
                if !pass.idle {
                    writeln!(
                        out,
                        "the queue is empty; checking again every {} for a job file or a \
                         labelled issue",
                        queue::span(PAUSE_POLL.as_secs())
                    )?;
                    pass.idle = true;
                }
                (lp.env.sleep)(PAUSE_POLL);
                return Ok(None);
            }
            let skipped: Vec<Skipped> = pass.refused.iter().chain(&waiting).cloned().collect();
            let stop = if opts.issues.is_empty() {
                "the queue is empty; the loop stops"
            } else {
                "every named issue was taken or skipped; the loop stops"
            };
            writeln!(out, "{stop}")?;
            if opts.issues.is_empty() {
                write!(
                    out,
                    "{}",
                    queue::report(&Report {
                        open,
                        skipped: &skipped,
                    })
                )?;
            }
            for h in pass.due.iter().chain(&pass.held) {
                writeln!(out, "{}", h.left())?;
            }
            return Ok(Some(stop.to_owned()));
        };
        // A stop asked during the reads that picked it: it isn't started.
        if lp.stopping() {
            break;
        }
        pass.idle = false;
        let found = open.iter().find(|i| i.number == issue);
        let title = found.map_or_else(|| format!("issue {issue}"), |i| i.title.clone());
        let size = found
            .and_then(|i| queue::size(&i.labels))
            .map(str::to_owned);
        let labeled = found.is_some_and(|i| i.labels.iter().any(|l| l == READY_LABEL));
        let slot = lp
            .locations
            .get(&issue)
            .copied()
            .flatten()
            .unwrap_or(lp.default_location);
        let started = lp.now();
        writeln!(
            out,
            "task {}: #{issue} {title} (size {}), {}",
            pass.tasks_run + tasks(live) + 1,
            size.as_deref().unwrap_or("unknown"),
            slot.as_str()
        )?;
        note(lock, lp, warn, |p| {
            for skipped in pass.refused.iter().chain(&waiting) {
                p.set(skipped.issue, Outcome::Skipped);
            }
            p.start(issue);
        })?;
        let mut t = Live {
            issue,
            title,
            size,
            started,
            stops: Vec::new(),
            slot,
            label: labeled,
            failures: 0,
            should_fix_ran: false,
            ran: false,
            resuming: false,
            pending: None,
            phase: None,
            usage: None,
            usage_waited: Duration::ZERO,
            limited: false,
            room_since: None,
            retried: None,
            idle_before: live
                .is_empty()
                .then(|| pass.idle_from.map(|end| started.saturating_sub(end)))
                .flatten(),
            job: None,
        };
        started_any = true;
        match lp.next_phase(&mut t, out, warn)? {
            None => live.push(t),
            Some(end) => task_ended(lp, lock, pass, t, end, live.is_empty(), out, warn)?,
        }
        sync(lp, live);
    }
    Ok(None)
}

/// The queued `run` jobs taken in this pass (issue 633), each started as a task child in the
/// slot its labels chose, as the loop starts an issue it picked itself; its end writes the job's
/// result ([`task_ended`]).
fn start_run_jobs<'a>(
    lp: &mut Loop<'a>,
    lock: &mut LoopLock,
    pass: &mut Pass,
    live: &mut Vec<Live<'a>>,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    for run in std::mem::take(&mut lp.run_jobs) {
        let mut t = Live {
            issue: run.issue,
            title: run.title,
            size: run.size,
            started: lp.now(),
            stops: Vec::new(),
            slot: run.slot,
            label: run.labeled,
            failures: 0,
            should_fix_ran: false,
            ran: false,
            resuming: false,
            pending: None,
            phase: None,
            usage: None,
            usage_waited: Duration::ZERO,
            limited: false,
            room_since: None,
            retried: None,
            idle_before: None,
            job: Some(run.file),
        };
        match lp.next_phase(&mut t, out, warn)? {
            None => live.push(t),
            Some(end) => task_ended(lp, lock, pass, t, end, live.is_empty(), out, warn)?,
        }
        sync(lp, live);
    }
    Ok(())
}

/// A held or cut-off task tried again: started, or ended at once. `true` when it started.
#[allow(clippy::too_many_arguments)]
fn start_again<'a>(
    lp: &mut Loop<'a>,
    lock: &mut LoopLock,
    pass: &mut Pass,
    live: &mut Vec<Live<'a>>,
    again: Held,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<bool> {
    let issue = again.issue;
    let owed = again
        .cut_off
        .then(|| lp.record(issue)?.hooks.get(RESUMED_HOOK).cloned())
        .flatten();
    if again.cut_off
        && let Err(e) = lp.mark_resume(issue, serde_json::Value::Bool(true))
    {
        let Some(in_use) = e.downcast_ref::<lock::InUse>() else {
            return Err(e);
        };
        writeln!(
            out,
            "{}: its resume is skipped: {}; the loop leaves it to that command",
            crate::title::named(&format!("#{issue}"), &again.title),
            in_use.describe()
        )?;
        return Ok(false);
    }
    writeln!(
        out,
        "task {}: #{issue} {} (size {}), tried again",
        pass.tasks_run + tasks(live) + 1,
        again.title,
        again.size.as_deref().unwrap_or("unknown")
    )?;
    note(lock, lp, warn, |p| p.start(issue))?;
    let (retry, cut_off) = (again.retry, again.cut_off);
    match lp.again_live(again, out, warn)? {
        Ok(mut t) => {
            t.retried = Some((retry, cut_off, owed));
            match lp.next_phase(&mut t, out, warn)? {
                None => {
                    live.push(t);
                    Ok(true)
                }
                Some(end) => {
                    task_ended(lp, lock, pass, t, end, live.is_empty(), out, warn)?;
                    Ok(false)
                }
            }
        }
        Err((again, end)) => {
            let t = Live {
                issue,
                title: again.title,
                size: again.size,
                started: again.started,
                stops: again.stops,
                slot: lp.slot_of(issue).unwrap_or(lp.default_location),
                label: false,
                failures: 0,
                should_fix_ran: false,
                ran: true,
                resuming: false,
                pending: None,
                phase: None,
                usage: None,
                usage_waited: Duration::ZERO,
                limited: false,
                room_since: None,
                retried: Some((retry, cut_off, owed)),
                idle_before: None,
                job: None,
            };
            task_ended(lp, lock, pass, t, end, live.is_empty(), out, warn)?;
            Ok(false)
        }
    }
}

/// A task's end, acted on as [`super::work`] acts on one: its log row (with its slot), the
/// loop's progress, and its stop rules. A stop that needs the user is kept in `pass.halt` and
/// the loop drains. `last`: no other child runs.
#[allow(clippy::too_many_arguments)]
fn task_ended(
    lp: &mut Loop,
    lock: &mut LoopLock,
    pass: &mut Pass,
    t: Live,
    end: std::result::Result<End, Halt>,
    last: bool,
    out: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<()> {
    let issue = t.issue;
    // A queued `run` job's task (issue 633): its job's result, not a task's row.
    if let Some(job) = t.job {
        if let Some(stop) = lp.run_job_ended(job, issue, end, out, warn)? {
            lp.draining = true;
            pass.halt.get_or_insert(stop);
        }
        return Ok(());
    }
    let row = |outcome: String, ready: Option<u64>, t: &Live, lp: &Loop| {
        let rec = lp.record(issue);
        let duration = |agent: Option<&record::Agent>| agent?.run.as_ref().map(|r| r.duration_s);
        Row {
            issue,
            title: t.title.clone(),
            size: t.size.clone(),
            slot: Some(t.slot),
            started: t.started,
            ready,
            worker_s: rec.as_ref().and_then(|r| duration(r.worker.as_ref())),
            reviewer_s: rec.as_ref().and_then(|r| duration(r.reviewer.as_ref())),
            rounds: rec
                .as_ref()
                .and_then(|r| r.review.as_ref())
                .map(|r| r.round),
            must_fix: rec
                .as_ref()
                .and_then(|r| r.review.as_ref())
                .map(|r| r.must_fix),
            outcome,
            stops: t.stops.clone(),
            idle_before: t.idle_before,
        }
    };
    let end = match end {
        Ok(end) => end,
        Err(Halt::Error(e)) => {
            lp.draining = true;
            note(lock, lp, warn, |p| {
                if p.current == Some(issue) {
                    p.current = None;
                }
            })?;
            pass.halt.get_or_insert(e);
            return Ok(());
        }
        // Stopped before any phase of it ran: no row, left as it was (review M-2 of issue 570).
        Err(Halt::Stopped) => {
            match &t.retried {
                Some((retry, cut_off, owed)) => {
                    if let Some(owed) = owed
                        && let Err(e) = lp.mark_resume(issue, owed.clone())
                    {
                        writeln!(
                            warn,
                            "warning: {}'s resume could not be marked as owed again ({e:#}); \
                             continue it with `sbxm task resume --issue {issue}`",
                            lp.issue_named(issue)
                        )?;
                    }
                    pass.due.push_front(t.held(*retry, *cut_off));
                }
                None if lp.record(issue).is_some() => {
                    writeln!(out, "{}", lp.stopped_line(issue))?;
                }
                None => {}
            }
            return Ok(());
        }
        Err(Halt::SignIn(e)) => {
            let rec = lp.record(issue);
            let stage = rec
                .as_ref()
                .map_or("no record".to_owned(), |r| r.stage.name().to_owned());
            let row = row(format!("stopped at {stage}: sign-in"), None, &t, lp);
            lp.append(&row, warn)?;
            lp.draining = true;
            pass.halt.get_or_insert(needs_user(
                Cause::SignIn,
                sign_in_stop(
                    e,
                    rec.is_some()
                        .then(|| format!("sbxm task resume --issue {issue}")),
                ),
            ));
            return Ok(());
        }
        Err(Halt::UsageLimit(total)) => {
            let stage = lp
                .record(issue)
                .map_or("no record".to_owned(), |r| r.stage.name().to_owned());
            let row = row(format!("stopped at {stage}: usage limit"), None, &t, lp);
            lp.append(&row, warn)?;
            lp.draining = true;
            pass.halt.get_or_insert(needs_user(
                Cause::UsageLimit,
                anyhow!(
                    "the usage limit held through {} of waits in a row; the loop stops; continue \
                     with `sbxm task resume --issue {issue}` once it resets",
                    queue::span(total.as_secs())
                ),
            ));
            return Ok(());
        }
    };
    let recorded = match &end {
        End::Done { .. } => Outcome::Done,
        End::Parked { .. } | End::Stopped(_) => Outcome::Parked,
        End::Refused(_) | End::Taken(_) => Outcome::Skipped,
        End::Held(_) => Outcome::Held,
    };
    note(lock, lp, warn, |p| {
        if p.current == Some(issue) {
            p.current = None;
        }
        p.set(issue, recorded);
        if matches!(recorded, Outcome::Done | Outcome::Parked) {
            p.counted += 1;
        }
    })?;
    let (ready, outcome, parked) = match end {
        End::Refused(reason) => {
            writeln!(out, "skipped #{issue}: refused before any write: {reason}")?;
            pass.refused.push(Skipped {
                issue,
                reason: format!("refused before any write: {reason}"),
            });
            return Ok(());
        }
        End::Taken(line) => {
            writeln!(out, "{line}")?;
            return Ok(());
        }
        End::Held(retry) => {
            pass.held.push(t.held(retry, false));
            return Ok(());
        }
        End::Done { ready, outcome } => (Some(ready), outcome, false),
        End::Parked { ready, outcome } => (ready, outcome, true),
        End::Stopped(outcome) => (None, outcome, false),
    };
    pass.due.extend(pass.held.drain(..));
    pass.tasks_run += 1;
    let row = row(outcome.clone(), ready, &t, lp);
    writeln!(
        out,
        "[issue-{issue} {}] {}: {outcome}",
        t.slot.as_str(),
        lp.issue_named(issue)
    )?;
    lp.append(&row, warn)?;
    if last {
        pass.idle_from = Some(ready.unwrap_or_else(|| lp.now()));
    }
    if parked {
        pass.parked_in_row += 1;
        if pass.parked_in_row >= 2 && pass.halt.is_none() {
            lp.draining = true;
            pass.halt = Some(needs_user(
                Cause::Parked,
                anyhow!(
                    "two tasks parked in a row; the loop stops; see `sbxm task status` for what \
                     each left"
                ),
            ));
        }
    } else {
        pass.parked_in_row = 0;
    }
    Ok(())
}

impl Loop<'_> {
    /// Whether the PRs awaiting the user leave room for one more child with `running` running
    /// (317(6)), checked before each start; says once when they don't. With no child of any
    /// kind running, [`Loop::wait_for_prs`] waited for them already.
    fn awaiting_room(
        &mut self,
        running: usize,
        out: &mut dyn Write,
        warn: &mut dyn Write,
    ) -> Result<bool> {
        if running == 0 && self.jobs.is_empty() {
            return Ok(true);
        }
        let awaiting = self.awaiting_prs(warn)?.len();
        if awaiting + running < PR_CAP {
            return Ok(true);
        }
        let line = format!(
            "paused: {awaiting} PRs await you and {running} tasks run, {PR_CAP} in all; no new \
             task starts until one ends or merges"
        );
        self.say_once(out, line)?;
        Ok(false)
    }

    /// Queued job `path`'s label, where it runs (`None`: as `[sandbox] location` says) as the
    /// job's own command chooses it, and whether it starts a task; `None` for a job that takes no
    /// slot, or whose file or issue can't be read here (the job's own run says why).
    pub(super) fn job_slot(
        &self,
        path: &std::path::Path,
    ) -> Option<(String, Option<Location>, bool)> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let job = jobs::parse(&text).ok()?;
        let (at, task) = match job {
            jobs::Job::Review { pr } => (self.review_slot(pr), true),
            jobs::Job::Fix { pr, .. } => (self.fix_slot(pr), true),
            // Where its labels pin it, as the job's run reads them.
            jobs::Job::Run { issue } => {
                let open = self.github.issues_open(&self.repo).ok()?;
                let found = open.iter().find(|i| i.number == issue)?;
                (
                    location::for_loop(&found.labels, self.cloud_slots).ok()?,
                    true,
                )
            }
            jobs::Job::Spec { .. } => (None, true),
            // A template build makes a sandbox, no task.
            jobs::Job::Template { .. } => (Some(Location::Local), false),
            jobs::Job::RmMerged {} => return None,
        };
        Some((job.label(), at, task))
    }

    /// Where `task review --pr N` runs PR `pr`'s review with no flag, as it chooses it
    /// (decision 253): where its `pr-N` task's record says, else `[sandbox] location`.
    fn review_slot(&self, pr: u32) -> Option<Location> {
        self.record_of(&format!("pr-{pr}")).map(|rec| rec.location)
    }

    /// Where `task fix --pr N` runs with no flag, as it chooses it ([`location::of_pr`]): where
    /// the PR's task ran, else its closing issue's label, else `[sandbox] location`. A failed
    /// read of GitHub falls back to the recorded place alone.
    pub(super) fn fix_slot(&self, pr: u32) -> Option<Location> {
        location::of_pr(&self.base_dir, &self.repo, pr, self.github)
            .unwrap_or_else(|_| self.fix_location(pr))
    }

    /// Why a step of the loop's own that runs in `location` waits for a later pass (issue 615):
    /// its slot is held by children, or, for a step that starts a task (`task`), a child waits out
    /// a usage limit (answer 4). `None`: it may run now; always so for a loop that runs one task
    /// at a time.
    pub(super) fn wait_reason(&self, location: Option<Location>, task: bool) -> Option<String> {
        self.slots?;
        if task && let Some(issue) = self.usage_hold {
            return Some(format!("#{issue}'s usage limit to clear"));
        }
        self.slot_taken(location).then(|| {
            format!(
                "a free {} slot",
                location.unwrap_or(self.default_location).as_str()
            )
        })
    }
}
