# Improvement review: the task pipeline and the loop (trial, issue #152)

Scope: `b6482a64babda56a4c13b69a7a35b3c64fe66aa3` (main), `src/task/`, `src/commands/task_loop.rs`, `src/commands/task_loop/`
Issues: pending (the user sorts the findings: issue #152's trial measure)

## Should fix

### IMP-1 - Two record-based checks of a PR's fix tasks disagree on a failed log-fix, so a restarted loop can arm what the running loop left

**Where:** `src/commands/task_loop.rs:2902-2929`, `src/commands/task_loop/waiting.rs:392-402`, `src/commands/task_loop/waiting.rs:354`, `src/task/queue.rs:200-214`, `src/commands/task_loop.rs:2815`, `src/commands/task_loop.rs:3690`
**What happens:** whether the fix tasks already pushed onto PR N (resolves and log-fixes, `fix-N-<head>`) let the PR be armed is decided twice, from the same records, by different rules. Both cite the same review M-2 of decision 300's issue.
- `Loop::earlier_resolves_pass` (`task_loop.rs:2902-2929`, used by the resolve and log-fix re-arms at 2815 and 3690) walks `record::task_folders` itself. A log-fix with `check-log.md` passes only if `log_fix_left(..)` is `None`, i.e. it is finished and clean ("A log-fix that isn't clean left the PR to the user, whatever came after it", 2919-2920). A folder that isn't sbxm's own, or an unreadable `task.json`, counts as not passing (`return false`, `Err(_) => false`).
- `Loop::fixes_clean` (`waiting.rs:392-402`, used by `recheck` at 354 to arm a waiting low- or medium-risk PR) runs over `queue::fixes_of`, which silently drops unusable folders and unreadable records (`queue.rs:205-211`). It then skips every fix task that isn't finished: `if fix.stage != Stage::Finished { return true; }`.

A concrete input where they differ: a log-fix of PR N that stopped at `working` (status `failed`, `check-log.md` present).
- `earlier_resolves_pass` returns `false`: `log_fix_left` says "stopped at working".
- `fixes_clean` returns `true`.

In one process the in-memory `unresolved` set (`task_loop.rs:2885`, checked at `waiting.rs:277`) hides the difference. After a loop restart, which decision 310(3) makes routine, that set is empty, and only the record-based rules remain. `recheck` can then arm the PR the earlier loop left to the user. An unreadable fix record differs the same way: blocking in one path, invisible in the other.
**Why it matters:** this is kind 1 of the review, duplicated logic that has already drifted. The rule ("a log-fix that isn't clean left the PR to the user", decision 300, review M-2) is meant to hold "so a restarted loop sees them too" (`task_loop.rs:2900-2901`). One of its two copies doesn't.
**Fix:** one function in `src/task/queue.rs` next to `fixes_of`, e.g. `fixes_pass(base_dir, repo, number, except, check)`, that walks the fix folders once with `earlier_resolves_pass`'s rules (unusable or unreadable counts as not passing; a log-fix passes only finished and clean). `earlier_resolves_pass` and `fixes_clean` both call it, each passing its own record check: `passes_arm_checks`, or `clean` with the history risk. `log_fix_left` moves with it, or is passed in.
**Size:** S (two callers in one area, one new shared function, no state change)
**Depends on:** none known
**Related:** IMP-2 (both touch the loop's end-of-task paths; either order works)
**Acceptance criteria:**
- [ ] With a fix task `fix-N-<head>` at `working`/`failed` with a `check-log.md`, and a fresh `Loop` (no in-memory `unresolved`), the waiting-PR recheck doesn't arm PR N, and it says why on the PR's log row
- [ ] An unreadable `task.json` in a `fix-N-…` folder keeps PR N unarmed in both the resolve re-arm and the waiting recheck
- [ ] A test covering it fails before the fix and passes after (in a new file, e.g. `tests/main/task_loop_fixes_pass.rs`)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change beyond the arming decision itself (README unchanged)

### IMP-2 - The one-at-a-time loop and the parallel loop each handle a task's end in their own copy, and the copies already write different log rows

**Where:** `src/commands/task_loop.rs:4659-4844`, `src/commands/task_loop/parallel.rs:1087-1264`, `src/commands/task_loop.rs:4702-4718`, `src/commands/task_loop.rs:4733-4749`, `src/commands/task_loop/parallel.rs:1106-1126`
**What happens:** `work` (`task_loop.rs:4460-4846`, 387 lines, the loop without `cloud_slots`) and `task_ended` (`parallel.rs:1087-1264`, 178 lines, with `#[allow(clippy::too_many_arguments)]`) repeat the same steps nearly line for line:
- the `Halt::Stopped` arm with its owed-resume re-mark: `task_loop.rs:4663-4695` and `parallel.rs:1141-1162`
- the `Halt::SignIn` and `Halt::UsageLimit` rows and stops: 4696-4758 and 1163-1197
- the `recorded` outcome match and the `note(..)` progress update: 4760-4772 and 1199-1213
- the `End` classification: 4773-4802 and 1214-1234
- "two tasks parked in a row": 4831-4843 and 1248-1262

They have drifted in the log row of a task stopped by a sign-in or a usage limit.
- `work` writes `worker_s: None, reviewer_s: None, rounds: None` (4702-4718, 4733-4749).
- `task_ended` builds the same row with its `row` closure (1106-1126), which fills those three columns from the record.

The same issue stopped at `reviewing: sign-in` therefore logs its worker time and rounds with `cloud_slots` set, and blanks without. The slot column comes from `lp.slot_of(issue)` in one and from `t.slot` in the other. Since 2026-11-01, `work`'s lines changed in 41 commits and `task_ended`'s in 4 (`git log -L`), so a change made to one copy only is the expected case. The loop log is what the retros read (`sdlc/retros/2026-11-16-retro.md`, patterns 3 and 4).
**Why it matters:** this is kinds 1 and 2: one fact (what a task's end means for the progress, the log row and the loop's stop) computed twice and already differing. `work` at 387 lines is the second-largest function in scope.
**Fix:** split along the seam both copies already share. One `Loop` method takes the task's `End` or `Halt`, plus what the row needs (issue, title, size, slot, started, stops, idle), and returns what the caller does next: go on, hold, skip, or stop with this error. It writes the progress note and the one row, built by one `Loop::task_row` used by every issue-task row. `work` and `task_ended` keep only their own bookkeeping (`due`/`held` against `pass`, `continue` against `return`).
**Size:** M (two loop modes and their tests; no state or format change)
**Depends on:** none known
**Related:** IMP-1, IMP-4 (both change code next to `work`; doing IMP-2 first makes them smaller)
**Acceptance criteria:**
- [ ] A task stopped by a lapsed sign-in writes the same loop-log row (worker time, reviewer time, rounds, must-fix) with and without `[resources] cloud_slots`
- [ ] `grep -c "Row {" src/commands/task_loop.rs src/commands/task_loop/parallel.rs` shows the issue-task row built in one place
- [ ] A test covering it fails before the fix and passes after (a new file, e.g. `tests/main/task_loop_end_rows.rs`, running the same stopped task in both modes)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change except the filled columns

### IMP-3 - An issue's size is read two ways: the queue takes the first size label, the medium arming refuses two, and it re-parses `gh` text to get them

**Where:** `src/task/queue.rs:256-263`, `src/task/medium.rs:54-98`, `src/github/gh.rs:607-611`, `src/commands/task_loop/check.rs:158-160`, `src/commands/task_loop.rs:2065-2069`, `tests/main/task_loop_arm.rs:583-600`
**What happens:** two rules decide an issue's size.
- `queue::size` (`queue.rs:256-263`) takes the first `size:*` label. It is the size for:
  - the queue's order (`queue_order`, `largest_first`)
  - `task loop --check`'s "no size label" reason (`check.rs:158`)
  - the loop log's Size column (`task_loop.rs:2065-2069`, 4637, `parallel.rs:892`, `jobs.rs:1128`)
- `medium::size_refusal` (`medium.rs:54-82`) has its own rule. It fetches the issue again as `gh issue view` text (`github.issue`), parses the `labels:` header itself (`medium.rs:85-98`, a copy of the header reader in `gh.rs:607-611`), and refuses more than one size: `_ => Some(format!("its issue #{issue} has more than one size label ({})", ..))`.

For an issue labelled `size:S` and `size:L`, the queue ranks it S, `--check` passes it and the log shows S. A medium-risk PR from it is refused for "more than one size label". With the labels in the other order, the queue says L. The test fixture shows the double source: `github_medium` (`task_loop_arm.rs:583-600`) has to script the labels twice, once as `labels:\t{labels}` text and once as `open_issue(3, &[READY, "size:S"], "")`.
**Why it matters:** this is kind 1 (one fact, two rules that disagree on a reachable input) and kind 3 (`medium.rs` re-parses a `gh` output format that `src/github/` owns). The sizing rule ("Every issue gets a size", `AGENTS.md`) means one size per issue.
**Fix:** one `queue::size` rule that returns the size, or an error for none and for more than one. Every caller uses it; the loop's check names a doubly-sized issue as blocked. `medium::size_refusal` gets the labels from `Issue::labels` (`issues_open`, or one structured read added to `GitHubBackend`) instead of parsing `IssueText::text`.
**Size:** S (one module's rule and its callers; the `GitHubBackend` read is optional)
**Depends on:** none known
**Acceptance criteria:**
- [ ] An issue labelled `size:S` and `size:L` is shown as blocked by `sbxm task loop --check` ("more than one size label") and is not taken by the queue
- [ ] `medium.rs` no longer contains `strip_prefix("labels:")`
- [ ] A test covering it fails before the fix and passes after (a new file, e.g. `tests/main/task_queue_size.rs`)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] `README.md`'s queue/`--check` text updated if it describes size labels

### IMP-4 - A loop that takes over a dead loop's lock deletes it at once and keeps who died only in memory, so an error or kill before the marks loses the dead loop's tasks

**Where:** `src/commands/task_loop.rs:623-632`, `src/commands/task_loop.rs:4250-4297`, `src/commands/task_loop.rs:4342-4365`, `src/commands/task_loop.rs:705-707`, `src/commands/task_loop.rs:1796-1815`
**What happens:** `take_lock` removes the dead loop's lock file and keeps its contents only as `dead = Some(held)` (`task_loop.rs:623-632`). The new lock it writes names only itself. Before anything records what `dead` owned, `run_loop` runs (4250-4297):
- the stop-file removal
- `free_log`, whose `?` can fail
- `Shown::load(..)?`
- `loop_log::post_late`, several GitHub calls

Only after that do the marks happen: `cut_off` marks owed resumes (1857-1870), `adopt` takes the live children, and `lock.adopted(..)` (4359) writes them into the lock.

If the new loop exits in that window, `LoopLock::drop` removes its own lock (550-555). The exit can be an error there, with `loop-trigger.ps1` restarting it (decision 310(3)), or a kill. The loop after it then finds no dead lock (`dead = None`):
- `cut_off`'s `by_dead` (1813) is false for every task the first loop was running. `owed_here` needs a mark that was never written, so those cut-off tasks aren't resumed.
- `ended_under`/`adopt` find nothing, so the live children of the first loop are never adopted, and their outcomes never finished or armed (issues 643, 657).

The comment at 4342-4343 says "their owed resumes are written before anything else can fail". Decision 310(4) says the task is "marked at once, under its lock". Neither holds for the steps before them. `tests/main/task_loop_adopt.rs` covers a second loop that died after it wrote `adopted` (`second_loop_died`), and no test fails a step between the take-over and the marks.
**Why it matters:** kind 4: a restart path that leaves the dead loop's work orphaned with no test. Decision 310(4)'s "marked at once" exists so that a loop failing before the resume leaves the task to the next one.
**Fix:** keep the dead loop's identity on disk until its tasks are marked and adopted. Two ways:
- write `dead`'s lock into the new lock (e.g. a `took_over: LockFile` field) in the same write as `take_lock`, and drop it once `lock.adopted` is written; a later loop whose dead lock holds `took_over` uses both
- or move `ended_under`, `cut_off` and `adopt` before `free_log`, `Shown::load` and `post_late`, so nothing that can fail runs first; a kill still loses it then

The first also covers a kill, at the cost of one lock field.
**Size:** M (the loop lock's format gains a field read by the loop and `sbxm status`; tests for the restart path)
**Depends on:** none known
**Related:** IMP-2 (same function; IMP-2 first avoids rework), IMP-5
**Acceptance criteria:**
- [ ] A loop that took over a dead lock and then failed at `free_log` (or any step before `cut_off`) leaves the next loop able to resume the dead loop's cut-off task and adopt its live child
- [ ] The comment at `task_loop.rs:4342-4343` matches what the code does
- [ ] A test covering it fails before the fix and passes after (a new file, e.g. `tests/main/task_loop_takeover_failure.rs`)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: `docs/workflow.md`'s restart notes, if they describe the take-over

### IMP-5 - The medium trial's merges and breaks are counted only by the loop process that armed the PR, so a restart before the merge loses them

**Where:** `src/commands/task_loop.rs:851-856`, `src/commands/task_loop.rs:4310-4325`, `src/commands/task_loop/waiting.rs:233-235`, `src/commands/task_loop.rs:3213-3223`, `src/commands/task_loop.rs:3361-3363`, `src/task/trial.rs:1-6`, `src/task/trial.rs:105-108`
**What happens:** the trial's record is the comments on the `medium-trial` issue: "sbxm keeps no state of its own for it" (`trial.rs:1-6`). It counts merges from those comments and stops at `SIZE` = 10 (`trial.rs:15`, 105-108). But the comments are written from state that lives only in the loop's memory, empty at each start (`task_loop.rs:4310-4325`):
- `arm_medium` puts an armed medium PR into `medium_armed` (`waiting.rs:235`) and `armed`.
- `check_main` posts the merge comment only for a PR still in `self.armed`/`medium_armed` (`task_loop.rs:3214-3223`).
- `pause` posts a break only for a PR in `medium_merged` (3361-3363).

A loop restarted between arming a medium PR and its merge (logon, `--stop` and restart, an error restart under decision 310(3)) therefore posts no merge comment for it. The trial undercounts and can arm more than 10 medium PRs. A red `main` after that merge is never recorded as the trial's break. `tests/main/task_loop_trial.rs` runs each case in one process. None restarts the loop between the arming and the merge.
**Why it matters:** kind 4: a restart leaves the trial half-recorded, with no test. Decision 316(4) fixes the trial at 10 medium auto-merges, judged on its breaks; both numbers come from these comments.
**Fix:** derive the trial's merges from durable facts at each pass instead of the in-memory sets. A medium PR armed by the loop is known from GitHub: auto-merge armed, review risk medium in its task record, or a `Medium trial: armed` line the window comment already posts. When a merge of such a PR has no merge comment yet (`trial::has_merge_of`), post it, whoever armed it. Watch its merge commit for a break the same way.
**Size:** M (the trial's merge and break detection, with restart tests; no new state file if the facts are read from GitHub and the task records)
**Depends on:** none known
**Related:** IMP-4 (both are restart gaps in the loop's start)
**Acceptance criteria:**
- [ ] A medium PR armed by one loop process and merged while a second runs gets its `Medium trial: merge` comment, once
- [ ] A red run on that merge commit seen by the second process posts the trial's break comment
- [ ] A test covering it fails before the fix and passes after (a new file, e.g. `tests/main/task_loop_trial_restart.rs`)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change beyond the comments posted

### IMP-6 - `prepare_source_with` is 415 lines and changes in nearly every pipeline feature; its rollback is a seam of its own

**Where:** `src/task/pipeline.rs:1426-1840`, `src/task/pipeline.rs:1437-1520`, `src/task/pipeline.rs:1549-1722`, `src/task/pipeline.rs:1735-1838`
**What happens:** `prepare_source_with` prepares every new task of every source, here or in the cloud. Its callers are `prepare_recorded` (1153), and through `prepare_source` (1422) `prepare_spec_resolved` (1327) and `prepare_fix` (1366). Its 415 lines hold three parts that already have their own comments:
- 1437-1520: the checks before acting (shape, profile, cloud TTL, and the per-source title/text/prompt/check log/expected start, three `match &source`)
- 1549-1722: the build closure (clone, plan, files, record, sandbox create)
- 1735-1838: the rollback on failure, about 100 lines, with a local branch (1739-1758) and a cloud branch (1759-1838)

`git log -L1426,1840:src/task/pipeline.rs --since=2026-10-16` lists 48 commits; 12 since 2026-11-01 (for example `bbd7545` archive before rollback, `fd6c13f` per-task sbx lines, `9db0020` keep a failed warm-up, `a2160f3` cloud shapes). Most of them touched the rollback or the checks, and each had to be read inside the whole function.
**Why it matters:** kind 2: the function every new start goes through has grown past what one change can hold in view. The rollback holds the decision-268 and review-M-1 rules (only the sandbox this start created, the record kept while a sandbox may exist) in 100 lines of nested branches.
**Fix:** split at the existing seams, with no behavior change: `check_source(..) -> Checked` (1437-1520), the build closure as a function taking `&mut Started` (1549-1722), and `undo_failed_start(local | cloud, ..) -> anyhow::Error` (1735-1838).
**Size:** M (one function in one module, a pure refactor, with the existing tests as the net)
**Depends on:** none known
**Acceptance criteria:**
- [ ] `prepare_source_with` is under 100 lines, and the rollback is a function of its own
- [ ] The existing start/rollback tests (`tests/main/task_start*.rs`, `task_cloud*.rs`, `task_failed_prepare_archive.rs`) pass unchanged; a refactor adds no test, so "a test fails before the fix" doesn't apply
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change

### IMP-7 - `record.rs` owns the task id grammar but not the issue/PR id, the workspace path or the clone suffixes, which are rebuilt by hand

**Where:** `src/task/record.rs:944-960`, `src/task/finish.rs:95-102`, `src/task/finish.rs:141-169`, `src/task/pipeline.rs:2237`, `src/task/pipeline.rs:4375`, `src/task/pipeline.rs:5274`, `src/task/pipeline.rs:1435`, `src/commands/task_loop.rs:1804-1808`, `src/commands/task_loop.rs:1910-1914`, `src/commands/task_loop.rs:1986-1990`, `src/task/queue.rs:77`
**What happens:** `record.rs` defines which ids are valid (`is_valid_id`, 944-960) and builds `spec_id`, `fix_id`, `task_dir` and `sandbox_name`. It has no constructor for `issue-<n>` or `pr-<n>`, nor for the workspace and clone folders.
- `format!("issue-{…}")` appears 56 times in `src/`. In scope: 15 times in `task_loop.rs`, plus `parallel.rs:196`, `task_loop/jobs.rs:291, 1089, 1202`, `queue.rs:77` and `pipeline.rs:555, 1045, 1152`.
- Two private helpers outside the scope do the same mapping: `main.rs:21` and `task_status.rs:14`.
- The predicate "this repo's issue task, not finished, no PR, id `issue-<number>`" is written three times in `task_loop.rs` (1804-1808, 1910-1914, 1986-1990).
- The workspace `<base>/tasks/<id>` is joined by hand at `pipeline.rs:233, 490, 1047, 1244, 1396, 1435` and `merged.rs:248`.
- The clone folders `<id>-gates` and `<id>-review` are created at `pipeline.rs:2237, 4375, 5274`. They are listed separately for removal in `finish.rs:95-102` (`task_folders`) and `finish.rs:152-155` (`fix_tasks_of`'s suffix list).

Nothing ties the creator to the remover: a third clone suffix added in the pipeline would leave `task rm` leaving its folder behind. Today they agree.
**Why it matters:** kind 3: a layout `record.rs` owns in part is re-derived in many places. Deferred gap G27 (`sdlc/evals-workflow-notes.md`, "task identifiers carry no project") is exactly the change that would have to find every one of them.
**Fix:** add `record::issue_id(n)`, `record::pr_id(n)` (or `record::id(kind, n)`), `record::workspace(base, id)` and one `record::CLONE_SUFFIXES` (or a `clone_dir(base, id, Role)`). Use them in the pipeline, the loop and `finish::task_folders`/`fix_tasks_of`; the two private helpers in `main.rs` and `task_status.rs` call the shared one.
**Size:** M (mechanical, but across the pipeline, the loop, finish and two commands)
**Depends on:** none known
**Acceptance criteria:**
- [ ] `grep -rn 'format!("issue-{' src/task src/commands/task_loop*` finds none outside `record.rs`
- [ ] `finish::task_folders` and the pipeline's clone paths take their suffixes from one list
- [ ] A test in `tests/main/` (a new file, e.g. `task_layout.rs`) asserts that `task rm` removes every clone folder the pipeline can create, fed from the same list; it fails if a suffix is added in one place only
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change

### IMP-8 - The loop copies the sign-in matching that `outcome.rs` says it owns

**Where:** `src/task/outcome.rs:1-6`, `src/task/outcome.rs:106-112`, `src/task/outcome.rs:127-136`, `src/commands/task_loop.rs:3782-3793`, `src/commands/task_loop/jobs.rs:992`
**What happens:** `outcome.rs`'s module doc says "The matching that finds a usage limit or a lapsed sign-in is done here, by the command that writes the file" (decision 321(2)). `Outcome::of` does it with:
- the `NotSignedIn` downcast (106)
- `says_harness_signed_out` (127-136): the `HarnessSignedOut` downcast, the error text, and output lines containing "failed"

`task_loop.rs:3782-3793` (`says_sign_in`) is the same four tests, line for line, used by the in-process job phase `Loop::phase` (`task_loop/jobs.rs:992`, spec and template jobs). The two agree today. A new sign-in wording or a new error type added to `outcome.rs` would leave the in-process path still seeing a plain failure, and the loop would go on instead of stopping for the user.
**Why it matters:** kind 3: a fact one module owns (how a failure is classified) re-derived in another, with nothing to keep them equal.
**Fix:** make `outcome.rs`'s classifier public as one function (e.g. `outcome::kind_of(&anyhow::Error, &str) -> Kind`) and have `Loop::phase` match on its `Kind` (`SignedOut`, `MissingSecret`, the rest), as `child_failed` (`task_loop/jobs.rs:466-492`) already does for children. Delete `says_sign_in`.
**Size:** S (one function made public, one caller changed, one deleted)
**Depends on:** none known
**Acceptance criteria:**
- [ ] `says_sign_in` is gone, and `Loop::phase` and `Outcome::of` call the same classifier
- [ ] A test (a new file, e.g. `tests/main/task_loop_job_sign_in.rs`) shows an in-process spec job whose output says a harness is signed out stops the loop with exit 3, and still passes after the change
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change

### IMP-9 - Each loop step reads every open loop PR's state from GitHub about ten times

**Where:** `src/commands/task_loop.rs:4552-4561`, `src/task/merged.rs:161`, `src/commands/task_loop.rs:2619`, `src/commands/task_loop.rs:3395`, `src/commands/task_loop.rs:3423`, `src/commands/task_loop.rs:3155`, `src/commands/task_loop/waiting.rs:280`, `src/commands/task_loop/waiting.rs:296`, `src/commands/task_loop.rs:1060`, `src/commands/task_loop.rs:1069`, `src/commands/task_loop.rs:1027`, `src/commands/task_loop.rs:1279-1315`
**What happens:** a step of the loop (`task_loop.rs:4552-4561`, and the same calls in `parallel.rs`'s `steps_and_starts`) runs `clean_merged`, `resolve_conflicts`, `check_runs`, `recheck_waiting`, `drain_jobs`, `notify_awaiting` and `wait_for_prs`. Each lists the loop's PRs again (`queue::loop_prs`, which reads every `task.json`) and reads each PR from GitHub again. Per open loop PR, per step:

| Step | GitHub reads |
|---|---|
| `clean_merged` | `github.pr` (`merged.rs:161`) |
| `resolve_conflicts` | `pr_merge_state` (2619) |
| `check_prs` | `pr_merge_state` (3395), `pr` (3423) and `commit_runs` (3155) |
| `recheck_waiting` | `pr_merge_state` (`waiting.rs:280`) and `pr` (296) |
| `notify_awaiting` | `pr_merge_state` (1060) and `pr` (1069) |
| `awaiting_prs` | `pr_merge_state` (1027) |

That is about 10 `gh` calls per PR, of which 4 identical `pr_merge_state` reads and 3 `pr` reads. A `gh` call costs about 0.5-1 s. With `PR_CAP` = 5 PRs awaiting, a step spends about 25-50 s on reads, and starts no task meanwhile. `wait_for_prs` and `--wait` repeat the checks every `PAUSE_POLL` (5 min, 1279-1315), so with 5 PRs waiting on the user the loop makes several hundred reads an hour (about 6 per PR per poll in `wait_for_prs`, more in a full step). In parallel mode `may_start` → `awaiting_prs` adds 5 more before each start.
**Why it matters:** kind 6, with a concrete cost: wall-clock time between tasks (the roadmap's first item is review and CI time, `docs/project.md`), and a share of the `gh` API budget.
**Fix:** read each loop PR once per step (state, head, draft, auto-merge) into a per-step map that every sub-step uses, and drop it at the step's end so nothing stale outlives it. A write (arm, disarm, push) invalidates that PR's entry.
**Size:** M (the loop's step functions take the map; tests count calls)
**Depends on:** none known
**Related:** IMP-2 (both reshape the loop's step; IMP-2 first)
**Acceptance criteria:**
- [ ] One loop step with one open loop PR makes at most one `pr_merge_state` and one `pr` read of it (`FakeGitHub::calls()`)
- [ ] An arm or disarm within a step is seen by the later sub-steps of the same step
- [ ] A test covering it fails before the fix and passes after (a new file, e.g. `tests/main/task_loop_reads.rs`)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs: no user-visible change

## Questions

### IMP-10 - Should a queued review or fix job run on a PR sbxm only pushed a fix to?

**Where:** `src/task/jobs.rs:497-502`, `src/task/jobs.rs:507-511`, `src/commands/task_loop/jobs.rs:1045-1058`, `src/task/finish.rs:2652`
**What happens:** a queued `review`/`fix` job runs "only of a PR sbxm opened (answer 4)" (`task_loop/jobs.rs:1045-1058`). Three predicates in scope answer "did sbxm open this PR?" differently:
- `jobs::opener` (`jobs.rs:497-502`), which the job gate uses, accepts any record whose `pr` names the PR (`named_by(.., |_| true)`). That includes a fix task's record: `finish_fix` sets `record.pr` to the PR it pushed to (`finish.rs:2652`).
- `jobs::first_opener` (507-511) accepts only issue and spec tasks that didn't continue a PR (issue 600).
- `queue::loop_prs` adds the loop's hook.

For a PR #77 opened by hand, on which the user once ran `sbxm task fix --pr 77`, the job gate passes. Its own refusal text says it applies when "sbxm didn't open it".
**Why it matters:** kind 1, three versions of one fact. It is a question because answer 4 may have meant "a PR sbxm has a record of", which a fix's record satisfies. The question names answer 4 of the queued-jobs issue (decision 309), and the evidence is that its words and `first_opener` (issue 600) both say "opened".
**Recommendation:**
- (a) the job gate uses `first_opener`, so only a PR an sbxm issue or spec task opened gets a queued job, and the refusal text stays true. Recommended: it matches the message and issue 600's rule, and a fixed hand-made PR can still be reviewed by hand.
- (b) keep `opener` as it is and reword the refusal ("no task record of … names PR #N").
**Size:** S (one predicate or one message, and a test)
**Depends on:** none known

## Summary

Read at `b6482a6` (lines per file):
- **`src/task/`:**
  - `pipeline.rs` 6038, `finish.rs` 2980, `rewrite.rs` 1471, `record.rs` 1195, `repo.rs` 1097
  - `machine.rs` 972, `merged.rs` 883, `review.rs` 837, `findings.rs` 771, `resume.rs` 679, `config.rs` 677, `queue.rs` 648, `outcome.rs` 619, `jobs.rs` 535
  - `gates.rs` 462, `stale_lock.rs` 412, `session.rs` 386, `loop_log.rs` 379, `lifetime.rs` 312, `pr_fix.rs` 292, `runlog.rs` 285, `window.rs` 272
  - `select.rs` 210, `risk.rs` 208, `loop_pause.rs` 203, `trial.rs` 183, `summary.rs` 182, `prompts.rs` 170, `warm_up.rs` 147, `fixes.rs` 126, `shape.rs` 118, `medium.rs` 110
  - `secrets.rs` 97, `location.rs` 79, `lock.rs` 75, `next.rs` 51, `mod.rs` 44, `real_check.rs` 43, `closes.rs` 42, `template_tasks.rs` 37
- **The loop:** `src/commands/task_loop.rs` 5703; in `src/commands/task_loop/`: `parallel.rs` 1415, `jobs.rs` 1306, `waiting.rs` 579, `child.rs` 425, `check.rs` 232, `job_children.rs` 226.
- **Total:** 34,213 lines.

The largest files were read by function (their structure, then each function a finding names, whole). The smaller ones were searched for the kinds below. Three read-only subagents swept for candidates; every finding here was checked against the code by hand. The tests, decisions 300, 309, 310, 315, 316, 321 and 322, `docs/project.md` and `sdlc/evals-workflow-notes.md` were read to drop tracked or decided items.

Findings per kind (a finding counted under its main kind):

| Kind | Findings |
|---|---|
| 1. Duplicated logic that drifted or could | 4 (IMP-1, IMP-2, IMP-3, IMP-10) |
| 2. Too large to change safely | 1 (IMP-6; IMP-2 is also `work`'s split) |
| 3. Hidden coupling | 2 (IMP-7, IMP-8) |
| 4. Half-done after a stop, kill or restart | 2 (IMP-4, IMP-5) |
| 5. Tests that pin implementation details | 0 |
| 6. Work done more often than needed | 1 (IMP-9) |

Looked at and found nothing worth a change:
- **State files:** `task.json`, `outcome.json`, the loop lock's rewrite, the pause file and the running job file are all written through a temp file and a rename, so a kill leaves no torn state file. The four copies of that pattern (`record.rs:1108`, `loop_pause.rs:143`, `task_loop.rs:534`, `jobs.rs:406`) differ only in temp-file cleanup.
- **Risk and history risk:** one rule (`queue::history_risk`, `queue::awaiting_rank`), shared by the loop and `sbxm status`.
- **`machine.rs`'s state table:** the one place for stage and status transitions.
- **The review-comment format between `review.rs` and `pr_fix.rs`:** its round trip is covered by the `task_fix*` tests, which build comments with the real writers.
- **`finish` killed between its push and the PR's create:** a retry adopts the pushed branch (issue 598), except when the review didn't cover the tip, which is refused by design.
- **`jobs::end`, which writes `done/` then removes `running/`:** a kill between the two would report the job twice. The window is two calls wide, too small to be worth a change.
- **Kind 5:** the call-order assertions found (`tests/main/task_finish.rs:1466` asserting `calls()[0]`, the `"room"` filters in `tests/main/task_loop.rs:502` and `:705`) are mild. None has broken on a refactor that I could find, so none is worth an issue.
- **`sbx ls`:** never called inside a loop in scope.
