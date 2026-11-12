# Milestone 5: the loop runs local and cloud tasks at once

Issue #561. Decisions [317] (what) and [321] (plan choices). Planned 2026-11-11.

## 1. Goal

`sbxm task loop` keeps up to `sandbox_slots` local and `cloud_slots` cloud tasks running at once, each end to end (worker, gates, review, fix rounds, finish), as child `sbxm` processes it supervises. On this machine (16 GB, `sandbox_slots = 1`): one local and two cloud tasks at once. Deferred: a per-provider rate cap and a money cap (each waits for its trigger, [317](10), (11)); 3 cloud tasks at once is unmeasured.

## 2. Plan-level choices

All confirmed 2026-11-11 and recorded as decision 321: the plan file, the outcome file, `cloud_slots` unset, the loop's window, the rollout, and the slices running in the cloud.

## 3. Structure (dependency order)

- `src/config.rs` and `src/slot.rs`: a second slot pool for cloud sandboxes (`[resources] cloud_slots`), the same files-and-tickets mechanism as [304], under its own folder; cloud commands (`task run --cloud`, `task resume`/`fix`/`review`/`gates` of a cloud task) take a cloud slot where they take `Slot::none()` today.
- `src/commands/task_loop.rs`: location per task (labels `run-local`/`run-cloud`, then the first free slot, local first), passed in `task_run::Options.location` (today always `None`).
- A new supervisor in `src/commands/task_loop/` (for example `children.rs`): a `Spawner` trait (start a child command, poll it, wait for it, stop it) with a real implementation that runs the current `sbxm` executable with the task command, and a fake for tests; the child writes `outcome.json` ([321](2)); the loop reads it instead of matching error types and output text.
- The loop's pass becomes: start children while slots are free and work is ready; react to each child's end (finish under a finish lock, arm, row, notify); jobs, resolves and log-fixes first ([317](7)).
- `src/task/loop_log.rs` / `src/task/queue.rs`: the row gains a "Slot" column; rows in finish order.
- `src/task/select.rs` and the loop's `next`: a Related clash waits instead of being refused for the run.
- `src/task/pipeline.rs` / `src/task/gates.rs`: host-tier gates take the local slot while they run ([317](5)).
- Stop and restart: `--stop` drains; Ctrl-C stops the children; restart resumes each interrupted child task once.

## 4. Interfaces

- **Config** (`config.toml`): `[resources] cloud_slots = 2` (N >= 1; 0 is a config error naming the setting; unset: no limit by hand, and the loop runs no cloud tasks).
- **Labels:** `run-cloud` and `run-local` pin an issue's location; both on one issue is a refusal naming them.
- **`outcome.json`** (in the task folder; written last, atomically): `{"end": "...", "provider": "codex" | "claude" | ... | null, "detail": "..."}`.
- **Loop window:** one line per child event, `[<task> <local|cloud>] <event>`.
- **Loop log row:** existing columns plus "Slot".
- **`sbxm status`:** the Loop section lists every running child (task, slot, stage).

## 4b. Failure modes and trust boundaries

- A child that crashes or is killed writes no `outcome.json`: the task counts as interrupted and is resumed once ([245](d)); a second time parks it.
- An `outcome.json` that doesn't parse, or names an unknown kind: treated as `failed` with its text quoted, never trusted further; the child's record decides the stage.
- The loop crashes with children running: on Windows the children may live on; the restarted loop finds their tasks' live locks (#579's rule), doesn't start them twice, and adopts them as running until they end.
- Two children finishing at once: the finish lock serializes push, PR, arm ([317](8)).
- A usage limit in one child: no new task on that provider until it clears; the others go on ([317](10)).
- Cloud cost: bounded by `cloud_slots` only ([317](11)); `sbx` shows no cost.
- Windows: Ctrl-C reaches the console's children; a hidden scheduled loop has no console, so `--stop` (a file) is its only stop.

## 5. Work order (vertical slices; each its own issue, run in the cloud ([321](6)))

1. **`[resources] cloud_slots`** (M): the setting, the cloud slot pool, every cloud command taking one; `sbxm status` shows cloud slots. Done when two by-hand cloud runs with `cloud_slots = 1` run one after the other, and with it unset run together. Run by hand in the cloud.
2. **The loop picks each task's location** (S): labels, then local (one task at a time still); `run-cloud` runs in the cloud. Done when a `run-cloud` issue is run by the loop in a cloud sandbox. Run by hand in the cloud. After it, the rest run from the loop with `run-cloud`.
3. **Children, one at a time** (L): the `Spawner`, `outcome.json`, the loop reading outcomes; behavior unchanged. Done when 3 real local tasks give the same rows and outcomes as before.
4. **Several at once** (L): start children while slots are free; the 5-PR pause counts running tasks; the "Slot" column; one-line window events; `sbxm status` lists children. Done by the real check: `sandbox_slots = 1`, `cloud_slots = 2`, 3 ready issues at once.
5. **Finish lock and jobs as children** (M): resolves, log-fixes and queue jobs run as child jobs ahead of issues, one per PR at a time; one finish at a time.
6. **Related waits; per-provider usage pause** (M).
7. **Host gates take the local slot** (S).
8. **Stop and restart with children** (M): drain on `--stop`, Ctrl-C, restart resumes each interrupted child task once, adopts live ones.

Each slice's issue is readied with the user just in time (open points, boundary cases, size), before its worker starts.

## 6. Manual end-to-end checklist

1. `cloud_slots = 2`, `sandbox_slots = 1`; label 3 small issues `ready-for-worker` (one `run-local`).
2. Start the loop from the scheduled task; `sbxm status --watch` shows 1 local and 2 cloud children.
3. Each reaches a PR; low-risk ones auto-merge; the loop log has 3 rows with their slots.
4. `task loop --stop` during the run: no new start; the running ones finish; the loop exits.
5. Kill the loop mid-run; restart it: no task starts twice; interrupted ones resume once.
6. Unset `cloud_slots`: the loop runs one task at a time, as before.

## 7. Out of scope, and risks

- Out of scope: a per-provider cap and a money cap (triggers in [317]); more than 2 cloud slots until measured; parallel local sandboxes on this machine.
- Risk: slice 3 rewires the loop's core; mitigation: behavior unchanged, a real check on 3 tasks, `cloud_slots` unset as the off switch.
- Risk: provider limits with 3 workers; mitigation: [317](10)'s pause and the trigger for a cap.
- Risk: conflicts between parallel PRs; mitigation: [317](7)'s resolves, already automatic.
