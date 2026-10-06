# Spike: the task state machine as a table (2026-10-06)

Decisions 173 and 174, spec `sdlc/specs/task-state-machine.md` section 5.5 step 1. Question: can the section 2 and 5.2 rules be
written as one table in code, and does that table agree with the guards the pipeline uses today?

**What was built (additive, no command changed):** `src/task/machine.rs` has `Row`, `TABLE` (21 rows: the pipeline's moves,
`gates`, `review` for issue and PR tasks, `finish`, `rm`), `verdict(state, event)`, `states(kind)` and `dead_ends(kind)`.
`tests/task_machine.rs` compares the table with `check_can_gate`, `check_can_review`, `check_can_review_pr`,
`check_can_finish`, `plan_removal` and `Record::advance` for every state and event. `Stage::next/statuses/done` became
`pub(crate)`.

A state here is `(stage, status, interrupted, kind)`: 23 states for an issue task (16 stage/status pairs, each `running`
one counted alive and interrupted) and 13 for a PR task (it has no worker, so never `working` or `fixing`). There are 11
events: `gates`, `review`, `finish`, `rm` and `advance(to)` for each of the 7 stages; `gates` is not asked of PR tasks
(`task gates` takes only `--issue`).

## What agrees

Every user operation. `gates`, `review` (issue and PR), `finish` and `rm` give the same answer from the table as from the guards
in every state, so the section 2 matrix is an accurate description of the code and the guards could be replaced by lookups
without changing a single verdict.

## What differs (10 pairs, 3 findings, all in `Record::advance`)

| # | Difference | Class |
|---|---|---|
| F1 | `advance` lets an issue task go `prepared -> gating` or `prepared -> reviewing`, skipping its worker. The table refuses. | surprise: `advance` ignores the task kind; only the pipeline's call order keeps issue tasks on the worker path |
| F2 | `advance` lets a PR task go to `working`, `fixing` or `finished`. The table refuses. | surprise, same cause. `check_can_finish` is what stops a PR task finishing; nothing stops the other two |
| F3 | The code lets a PR task go `prepared -> reviewing` (spec section 1 says "straight to review"); spec 5.2 lists only `prepared -> gating (pr)`. | a bug in the spec: add the row |

## Dead ends: the gaps fall out of the table

`dead_ends` lists the states that are not the end of a task's life, not in flight (a running task whose process is alive)
and from which no command moves it on, only `rm`. For an issue task:

| State | Spec gap |
|---|---|
| `prepared/running`, interrupted | T9 |
| `prepared/failed` | **not listed in the spec (F4)**: only `rm`/`--restart` |
| `working/running`, interrupted | T9 |
| `working/failed` | T6 |
| `reviewing/running`, interrupted | T3, T9 |
| `reviewing/completed` | T4 |
| `fixing/running`, interrupted | T5, T9 |
| `fixing/failed` | T5 |

For a PR task: `prepared/failed` (F4), `gating/running` interrupted (T9; a PR task has no `gates` command, and `review`
refuses it), `reviewing/running` interrupted (T3) and `reviewing/completed` (T4).

So T3, T4, T5, T6 and T9 are exactly the dead ends, and a missing `resume` row for each is what closes them. The test pins
both lists, so adding the `resume` rows will make it fail until the lists are updated, which is the point.

## What the table cannot express yet

- **T1** (`ready` with must-fix left) and **T2** (`gates-failed` with no fix): the state has no must-fix count, round counter
  or `stopped` reason, and there is no `fix` event for a failed gate. These are the new fields and rows of spec 5.1 and 5.2.
- **T7, T8**: they depend on other tasks and on a PR changing after a review; they are not facts about one task.
- Actions: the table says whether a move is allowed, not what it does (run the worker, post the comment). The driver still has
  to map a row to its action; that mapping is the real work of migration step 2.
- Guards that look outside the record (`task_folders`, the readable `task.json`, the schema check in `plan_removal`) stay
  in code; the test builds a real folder so only the state decides.
- `check_can_finish`'s "already has a PR" check is redundant with the `finished` stage, so the table has no `pr` column. A record
  with `pr` set in another stage is an inconsistency, not a state.

## Verdict

Workable. The table is small (21 rows), reads like the spec, and agrees with every user-facing guard; the differences are in the
one place the code never enforced the task kind. For migration step 2, move one operation at a time:

1. Replace each `check_can_*` with a `verdict` lookup plus the refusal text for the state (the messages stay in code, keyed by
   state); the test keeps proving no verdict moves.
2. Give `advance` the kind (F1, F2) when a guard moves, and add the F3 row to the spec.
3. Add the new fields and `resume` rows only after that, each with a test that fails first; the dead-end test will say when a gap closes.
4. Generate the section 5.2 table in the spec from `TABLE` (step 3 of 5.5); the F3 omission is the kind of drift that would stop.

Open for the user: whether F4 (`prepared/failed`) should get a retry row or stay `rm` only.
