//! `sbxm task resume` (issue 118; decisions 173(c), 175; spec `sdlc/specs/task-state-machine.md`
//! section 5.2): continues an issue or spec task from its recorded stage, keeping its clone and
//! commits, and carries it on to `ready` the way `task review` would. Refused while the recorded
//! `sbxm` process is alive, and for a PR task (no worker; `task review --pr` moves it on). A
//! `task fix --pr N` task is named by `--pr N` (issue 485): the one made for the PR's current
//! head ([`fix_task_for_head`]). Decided through
//! `machine::TABLE`'s `Resume` rows; the refusal text for each state lives here.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::findings::{self, Finding};
use super::finish;
use super::lifetime;
use super::machine::{self, Event};
use super::next;
use super::pipeline::{self, Prepared, ReviewReport, TaskEnv, Worked};
use super::pr_fix;
use super::record::{self, Kind, Process, ProcessProbe, Record, Stage, Status, Stopped};
use crate::backend::Location;
use crate::github::GitHubBackend;
use crate::headless::RunStatus;
use crate::run::results::now;

/// What `resume` did.
#[derive(Debug)]
pub struct Resumed {
    /// The stage and status the task was resumed from.
    pub from: (Stage, Status),
    /// Whether the worker's sandbox was gone and had to be made again.
    pub sandbox_remade: bool,
    /// The worker's run, when the worker ran again.
    pub worked: Option<Worked>,
    /// The review that carried the task on to `ready` (or stopped it on failed gates); `None`
    /// when an earlier step failed.
    pub review: Option<ReviewReport>,
}

/// The status command to point at: a spec task has no number to filter by.
fn status_hint(task: &Record) -> String {
    match task.kind {
        Kind::Spec | Kind::Fix => "sbxm task status".to_owned(),
        _ => format!("sbxm task status --issue {}", task.number),
    }
}

/// The fix task `task resume --pr N` resumes (issue 485): the one PR `pr`'s fix tasks hold for the
/// PR's head as GitHub has it now, read from the repo they record. Refused, naming the way on,
/// when PR `pr` has no fix task, when the PR is closed, merged or from a fork (as `task fix`
/// refuses it), and when its only fix tasks were made for an older head: an old task is never
/// moved onto a new head. Reads only.
pub fn fix_task_for_head(base_dir: &Path, pr: u32, github: &dyn GitHubBackend) -> Result<String> {
    let fixes = finish::fix_tasks_of(base_dir, pr)?;
    if fixes.is_empty() {
        let review = format!("pr-{pr}");
        if finish::exists(base_dir, &review) {
            bail!(
                "PR #{pr} has no fix task, only its review task {review}, which has no worker to \
                 resume; to review the PR again, run `sbxm task review --pr {pr}`, or fix it with \
                 `sbxm task fix --pr {pr}`"
            );
        }
        bail!("no fix task of PR #{pr}; start one with `sbxm task fix --pr {pr}`");
    }
    let mut repos: Vec<String> = fixes
        .iter()
        .filter_map(|id| record::read(&record::task_dir(base_dir, id).join("task.json")).ok())
        .filter(|task| task.kind == Kind::Fix && task.number == pr)
        .map(|task| task.repo)
        .collect();
    repos.sort();
    repos.dedup();
    let list = fixes.join(", ");
    if repos.is_empty() {
        bail!(
            "the fix task(s) of PR #{pr} ({list}) have no readable task.json, so which repo's PR \
             they fix can't be told; see `sbxm task status`, or remove them with `sbxm task rm \
             --pr {pr}`"
        );
    }
    let mut heads = Vec::new();
    for repo in &repos {
        let info = github
            .pr(repo, pr)
            .with_context(|| format!("cannot read PR #{pr} of {repo}"))?;
        pr_fix::check_fixable(&info, repo, None)?;
        if let Some(id) = record::fix_id(pr, &info.head_oid)
            && fixes.contains(&id)
        {
            return Ok(id);
        }
        heads.push(info.head_oid);
    }
    bail!(
        "PR #{pr}'s head is now {}, and no fix task was made for it: {list} {} made for an older \
         head, and sbxm doesn't move a task onto a new head; remove {} with `sbxm task rm --pr \
         {pr}`, then fix the new head with `sbxm task fix --pr {pr}`",
        heads.join(", "),
        if fixes.len() == 1 { "was" } else { "were" },
        if fixes.len() == 1 { "it" } else { "them" },
    )
}

