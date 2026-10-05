# Task state machine: states, operations, gaps (draft, 2026-10-05)

**Status (updated 2026-10-05): the interview is done and its six answers are decision 173 (section 4 below);
sections 1-3 still describe today's code. The target design (one transition table with rounds and a pluggable
source, section 5) is still to be written.** It rebuilds a table the user remembers making on 2026-10-03 and that
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

## 5. Target design (to write)

Open for the spec: the transition table (events, guards, actions), where the round counter and the source live in
`task.json`, the no-progress rule, whether `resume` can extend the round budget (`--rounds N`, proposed), how a spec or
local-repo source fits the git trust boundary (decision 159), and the migration steps (spike first, after PRs 102/103).

Related: `sdlc/evals-workflow-notes.md` G16, G17, G24, G25; decisions 116, 154, 159, 169.
