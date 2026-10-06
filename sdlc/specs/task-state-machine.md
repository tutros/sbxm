# Task state machine: states, operations, gaps (draft, 2026-10-05)

**Status (updated 2026-10-06): both interviews are done; their answers are decisions 173 and 174 (sections 4 and 5);
sections 1-3 still describe today's code. Section 5 is the target design (one transition table with rounds and a
pluggable source), a draft for review.** It rebuilds a table the user remembers making on 2026-10-03 and that
was never saved to a file. It is derived from the code (`src/task/record.rs` `Stage::next`/`statuses`/`done`,
`check_can_gate`, `check_can_review`, `check_can_review_pr`, `check_can_finish`, `finish::plan_removal`), so it
describes what sbxm does today. Decision 169(f) ("a task's own `ready` state is not re-entered") is the choice this
draft questions.

## 1. States of an issue task (`stage` / `status`)

Legal moves (`Stage::next`): prepared -> working -> gating -> reviewing -> (fixing -> gating -> reviewing) -> ready -> finished.
A PR task (`pr-N`) has no worker: prepared -> gating -> reviewing -> ready.

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
| `source` | `issue <n>`, `pr <n>` or `spec <path>`; decides the id (`issue-<n>`, `pr-<n>`, `spec-<name>`), the input text copied into the task folder and the sink |
| `round` | fix rounds used so far |
| `fix_rounds` | the budget: `[worker] fix_rounds` (default 3), raised by `resume --rounds N` |
| `stopped` | absent, or why the task ended `ready` with must-fix left: `repeat-finding` or `rounds-exhausted` |

A review also records its `scope`: `narrow` (commits since the last review) or `full` (all commits).

### 5.2 The transition table (the code's single source; this table is generated from it)

Events: `worker` (done/timed-out/failed), `gates` (passed/failed), `review(n, repeat)` (n must-fix findings;
`repeat` = a finding the reviewer marked `Repeat of: <id>` whose file matches the earlier finding's), `fix`
(done/timed-out/failed), `interrupted` (a `running` state whose process is gone), `resume(+N)`, `finish`, `rm`.

| From | Event | To | Action |
|---|---|---|---|
| prepared | start | working (issue, spec) or gating or reviewing (pr) | run worker / gates / reviewer |
| prepared | preparation failed | prepared `failed` | `resume` retries preparation (decision 175) |
| working | worker done or timed-out | gating | run gates |
| working | worker failed | working (retry only by `resume`) | keep clone and commits |
| gating | gates passed | reviewing, scope `narrow` if `round` > 0 else `full` | run reviewer |
| gating | gates failed, `round` < `fix_rounds`, source has a worker | fixing, `round` + 1 | fix prompt gets the gate output |
| gating | gates failed otherwise | gating `gates-failed` | stays; `task gates` re-runs |
| reviewing `narrow` | review(0) | reviewing, scope `full` | the final full review |
| reviewing `full` | review(0) | ready | |
| reviewing | review(n > 0, repeat) | ready, `stopped` = `repeat-finding` | |
| reviewing | review(n > 0), `round` >= `fix_rounds` | ready, `stopped` = `rounds-exhausted` | |
| reviewing | review(n > 0), rounds left | fixing, `round` + 1 | fix prompt gets `review.md` |
| reviewing | reviewer failed | reviewing `failed` | `task review` or `resume` retries |
| fixing | fix done or timed-out | gating | run gates |
| fixing | fix failed | fixing `failed` | `resume` retries |
| any `running` | interrupted | same stage, `running` flagged interrupted | `resume` continues it |
| ready with `stopped` | resume(+N) | fixing, `fix_rounds` + N, `round` + 1 | fix prompt gets `review.md` |
| ready | finish | finished | sink, see 5.4 |
| finished | any | refused | new work is a new task (decision 169) |
| any not live | rm | removed | |

`resume` is refused while the recorded process is alive (pid and start time, as `interrupted` is decided today).
A PR source has no worker and no fix rounds: its `fix_rounds` is 0, so a failed gate or a finding ends the task.

### 5.3 The no-progress rule

After a review with n > 0, the task stops (`ready`, `stopped` = `repeat-finding`) if any finding carries
`Repeat of: <id>` and shares a file path (from `Where:`, line numbers ignored) with that earlier finding. The review
prompt for rounds 2 and up includes the previous review so the reviewer can say so; the file check is the guard,
so a wrong "repeat" claim alone cannot stop a task. A gate failure that repeats the same failing command output
counts the same way (to settle in the spec's tests).

### 5.4 Sources and sinks

| Source | Input copied into the task folder | Review comment | Sink at `finish` |
|---|---|---|---|
| issue | `issue.md` | PR comment (once a PR exists) | branch pushed, PR opened (draft if `stopped` or must-fix left) |
| pr | `issue.md` of the linked issues + the PR | PR comment | none (review only) |
| spec | `source.md` | `review.md` only | `[finish] sink`: `local` (default: the branch stays in `repo.git`; `finish` prints how to fetch it) or `push` (pushed to `origin`, no PR) |

The git trust boundary (decision 159) is unchanged: the host only fetches a verified bundle into `repo.git`, and
never runs git inside the agent's clone, whatever the source.

### 5.5 Migration (decision 173(f))

1. **Spike:** add the table beside the current code (`Stage::next/statuses/done` grow into it) and a test that
   the old `check_can_*` guards agree with it on every (state, event) pair; the differences are the T1-T9 gaps.
2. Move one phase at a time into a driver that looks up the table; the existing tests stay green at each step.
3. Generate `task states` and the section 5.2 table from the code; a test compares them with this file.
4. Then add `resume`, `fix_rounds`, `Repeat of:` and the `spec` source as new rows, each with tests that fail first.

Related: `sdlc/evals-workflow-notes.md` G16, G17, G24, G25; decisions 116, 154, 159, 169.