/// Whether all that is left of a fix task is its push onto its PR's branch (issue 485): it is
/// `ready` without a stop (the push didn't happen, or its process was gone before it), so
/// `task resume --pr` pushes it instead of resuming a stage.
pub fn only_push_left(task: &Record) -> bool {
    task.kind == Kind::Fix && task.stage == Stage::Ready && task.stopped.is_none()
}

/// Whether `task resume` (with `--rounds`, when given) may continue this task now. Every refusal
/// says why and what to do instead.
pub fn check_can_resume(
    task: &Record,
    probe: &dyn ProcessProbe,
    rounds: Option<u32>,
) -> Result<()> {
    check_can_resume_with(task, probe, rounds, false)
}

/// [`check_can_resume`], for `--should-fix` too when `should_fix` (issue 510): a clean `ready`
/// task then has something to resume, which [`check_should_fix`] checks.
pub fn check_can_resume_with(
    task: &Record,
    probe: &dyn ProcessProbe,
    rounds: Option<u32>,
    should_fix: bool,
) -> Result<()> {
    let id = &task.id;
    let state = machine::State {
        stage: task.stage,
        status: task.status,
        interrupted: task.is_interrupted(probe),
        kind: task.kind,
    };
    if task.kind == Kind::Pr {
        // `task review --pr` moves a PR task on from every state it can be left in: a `ready`
        // one (decision 190), a failed reviewer, and since issue 379 the states it used to be
        // stuck in (decision 282).
        bail!(
            "task {id} is a PR task, which has no worker or fix rounds to resume; to review the PR \
             again, run `sbxm task review --pr {}`",
            task.number
        );
    }
    if task.process_alive(probe)
        && let Some(process) = &task.process
    {
        bail!(
            "task {id} is still running in sbxm process pid {} (started {}); wait for it to \
             finish, or stop that process first",
            process.pid,
            process.started_at
        );
    }
    if should_fix {
        check_should_fix_rounds(task, rounds)?;
    }
    if rounds == Some(0) {
        bail!("--rounds adds fix rounds to the task's budget; give 1 or more");
    }
    if let Some(n) = rounds
        && task.stage != Stage::Ready
    {
        bail!(
            "--rounds {n} is for a task that stopped ready with its rounds used; task {id} is at \
             {} ({}), so resume it without --rounds",
            task.stage.name(),
            task.status.name()
        );
    }
    if let Some(n) = rounds
        && rounds_cap(task, n).is_none()
    {
        bail!(
            "--rounds {n} is too many: task {id} has used {} fix rounds, and the total can be at \
             most {}",
            task.round,
            u32::MAX
        );
    }
    if machine::verdict(&state, Event::Resume) {
        return if task.stage == Stage::Ready {
            if should_fix && task.stopped.is_none() {
                return Ok(());
            }
            check_stopped(task, rounds)
        } else {
            Ok(())
        };
    }
    match task.stage {
        Stage::Finished => bail!(
            "task {id} is already finished; to work on what its review found, file the findings \
             (`sbxm task file-findings`) and start tasks for them"
        ),
        stage => bail!(
            "task {id} is {} ({}), which `task resume` cannot continue; see `{}`",
            stage.name(),
            task.status.name(),
            status_hint(task)
        ),
    }
}

