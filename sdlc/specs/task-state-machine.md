# Task state machine: states, operations, gaps (draft, 2026-10-05)

**Status (updated 2026-10-06): both interviews are done; their answers are decisions 173 and 174 (sections 4 and 5);
sections 1-3 still describe today's code. Section 5 is the target design (one transition table with rounds and a
pluggable source), a draft for review.** It rebuilds a table the user remembers making on 2026-10-03 and that
was never saved to a file. It is derived from the code (`src/task/record.rs` `Stage::next`/`statuses`/`done`,
`check_can_gate`, `check_can_review`, `check_can_review_pr`, `check_can_finish`, `finish::plan_removal`), so it
describes what sbxm does today. Decision 169(f) ("a task's own `ready` state is not re-entered") is the choice this
draft questions.

**Issue 117 (part 3 of 7) built the `round`/`fix_rounds`/`stopped` fields, the multi-round
review-fix-review loop (`pipeline::review_issue`), the narrow-then-one-full reviewer scoping
(`Record::last_reviewed_commit`), and the gate-failure-feeds-a-fix-round move (`Stage::Gating`
`GatesFailed` to `Fixing`, `Record::done_for`) described in section 5.2 — everything in that section
except the no-progress/`repeat-finding` and `repeat-gate-failure` stops (173(a)(d)(e), 174(a); left
for the no-progress issue), `resume` and draft PRs, which are still open per section 5.5.**

**Issue 142 (part 7a of 7) built the `spec` source: `Kind::Spec`, `record::spec_id`,
`task start --spec <file>` (`pipeline::prepare_spec`) and `task review --spec <file>`, running the
same `pipeline::review_issue` loop as an issue task's (`machine::TABLE`'s `HAS_WORKER` rows cover
`Kind::Issue` and `Kind::Spec` alike, so no new rows were added). `review.md` is its only output
(`Role::ReviewerSpec`/`FixSpec`/`FixGateSpec`, no GitHub call anywhere); `task finish` is refused
for it (`check_can_finish`) until the sink (`[finish] sink` = `local`/`push`, decision 174(e))
lands, in the two issues that follow it.**

**Issue 143 (part 7b of 7) built the `local` sink: `[finish] sink` in `sbxm-task.toml` (`local`, the
default, or `push`), `finish::finish_local` (a ready spec task with commits beyond its base becomes
`finished`; its branch stays in `repo.git`, nothing is pushed, and `finish` prints the `git fetch`
command), `push` refused as not available yet (issue 144), and `task rm --spec` with its protection
(`finish::check_spec_result_fetched`, below in 5.4).**

**Issue 144 (part 7c of 7) built the `push` sink: `finish::finish_push` pushes the branch to `origin` as a
new branch (`repo::push_new`, an empty `--force-with-lease`), opens no PR, and keeps the safety rules of
5.4.**

## 1. States of an issue task (`stage` / `status`)

Legal moves (`Stage::next`): prepared -> working -> gating -> reviewing -> (fixing -> gating -> reviewing) -> ready -> finished.
A PR task (`pr-N`) has no worker: prepared -> gating -> reviewing -> ready.
A spec task (`spec-<name>-<hash>`, issue 142) walks the same moves as an issue task, `finished` included; its `finish`
goes through the `[finish] sink` instead of a PR (decision 174(e), issue 143).

| # | State (stage / status) | How it is reached |
|---|---|---|
| S1 | prepared / running | `task start` made the folders, clone and sandbox |
| S2 | prepared / failed | preparation failed after the first write |
| S3 | working / running | worker agent is running |
| S4 | working / completed or timed-out | worker finished (a timeout keeps partial work) |
| S5 | working / failed | worker errored or its commits couldn't be collected |
| S6 | gating / running | gates running |
| S7 | gating / passed | all gates passed |
| S8 | gating / gates-failed | a gate failed |
| S9 | reviewing / running | reviewer running |
| S10 | reviewing / completed | a valid review.md was saved |
| S11 | reviewing / failed | reviewer failed, timed out or wrote no valid review |
| S12 | fixing / running | worker is doing the one fix round |
| S13 | fixing / completed or timed-out | fix round finished |
| S14 | fixing / failed | fix round errored |
| S15 | ready / ok | review done (0 or more must-fix findings left) |
| S16 | finished / ok | branch pushed, PR opened |