/// Whether `task resume --rounds N --should-fix` may run on this task now (issue 510, decision
/// 306), and if so the findings that are its fix round's work: the should-fix findings of the
/// task's `review.md` whose reviewer gave them `**Size:** S` (one without a size line counts as
/// not S). Only for a task that stopped `ready` clean (0 must-fix, no stop), at most once per task.
/// Reads only; every refusal says why and what to do instead.
pub fn check_should_fix(task: &Record, meta: &Path, rounds: Option<u32>) -> Result<Vec<Finding>> {
    let (id, flag) = (&task.id, next::record_flag(task));
    check_should_fix_rounds(task, rounds)?;
    if task.stage != Stage::Ready {
        bail!(
            "--should-fix is for a task that stopped ready with a clean review; task {id} is at {} \
             ({}), so resume it without --should-fix",
            task.stage.name(),
            task.status.name()
        );
    }
    let must_fix = task.review.as_ref().map_or(0, |r| r.must_fix);
    if task.stopped.is_some() || must_fix > 0 {
        bail!(
            "--should-fix is for a clean task (0 must-fix, no stop); task {id} stopped ready ({}) \
             with {must_fix} must-fix finding(s), so resume it without --should-fix",
            task.stopped.map_or("no stop", Stopped::name)
        );
    }
    if task.should_fix_round {
        bail!(
            "task {id} already had its should-fix round, and a task gets at most one; finish it \
             with `sbxm task finish {flag}`, and what is left is filed with `sbxm task \
             file-findings`"
        );
    }
    let path = meta.join("review.md");
    let text = fs::read_to_string(&path)
        .with_context(|| format!("cannot read task {id}'s review, {}", path.display()))?;
    let small: Vec<Finding> = findings::parse(&text)
        .findings
        .into_iter()
        .filter(|f| f.label == "should-fix" && f.size() == Ok("S"))
        .collect();
    if small.is_empty() {
        bail!(
            "task {id}'s review has no should-fix finding sized S (one without a **Size:** line \
             counts as not S), so there is no should-fix round to run; finish it with `sbxm task \
             finish {flag}`, then file its findings with `sbxm task file-findings`"
        );
    }
    Ok(small)
}

/// A should-fix round is exactly one fix round (issue 510, answers 1 and 3; review M-1): its
/// review's new must-fix is a work stop, which a larger budget would turn into another fix round.
fn check_should_fix_rounds(task: &Record, rounds: Option<u32>) -> Result<()> {
    if rounds == Some(1) {
        return Ok(());
    }
    let flag = next::record_flag(task);
    let given = rounds.map_or_else(|| "no --rounds".to_owned(), |n| format!("--rounds {n}"));
    bail!(
        "--should-fix runs exactly one fix round, so it takes --rounds 1 ({given} was given): run \
         `sbxm task resume {flag} --rounds 1 --should-fix`; a must-fix that round's review raises \
         stops the task, and more rounds for it are an ordinary `sbxm task resume {flag} --rounds \
         N` after that stop"
    )
}

/// The review a should-fix round's worker is given (issue 510): `findings`, taken whole from the
/// task's `review` text, under a heading that says they are the round's only work.
pub fn should_fix_review(review: &str, findings: &[Finding]) -> String {
    let lines = findings::lines_of(review);
    let mut text = String::from(
        "# Review: the should-fix findings sized S\n\nThe review found no must-fix finding. This \
         fix round's work is the review's should-fix findings sized S, below; its other findings \
         are filed as issues once the task finishes.\n",
    );
    for finding in findings {
        let end = finding.end_line.min(lines.len());
        let start = finding.start_line.saturating_sub(1).min(end);
        text.push('\n');
        text.push_str(lines[start..end].join("\n").trim_end());
        text.push('\n');
    }
    text
}

/// Refuses to resume a task whose record holds a cloud create that was started and never
/// recorded as returned (decision 268), unless `forget` (`--forget-ambiguous`): sbxm can't tell
/// whether that sandbox is the one it asked for or another caller's, so it neither uses nor
/// removes it, and the user looks first.
pub fn check_ambiguous(task: &Record, forget: bool) -> Result<()> {
    if forget {
        return Ok(());
    }
    refuse_ambiguous(task, "be resumed")
}

/// The one guard of decision 268 for every path that would list, copy to or from, `exec` in,
/// reuse, rebuild over or remove a cloud sandbox by its name: refuses, `task <id> can't <what>
/// yet`, when the record holds a create that was started and never recorded as returned. Such a
/// sandbox is never inferred to be the task's from the cloud's listing; the user looks first and
/// then follows the task's kind of recovery (`recovery`).
pub fn refuse_ambiguous(task: &Record, what: &str) -> Result<()> {
    match task.ambiguous_create() {
        Some(name) => bail!(
            "task {} can't {what} yet: {}",
            task.id,
            lifetime::ambiguous_create(name, &recovery(task))
        ),
        None => Ok(()),
    }
}

/// What the user runs once the sandbox is removed or kept: a PR task can't be resumed (its
/// `task resume` target doesn't exist), so `task rm --pr` drops the entry and the review is asked
/// for again; an issue or spec task is resumed.
fn recovery(task: &Record) -> String {
    match task.kind {
        Kind::Pr => format!(
            "once you have removed it or decided to keep it, run `sbxm task rm --pr {n} \
             --forget-ambiguous` and then `sbxm task review --pr {n}`; the first drops it from \
             task.json and touches nothing in the cloud",
            n = task.number
        ),
        _ => format!(
            "once you have removed it or decided to keep it, run `sbxm task resume {} \
             --forget-ambiguous`, which drops it from task.json and touches nothing in the \
             cloud",
            next::record_flag(task)
        ),
    }
}

/// The fix-round cap `--rounds n` sets (issue 436): n more rounds from now, so the rounds already
/// used plus n, whatever the stop; `None` when that overflows.
pub fn rounds_cap(task: &Record, n: u32) -> Option<u32> {
    task.round.checked_add(n)
}

/// A `ready` task is resumed only when it stopped with something unresolved (spec section 5.2):
/// `--rounds N` adds rounds from any stop, and without it a repeat stop needs rounds left in the
/// budget (decision 177(q)).
fn check_stopped(task: &Record, rounds: Option<u32>) -> Result<()> {
    let (id, flag) = (&task.id, next::record_flag(task));
    let Some(stopped) = task.stopped else {
        // Its push onto the PR's branch is what is left: `task resume --pr` makes it (issue 485).
        if task.kind == Kind::Fix {
            if rounds.is_some() {
                bail!(
                    "task {id} is ready and did not stop on anything unresolved, so it needs no \
                     more fix rounds; push it with `sbxm task resume {flag}`"
                );
            }
            return Ok(());
        }
        bail!(
            "task {id} is ready and did not stop on anything unresolved, so there is nothing to \
             resume; publish it with `sbxm task finish {flag}`"
        );
    };
    let used_up = task.round >= task.fix_rounds;
    if rounds.is_none() && (stopped == Stopped::RoundsExhausted || used_up) {
        bail!(
            "task {id} stopped ready ({}) with all {} of its fix rounds used; give it more with \
             `sbxm task resume {flag} --rounds N`",
            stopped.name(),
            task.fix_rounds
        );
    }
    Ok(())
}

/// Whether resuming the task at this stage and status restores the worker's input, rebuilds its
/// sandbox to run it, or runs the worker or a fix round again (M-3, PR 163 review): the stages
/// where [`pipeline::check_worker`] must run first, the same way [`pipeline::check_reviewer`]
/// already does for every resume. A path that only collects commits already made, runs the gates,
/// or replays a recorded review never touches the worker and needs no such check.
fn resume_runs_worker(task: &Record) -> bool {
    matches!(
        (task.stage, task.status),
        (Stage::Prepared, _)
            | (Stage::Working, Status::Running | Status::Failed)
            | (Stage::Fixing, Status::Running | Status::Failed)
            | (Stage::Ready, _)
    )
}