"Interrupted" is a flag on any `running` state: the sbxm process that wrote it is gone (pid/start time).

## 2. Operations per state

`ok` = allowed. `no` = refused with a message. Blank-ish cells say what happens.

| State | `task gates` | `task review` | `task finish` | `task rm` / `start --restart` |
|---|---|---|---|---|
| S1 running | no (no worker) | no (no worker) | no | no while live; ok if interrupted |
| S2 failed | no | no | no | ok |
| S3 running | no (worker running) | no | no | no while live; ok if interrupted |
| S4 completed/timed-out | ok | ok (runs gates first) | no | ok |
| S5 failed | no | no | no | ok |
| S6 running | ok only if interrupted | ok only if interrupted | no | as S3 |
| S7 passed | ok | ok | no | ok |
| S8 gates-failed | ok (re-run) | ok (re-runs gates, stops if they fail) | no | ok |
| S9 running | no | no ("already being reviewed or has been") | no | as S3 |
| S10 completed | no | no | no | ok |
| S11 failed | no | ok (try again) | no | ok |
| S12 running | no | no | no | as S3 |
| S13 completed/timed-out | ok | ok | no | ok |
| S14 failed | no | no | no | ok |
| S15 ready | no | no ("already ready") | ok | ok |
| S16 finished | no | no | no (already has a PR) | ok |

## 3. Gaps (operations that are missing)

The only exits from several states are "delete the task and start over" (about 1.5 hours and a new sandbox).