/// Continues the task from its recorded stage (issue 118). Checks first: nothing runs or is
/// written when [`check_can_resume`], the reviewer's checks, (when the worker will run) the
/// worker's checks, or (when a recorded review is replayed) [`pipeline::check_replay`] refuse.
///
/// A task recorded in the cloud (milestone 4 slice 6) is resumed there: `env.backend` is the
/// cloud instance, the worker's checks are the cloud preflight of its sandbox's rebuild, made
/// whatever the stage (decision 261), a gone worker sandbox is made again from the recorded
/// profile and a bundle of the branch (P5 (d)), the task's input goes into it by `cp`, and it is
/// removed when the task stops, whichever way the resume ends after the checks.
pub fn resume(env: &TaskEnv, prepared: &mut Prepared, rounds: Option<u32>) -> Result<Resumed> {
    resume_forgetting(env, prepared, rounds, false, false)
}

/// [`resume`], with `--forget-ambiguous` when `forget` (decision 268): a cloud create that was
/// started and never recorded as returned is refused ([`check_ambiguous`]) unless `forget`, when
/// it is dropped from `task.json` once every other check passed, with no cloud call for it.
///
/// With `should_fix` (`--should-fix`, issue 510, decision 306), a clean `ready` task's fix round
/// is given only its review's should-fix findings sized S ([`check_should_fix`]), and the record
/// says it had its should-fix round.
pub fn resume_forgetting(
    env: &TaskEnv,
    prepared: &mut Prepared,
    rounds: Option<u32>,
    forget: bool,
    should_fix: bool,
) -> Result<Resumed> {
    check_can_resume_with(&prepared.record, env.probe, rounds, should_fix)?;
    check_ambiguous(&prepared.record, forget)?;
    let small = if should_fix {
        Some(check_should_fix(&prepared.record, &prepared.meta, rounds)?)
    } else {
        None
    };
    if small.is_none() && only_push_left(&prepared.record) {
        bail!(
            "task {} is ready with nothing unresolved: only its push onto PR #{}'s branch is \
             left, which `sbxm task resume --pr {}` makes",
            prepared.record.id,
            prepared.record.number,
            prepared.record.number
        );
    }
    pipeline::check_reviewer(env, prepared)?;
    let cloud = prepared.record.location == Location::Cloud;
    if cloud {
        pipeline::check_cloud_rebuild(env, &prepared.record, rounds)?;
    } else if resume_runs_worker(&prepared.record) {
        pipeline::check_worker(env, prepared)?;
    }
    if (prepared.record.stage, prepared.record.status) == (Stage::Reviewing, Status::Completed)
        && let Some(result) = &prepared.record.review
    {
        pipeline::check_replay(prepared, result)?;
    }
    // Every check passed: a cloud task takes its cloud slot now, before anything changes (issue
    // 599).
    let _slot = pipeline::take_cloud_slot(env, &prepared.record)?;
    if let Some(name) = prepared.record.ambiguous_create().map(str::to_owned) {
        record::write_change(&prepared.meta, &mut prepared.record, |t| {
            t.forget_ambiguous();
        })?;
        prepared.events.push(format!(
            "cloud sandbox {name}'s ambiguous create is dropped from task.json; nothing in the \
             cloud was touched for it"
        ));
    }
    let resumed = resume_checked(env, prepared, rounds, small.as_deref());
    // The task stops here, whichever way: a cloud worker sandbox goes (P5 (d), decision 263).
    // A local one goes where the phase that used it ends (issue 341), never after a refusal.
    pipeline::stop_cloud_worker(env, prepared);
    resumed
}