| Gap | State | Today | Missing operation |
|---|---|---|---|
| T1 | S15 ready, must-fix left | `finish` (publishes with the findings) or fix by hand. Hit on issue 83, 2026-10-05. | A further fix round + review in a sandbox (G25's `--again`, rejected by decision 169(f)); or a rule that `finish` refuses while must-fix > 0 |
| T2 | S8 gates-failed | gates can be re-run; nothing makes the worker fix them | A fix round for failed gates |
| T3 | S9 running, interrupted | `review` refuses ("already being reviewed or has been") | Resume or retry a review whose process died (S11 can retry, S9 can't) |
| T4 | S10 completed | no command moves it on (only an interrupted `review` between Completed and Fixing/Ready leaves it) | Continue from a saved review |
| T5 | S12 running, interrupted / S14 failed | only `rm` | Retry the fix round (keep the clone and commits) |
| T6 | S5 failed (worker) | only `rm` / `--restart` | Retry the worker keeping the clone (the clone "keeps whatever it did") |
| T7 | S16 finished, review of the PR finds more | `task review --pr N` (new task), `file-findings`, then `start` per finding (decision 169) | Covered by decision 169; a PR with k findings costs k tasks and k sandboxes |
| T8 | any task whose PR changed after the review | `rm` then review again | Re-review without deleting (decision 169(f) chose `rm` + `review --pr`) |
| T9 | S1 / S3 / S6 / S9 / S12 interrupted | `rm` only | Resume at the interrupted stage |

Pattern: every stage that can fail or be interrupted has no retry except S11 (review) and S8 (gates). Round count is
fixed at one (decision 116), so "one more round" is a design question, not only a missing command.

## 4. Interview questions and the user's answers (2026-10-05; recorded as decision 173)

1. **Rounds:** configurable `[worker] fix_rounds`, default 3, with a no-progress stop.
2. **Ready with must-fix left:** `finish` opens a draft PR listing the findings.
3. **Retry operations** (T3, T5, T6, T9): one `task resume` from the recorded stage.
4. **Failed gates (T2):** a gate failure feeds a fix round, counted against `fix_rounds`.
5. **Reviewer scope:** rounds 2+ review only new commits; one full review before `ready`.
6. **Where the table lives:** the code is the single source (a state machine); docs are generated from it.
   Also from the interview: the task source is pluggable (issue, PR, spec, later a local repo).

## 5. Target design (decisions 173 and 174; draft for review)

### 5.1 What a state is

A task's state is `(stage, status)` as today, plus four recorded facts that today live in code or nowhere:

| Field | Meaning |
|---|---|
| `source` | `issue <n>`, `pr <n>` or `spec <path>`; decides the id (`issue-<n>`, `pr-<n>`, `spec-<name>-<hash>`: the sanitized file name plus the first 6 hex digits of the SHA-256 of the canonical path, so two files with the same name never collide), the input text copied into the task folder (`source.md` is the durable input for `resume`) and the sink |
| `round` | fix rounds used so far |
| `fix_rounds` | the budget: `[worker] fix_rounds` (default 3), raised by `resume --rounds N` |
| `stopped` | absent, or why the task ended `ready` with something unresolved: `repeat-finding`, `repeat-gate-failure` or `rounds-exhausted` |
| `last_gate_failure` | the command and exit code of the last failed gate, kept to detect a repeat (5.3) |
| `review` | the accepted result of the last review, recorded once when the review is parsed: SHA-256 of `review.md`, scope, must-fix count, the findings (id and `Where:` file), risk. Everything after that (`finish`, the no-progress check, the draft decision) reads this record, never `review.md` again, so a missing or edited file cannot change a decision |

Two more recorded facts: `last_reviewed_commit` (the commit the last review covered; a review is `narrow` only when
one exists, so the first review after a gate-only round is `full`), and `open_findings` (the must-fix findings still
unresolved across all recorded reviews since the last clean full review, so a draft PR lists all of them even after a
narrow review, and a stopped task never reaches `ready` understating what is open).

`task.json` gets `schema` 2 when `source`, `round`, `fix_rounds`, `stopped` or `review` are first written, and
`finish` also checks the recorded review's must-fix count itself (never only `stopped`), so an older sbxm that
ignores the new fields cannot open a normal PR for a task with must-fix findings left. `load_all` skips a record it
cannot read, with a warning naming it, so one new-format task cannot break `task status` for the others. Every
command that reads a task's record, checks it and writes it (`resume`, `review`, `gates`, `finish`, `rm`) holds an
exclusive lock file in the task folder for the whole read-check-write, so two commands cannot both pass the
liveness check and run two agents in one clone.

A review's `scope` is `narrow` (commits since the last review) or `full` (all commits). The saved `review.md` starts
with a `Reviewer:` line, so the parser reads the reviewer's own text before the header is added, and the result is
stored in `review` above (the format round-trip is tested through the exact write and read path).

Source capabilities, not special cases: each source answers `has_worker`, `runs_gates`, `review_destination` and
`finish_sink` (5.4), and the table's predicates use these, so a new source adds one answer set and no new rows.

### 5.2 The transition table

**Generated by `sbxm task states` from `src/task/machine.rs`'s `TABLE`; checked against this file by
`tests/task_states.rs`'s `the_generated_table_matches_the_spec`, which fails the moment the two differ**
(migration step 3 of 5.5). What follows is in two parts: the table the code has today (built through migration
step 2 of 5.5: the pipeline's moves, `task gates`, `task review`, `task finish` and `task rm`), and the target
table (decisions 173, 174) for the rows not yet built — `resume`, `fix_rounds`, `Repeat of:`, the `spec` source —
kept here as the design for issues #117-121 to build toward.

#### Today (generated)

| From | Event | To | Action |
|---|---|---|---|
| prepared running (issue/spec) | advance | working | run the worker |
| prepared running (pr) | advance | gating | run the gates |
| prepared running (pr) | advance | reviewing | run the reviewer |
| working completed/timed-out (issue/spec) | advance | gating | run the gates |
| gating passed | advance | reviewing | run the reviewer |
| gating gates-failed (issue/spec) | advance | fixing | run the fix round for the gate's own output |
| reviewing completed (issue/spec) | advance | fixing | run the fix round |
| reviewing completed | advance | ready | record the task as ready |
| fixing completed/timed-out (issue/spec) | advance | gating | run the gates |
| ready ok (issue) | advance | finished | push the branch and open the PR |
| ready ok (spec) | advance | finished | deliver the branch through the [finish] sink |
| working/fixing completed/timed-out (issue/spec) | task gates | gating | run the gates |
| gating passed/gates-failed (issue/spec) | task gates | gating | re-run the gates |
| gating running (issue/spec), interrupted | task gates | gating | re-run the abandoned gates |
| working/fixing completed/timed-out (issue/spec) | task review | reviewing | run the gates, then the reviewer |
| gating passed/gates-failed (issue/spec) | task review | reviewing | run the reviewer |
| gating running (issue/spec), interrupted | task review | reviewing | re-run the abandoned gates, then the reviewer |
| reviewing failed (issue/spec) | task review | reviewing | retry the reviewer |
| prepared running (pr) | task review | gating | run the gates, then the reviewer |
| gating passed/gates-failed (pr) | task review | reviewing | run the reviewer |
| reviewing failed (pr) | task review | reviewing | retry the reviewer |
| ready ok (issue) | task finish | finished | push the branch and open the PR |
| ready ok (spec) | task finish | finished | deliver the branch through the [finish] sink |
| any completed/timed-out/failed/passed/gates-failed/ok | task rm | removed | remove the task's folders and sandboxes |
| any running, interrupted | task rm | removed | remove the task's folders and sandboxes |

#### Target (decisions 173, 174; not yet built — issues #117-121; the `prepared`-to-`ready` rows
for a spec source are built by issue 142 through the existing issue-task mechanics below, not this
table yet — see the note after it)

Events: `worker` (done/timed-out/failed), `gates` (passed/failed), `review(n, repeat)` (n must-fix findings;
`repeat` = a finding the reviewer marked `Repeat of: <id>` whose file matches the earlier finding's), `fix`
(done/timed-out/failed), `interrupted` (a `running` state whose process is gone), `resume(+N)`, `finish`, `rm`, and
the manual operations `task gates` and `task review`, which are the same events entering the table by hand (the
spike's agreement test already walks them).

| From | Event | To | Action |
|---|---|---|---|
| prepared | start, source has a worker | working | run worker |
| prepared | start, no worker (pr), gates configured | gating | run gates, then the reviewer |
| prepared | start, no worker (pr), no gates configured | reviewing | run reviewer |
| prepared | preparation failed | prepared `failed` | `resume` retries preparation (decision 175) |
| working | worker done or timed-out, no new commits | working `failed` | nothing to review; `resume` retries |
| working | worker done or timed-out, commits collected | gating | run gates |
| working | worker failed | working (retry only by `resume`) | keep clone and commits |
| gating | gates passed | reviewing, scope `narrow` if a `last_reviewed_commit` is recorded else `full` | run reviewer |
| gating `running` | interrupted (the process died) | gating, flagged interrupted; no round is used | `resume` or `task gates` re-runs the gates; an abandoned gate run is never recorded as `gates-failed` |
| gating | gates failed, same command as `last_gate_failure` | ready, `stopped` = `repeat-gate-failure` | recorded as unresolved |
| gating | gates failed, `round` < `fix_rounds`, source has a worker | fixing, `round` + 1 | `last_gate_failure` set; fix prompt gets the gate output |
| gating | gates failed, rounds used, source has a worker | ready, `stopped` = `rounds-exhausted` | the gate failure is recorded as unresolved; `finish` opens a draft, `resume --rounds N` can continue |
| gating | gates failed, source has no worker (pr) | gating `gates-failed` | stays; `task gates` re-runs |
| reviewing `narrow` | review(0) | reviewing, scope `full` | the final full review |
| reviewing `full` | review(0) | ready | |
| reviewing | review(n > 0, valid must-fix repeat) | ready, `stopped` = `repeat-finding` | the draft lists the open findings (below) |
| reviewing | review(n > 0), `round` >= `fix_rounds` | ready, `stopped` = `rounds-exhausted` | same |
| reviewing | review(n > 0), rounds left | fixing, `round` + 1 | fix prompt gets the recorded review |
| reviewing | reviewer failed | reviewing `failed` | `task review` or `resume` retries |
| fixing | fix done or timed-out, commits collected and verified | gating | run gates |
| fixing | fix done or timed-out, collection failed | fixing `failed` | clone intact; `resume` retries the collection |
| fixing | fix failed | fixing `failed` | `resume` retries |
| any `running` | interrupted | same stage, `running` flagged interrupted | `resume` continues it |
| `working`/`fixing` `completed`, `gating` `passed`, `reviewing` `completed` (not `running`, nothing drove it on) | resume | the stage that follows | replay the stored result (re-apply `worker done`, `gates passed`, `review(n, repeat)` from the recorded review) |
| ready with `stopped`, source has a worker | resume(+N), N >= 1 | fixing, `fix_rounds` + N, `round` + 1, `stopped` cleared | fix prompt gets the recorded review (or the gate output for a gate stop) |
| ready with `stopped` = `repeat-finding` and rounds left, source has a worker | resume (N = 0) | fixing, `round` + 1, `stopped` cleared | the first review after it ignores repeats of findings that already existed (no immediate re-stop) |
| ready, issue or spec source | finish | finished | sink, see 5.4 |
| ready, pr source | finish | refused | review only; `ready` ends the task (only `rm`) |
| finished | any event except rm | refused | the way on after a published draft is decision 169: `file-findings`, then issues that continue the PR branch; `resume` does not reopen it |
| any not live | rm | removed (see 5.4: a spec task whose result is only in `repo.git` is refused without `--force`) | |

Rows are checked top to bottom and the first match wins. A validation test walks every reachable (state, event)
pair and asserts that it matches exactly one row, so an accidental overlap fails the build.

**Issue 142 status:** a spec task's `prepared` through `ready` rows are built, but through the section 5.2 "Today
(generated)" mechanics (the `HAS_WORKER` kind list covers `Kind::Issue` and `Kind::Spec` alike), not this target
table's `resume`/`round`/`repeat` machinery, which is still unbuilt for every source. The `ready, issue or spec
source | finish | finished | sink, see 5.4` row above is built for a spec source by issues 143 (the `local` sink) and 144 (the `push`
sink), as its own `(spec)` row in the "Today (generated)" table.

`resume` is refused while the recorded process is alive (pid and start time, as `interrupted` is decided today).
`--rounds N` is additive ("N more rounds", N >= 1; 0 is refused) and is valid from any `ready` with `stopped`,
including `repeat-finding` and `repeat-gate-failure` before the budget is used; a successful resume clears `stopped`
and a later stop sets it again. Resuming an interrupted or failed `fixing` does not add 1 to `round` again (it was
counted on entry); plain `resume` on `rounds-exhausted` is refused (N >= 1 is required); `fix_rounds` is the value
recorded at start, so a changed `sbxm-task.toml` affects new tasks only. A PR source has no worker and no fix
rounds: its `fix_rounds` is 0, so a failed gate or a finding ends the task `ready` (findings) or `gates-failed`
(gates) without a `stopped` reason, and `resume` is refused for it.

### 5.3 The no-progress rule

After a review with n > 0, the task stops (`ready`, `stopped` = `repeat-finding`) if any **must-fix** finding carries
`Repeat of: <id>` where `<id>` is one of the task's `open_findings` (from any earlier review, not only the last one:
a narrow review between two full ones must not hide a finding that keeps coming back) and the two are must-fix
findings that share a file path (from `Where:`, line numbers ignored). A repeated should-fix finding or note never
stops a task. The review prompt for rounds 2 and up includes the open findings so the reviewer can say so. Each
round's review is kept as its own file (`reviews/round-<k>.md`, with `review.md` the latest), and finding ids are the
reviewer's `M-<n>` and `S-<n>` of that round, recorded with the round number so an id names one finding. The id and
file checks are the guard: a claim naming an id that is not open, or a different file, counts as a new finding and
adds a warning, so a wrong claim alone cannot stop a task. The first review after a `resume` ignores repeats of
findings that already existed when the task resumed, so a resumed task is not stopped again at once.

A gate failure is a repeat when the same command fails again after a fix round (`last_gate_failure`: command and
exit code, not the output text); the task stops with `stopped` = `repeat-gate-failure`, a separate reason because
no reviewer finding is involved. A different failing command is progress.

### 5.4 Sources and sinks

| Source | Input copied into the task folder | Review comment | Sink at `finish` |
|---|---|---|---|
| issue | `issue.md` | PR comment (once a PR exists) | branch pushed, PR opened (draft if `stopped` or must-fix left) |
| pr | `issue.md` of the linked issues + the PR | PR comment | none (review only) |
| spec | `source.md` (built, issue 142) | `review.md` only (built, issue 142) | `[finish] sink`: `local` (default: the branch stays in `repo.git`; `finish` prints how to fetch it; built, issue 143) or `push` (pushed to `origin` as a new branch, no PR; built, issue 144) |

For an issue task tied to an open PR (decision 169), `finish` pushes to the PR's own branch and cannot make it a draft:
it posts the remaining findings as a PR comment and leaves the PR's draft status alone.

Safety of the spec sinks (confirmed by the user, 2026-10-07; the fetched rule is decision 178): `finish` with the `push` sink refuses a spec task
that has `stopped` or open findings unless `--push-unresolved` is given, because a spec task has no PR and no draft
to mark it unfinished; `push` never force-pushes, and a branch-name collision is refused, while a branch already at the task's own commit is only recorded
(5.4a; built, issue 144).
`task rm` refuses a spec task whose result exists only in `repo.git` (the `local` sink, not yet fetched), because
`repo.git` lives in the task folder, and needs `--force` to delete it (built, issue 143). "Not yet fetched" means:
the task's branch has commits beyond its base, and its tip commit isn't in the git checkout `task rm` runs from
(`git cat-file -e <tip>^{commit}` there, a read in the user's own checkout; nothing runs in the agent's clone). This
holds at any stage, not only `finished`, and the refusal comes before the confirmation question. A result fetched
into a different clone still needs `--force`.

The git trust boundary (decision 159) is unchanged: the host only fetches a verified bundle into `repo.git`, and
never runs git inside the agent's clone, whatever the source.

### 5.4a Actions that leave the machine are idempotent

`resume` must not repeat an action that already happened outside the task folder. Each such action is recorded in
`task.json` as intent before it runs and as result after, and `resume` reconciles with the outside world before
retrying:

| Action | Reconcile on resume |
|---|---|
| push the branch | compare the remote branch head with the local one; push only if they differ |
| open the PR (draft or normal) | look up an open PR for the head branch; reuse it instead of opening a second |
| post the review comment | find the comment carrying a marker line with the review's SHA-256; skip if present |
| record `finished` | written after the PR exists; a crash before it finds the PR and records it |
| collect commits after a worker or fix | verify the bundle against the clone; collect only if the commits are not in `repo.git` |

`resume` continues the incomplete action; it does not rerun the whole stage.

### 5.5 Migration (decision 173(f))

1. **Spike:** add the table beside the current code (`Stage::next/statuses/done` grow into it) and a test that
   the old `check_can_*` guards agree with it on every (state, event) pair; the differences are the T1-T9 gaps.
2. Move one phase at a time into a driver that looks up the table; the existing tests stay green at each step.
3. **Done (issue #116):** `sbxm task states` and the section 5.2 "Today (generated)" table are generated from
   the code; `tests/task_states.rs` compares them with this file and fails when they differ.
4. Then add `resume`, `fix_rounds`, `Repeat of:` and the `spec` source as new rows (the 5.2 "Target" table above),
   each with tests that fail first. **Issue 142 (part 7a) did the `spec` source's `prepared`-to-`ready` stages
   ahead of this step, through the existing mechanics (section 2); `finish`'s sink row is left for the two issues
   that follow it; issues 143 and 144 added it for the `local` and `push` sinks.**

Related: `sdlc/evals-workflow-notes.md` G16, G17, G24, G25; decisions 116, 154, 159, 169.