/// [`resume`] once its checks have passed.
fn resume_checked(
    env: &TaskEnv,
    prepared: &mut Prepared,
    rounds: Option<u32>,
    should_fix: Option<&[Finding]>,
) -> Result<Resumed> {
    let cloud = prepared.record.location == Location::Cloud;
    let mut resumed = Resumed {
        from: (prepared.record.stage, prepared.record.status),
        sandbox_remade: false,
        worked: None,
        review: None,
    };
    let probe = env.probe;
    // A worker that runs again gets the task's own input back first, before any sandbox is
    // touched: the last run could change it, or leave a link where it goes. A cloud clone gets it
    // once its sandbox is there, below.
    let reruns_worker = matches!(
        (prepared.record.stage, prepared.record.status),
        (Stage::Prepared, _) | (Stage::Working, Status::Running | Status::Failed)
    );
    if reruns_worker && !cloud {
        pipeline::restore_worker_input(env.backend, prepared)?;
    }
    // A stopped `ready` task's fix round works from its recorded review: it is read, and with the
    // prompt written into the clone, before the sandbox, the budget or the stage change. A cloud
    // task's worker sandbox went at its stop: the fix round makes it again and places the input.
    // A failed fix round's retry is given the input it had, read and placed the same way, before
    // a local sandbox is made again (issue 341: it is usually gone by then).
    let fix_input = match (prepared.record.stage, prepared.record.status) {
        (Stage::Ready, _) => Some(match should_fix {
            Some(small) => pipeline::should_fix_input(env, prepared, small)?,
            None => pipeline::fix_input(env, prepared, None)?,
        }),
        (Stage::Fixing, Status::Running | Status::Failed) => {
            Some(pipeline::rerun_fix_input(prepared)?)
        }
        _ => None,
    };
    if let Some(input) = &fix_input
        && !cloud
    {
        input.place(env.backend, prepared)?;
    }
    // The worker's sandbox runs the worker, the sandbox gates and every fix round; it can vanish
    // between stages (workflow note G17), so it is made again first when it is gone. A
    // preparation makes it afresh below instead, and a cloud fix round from `ready` makes its own.
    // A local review needs none: its gates make it again when they run (issue 341).
    if prepared.record.stage != Stage::Prepared
        && !(cloud && prepared.record.stage == Stage::Ready)
        && (cloud || prepared.record.stage != Stage::Reviewing)
    {
        resumed.sandbox_remade = pipeline::ensure_worker_sandbox(env, prepared, false)?;
        if resumed.sandbox_remade {
            prepared
                .record
                .notes
                .push("resume: the worker's sandbox was gone and was made again".to_owned());
        }
    }
    if reruns_worker && cloud && prepared.record.stage != Stage::Prepared {
        pipeline::restore_worker_input(env.backend, prepared)?;
    }
    // Whether the worker runs again (its stage is set up below, running).
    let mut run_worker = false;
    match (prepared.record.stage, prepared.record.status) {
        // F4, decision 175: a cut-off `sbx create` may have left a half-made sandbox, so it is
        // made afresh. A failure leaves the task `prepared/failed` with its folders, for another
        // resume.
        (Stage::Prepared, _) => {
            if let Err(e) = pipeline::ensure_worker_sandbox(env, prepared, true) {
                prepared.record.finish(Status::Failed)?;
                prepared
                    .record
                    .notes
                    .push(format!("resume: the preparation failed again: {e:#}"));
                record::write(&prepared.meta, &prepared.record)?;
                return Err(e);
            }
            resumed.sandbox_remade = true;
            prepared.record.rerun(now(), Process::current(probe))?;
            prepared
                .record
                .advance(Stage::Working, now(), Process::current(probe))?;
            run_worker = true;
        }
        // T6, T9: the worker runs again in its own clone, which keeps what the last run did.
        (Stage::Working, Status::Running | Status::Failed) => {
            prepared.record.rerun(now(), Process::current(probe))?;
            run_worker = true;
        }
        // A worker or fix round that ended, but whose process was gone before the review: it may
        // have been cut off while its commits were collected, so they are collected again (a
        // fetch of what `repo.git` already has changes nothing). A fix round is counted once its
        // run is over (`review_issue` adds it then), so this one was never counted.
        (Stage::Working | Stage::Fixing, Status::Completed | Status::TimedOut) => {
            if let Err(e) = pipeline::recollect_commits(env, prepared) {
                pipeline::release_local_worker_into(env, prepared);
                return Err(e);
            }
            if prepared.record.stage == Stage::Fixing {
                prepared.record.round += 1;
            }
        }
        // Stopped `ready`: the budget becomes the rounds used plus `--rounds` ([`rounds_cap`]),
        // `stopped` is cleared (a later stop sets it again), and a fix round runs from the
        // recorded review.
        (Stage::Ready, _) => {
            if let Some(n) = rounds {
                prepared.record.fix_rounds =
                    rounds_cap(&prepared.record, n).context("--rounds overflows the budget")?;
            }
            if let Some(small) = should_fix {
                // At most one per task (decision 306): recorded with the round's start below.
                prepared.record.should_fix_round = true;
                let ids: Vec<&str> = small.iter().map(|f| f.id.as_str()).collect();
                prepared.record.notes.push(format!(
                    "resume: a should-fix round on the should-fix findings sized S ({}), {} of \
                     {} fix rounds used so far",
                    ids.join(", "),
                    prepared.record.round,
                    prepared.record.fix_rounds
                ));
            } else {
                let was = prepared.record.stopped.take().map_or("", Stopped::name);
                prepared.record.notes.push(format!(
                    "resume: stopped {was}; another fix round, {} of {} used so far",
                    prepared.record.round, prepared.record.fix_rounds
                ));
            }
            let input = fix_input.context("the fix round's input was not read")?;
            resumed.review = Some(pipeline::fix_from_ready(env, prepared, &input)?);
            return Ok(resumed);
        }
        // T5, T9: the fix round runs again in the same clone with the input it was given, then
        // the gates and the review rounds go on.
        (Stage::Fixing, Status::Running | Status::Failed) => {
            let input = fix_input.context("the fix round's input was not read")?;
            pipeline::rerun_fix_round(env, prepared, &input)?;
            resumed.review = Some(pipeline::continue_after_fix(env, prepared)?);
            return Ok(resumed);
        }
        // T3: its process is gone, so the review it was running never finished: the same as one
        // that failed, which `review_issue` runs again (its gates passed for these commits).
        (Stage::Reviewing, Status::Running) => {
            prepared.record.status = Status::Failed;
            prepared
                .record
                .notes
                .push("resume: the review was interrupted; it runs again".to_owned());
        }
        // T4: the review completed, but its process was gone before it acted on the result: the
        // recorded result is replayed (decision 177(n)), so the reviewer doesn't run again. An
        // older record without one runs the reviewer again, as after a failed review.
        (Stage::Reviewing, Status::Completed) => match prepared.record.review.clone() {
            Some(result) => {
                let report = pipeline::ReviewReport::default();
                let round = result.round;
                resumed.review = Some(pipeline::review_rounds(
                    env,
                    prepared,
                    report,
                    round,
                    Some(result),
                    false,
                )?);
                return Ok(resumed);
            }
            None => {
                prepared.record.status = Status::Failed;
                prepared.record.notes.push(
                    "resume: the completed review's result was not recorded; it runs again"
                        .to_owned(),
                );
            }
        },
        // Gates (ended or cut off) and a failed review: what `review_issue` already continues.
        (Stage::Gating, _) | (Stage::Reviewing, Status::Failed) => {}
        (stage, status) => bail!(
            "task {} is {} ({}), which `task resume` cannot continue yet",
            prepared.record.id,
            stage.name(),
            status.name()
        ),
    }
    if run_worker {
        record::write(&prepared.meta, &prepared.record)?;
        // A failed worker ends its phase here; one that completed goes on to its gates.
        let worked = match pipeline::work(env, prepared) {
            Ok(worked) => worked,
            Err(e) => {
                pipeline::release_local_worker_into(env, prepared);
                return Err(e);
            }
        };
        let failed = matches!(worked.status, RunStatus::Failed(_));
        resumed.worked = Some(worked);
        if failed {
            pipeline::release_local_worker_into(env, prepared);
            return Ok(resumed);
        }
    }
    resumed.review = Some(pipeline::review_issue(env, prepared)?);
    Ok(resumed)
}
