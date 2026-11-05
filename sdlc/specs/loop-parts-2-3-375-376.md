# Spec: `task loop` parts 2 and 3, #375 and #376 as one task

Decision 286's second group (an experiment, the 2026-11-04 retro): two issues that touch the same code are built by
one worker as one spec, with every issue's acceptance checks, one review loop and one PR. The issues' bodies are
copied below in full (the worker can't read GitHub); their open points were answered with the user (2026-11-02, and
#376's state source changed on 2026-11-04) and nothing is left to decide.

## How the two fit together

- **Order:** #375 first (arming auto-merge, in `src/commands/task_loop.rs` and a new `GitHubBackend` call), then
  #376 (a Loop section in `sbxm status`, under `src/status/`). Both use the loop's count of PRs awaiting the user
  (`task_loop.rs`, the cap of 5): #376 shows what #375 changes, so share one function for "awaits the user" rather
  than two copies.
- **Decision numbers, reserved now (gap G41):** #375 adds **decision 293**, #376 adds **decision 294**. Use exactly
  these numbers.
- **New tests go in files of their own** (decision 292's rule, #404), e.g. `tests/task_loop_arm.rs` and
  `tests/status_loop.rs`, never appended at the end of a shared test file.
- **Commits:** at least one commit per issue, each message naming its issue; the last commit's message ends with
  `Fixes #375` and `Fixes #376`, one per line.
- **Done means:** every acceptance point and boundary case of both issues has a named test (or, where a test can't
  reach it, says so under "Not checked here" in `result.md`), plus `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings` and `cargo test` passing. `README.md`, `docs/workflow.md` and the decisions are updated in the same
  change.

## Issue #375: task loop part 2: arm auto-merge on low-risk loop PRs (decisions 266, 280) (adds decision 293)

**Part 2 of 3** of #320 (`task loop`), split 2026-11-02 by the user.

**What:** `task loop` arms GitHub's auto-merge on a low-risk PR it opened, under decisions 266 and 280: the risk after the path-rule floor is low, the in-task review covered the PR's head with 0 must-fix, and it runs `gh pr merge <N> --auto --merge --match-head-commit <reviewed sha>`. Medium and high risk, and drafts, wait for the user. A merged PR is cleaned up with `task rm --merged`. This is the "software factory" step the user named on 2026-11-02 (the session arms auto-merge until this lands; #317 decision 2).

### Decisions (the user, 2026-11-02)

1. **No CI check before arming:** rely on GitHub's `--auto`, which waits for the required checks (including `sbxm/review` once #317 lands). No `gh pr checks` polling.
2. **Only `task loop` arms**, for now. `task finish` and `task review --pr` don't; for a PR made by hand the session arms under decision 280.

### Boundary cases (decision 244; tests first, on `FakeGitHub`)

- [ ] Risk exactly low after the path-rule floor: armed with `--match-head-commit <reviewed sha>`.
- [ ] Risk medium (the reviewer's level, or raised by a path rule): not armed; counted toward the cap of 5.
- [ ] A draft PR (must-fix left): not armed.
- [ ] The head moved after the review: not armed; the loop log says why.
- [ ] A PR already armed: left alone.
- [ ] The arming call fails (`gh` error): a warning; the PR counts toward the cap; the loop goes on.
- [ ] A PR GitHub merged: the next loop step cleans it up with `task rm --merged`.

**Size:** M (confirmed 2026-11-02 once the open points were answered): one arming step and its refusals on a new `GitHubBackend` call (`gh pr merge --auto`), plus `task rm --merged` after a merge, with fake-GitHub tests.

**Depends on:** #320 (part 1), #317 (`sbxm/review` status).



### Group (decision 286, the user 2026-11-04)

Built with #376 as one spec task (`sdlc/specs/loop-parts-2-3-375-376.md`), order #375 then #376, one PR closing both. This issue adds **decision 293** (reserved, gap G41): only `task loop` arms auto-merge, on the conditions above. Group size about L (M + M). Checked against the code on 2026-11-04: `#317`'s `sbxm/review` status is merged; `GitHubBackend` has `pr_merge_state` (reads `auto_merge`) but no call that arms it yet.

## Issue #376: task loop part 3: the loop's queue, running task and awaiting PRs in sbxm status (adds decision 294)

**Part 3 of 3** of #320 (`task loop`), split 2026-11-02 by the user.

**What:** a Loop section in `sbxm status` (decision 171): whether a loop runs, the running task and its stage, the queue (labeled issues in the order the loop takes them), and the PRs awaiting the user (the cap count of #320 decision 3), read without calling `sbx`.

### Decisions (the user, 2026-11-02)

1. **Changed 2026-11-04 (the user chose option (a) while readying the group):** state comes from the loop's lock plus the task records, no new state file and no new writes. The lock (`<base>/.sbxm/task-loop.lock`, today only the process identity) says whether a loop runs: a lock whose process is gone (the same pid and start-time check as `run.log`, decision 163) shows as not running. The running task and its stage come from the issue task's `task.json` that is still running (`status == running`) and whose recorded `process` equals the lock's process. **Changed 2026-11-06 (the user approved this rule after review M-2):** the marker (`hooks.task_loop`, `LOOP_HOOK`) is written only when the loop claims a ready task for `task finish`, so it can't identify the task being worked on; the process match replaces the earlier "carries the marker" rule, and the marker keeps its one meaning (the loop finished this task; its PR counts toward the cap). Decision 294 and the README record this as the user's decision, not as provisional. The tasks done in the window are the marked records finished since the lock's process started. The loop's log (`sdlc/batches/<date>-loop.md`, in the repo checkout) is not read: `status` reads only the base folder. *(Was, 2026-11-02: the lock would hold the current issue and stage, and the log would give the tasks done.)*
2. **The queue needs a GitHub call: at most one per minute**, kept in memory between `--watch` refreshes; a plain `status` calls once. A failed call shows "queue: unreadable (<first line of the error>)" and the rest of `status` still shows, as the cloud section does.

**Later (the user, 2026-11-02):** a queue that feeds the loop its next task, for continuous looping day and night, will be looked into separately; this section reads the label queue until then.

### Boundary cases (decision 244; tests first, golden like the Tasks section)

- [ ] No lock file: "loop: not running".
- [ ] A stale lock (its process gone): "not running", naming the stale lock.
- [ ] A queue of 0: "queue: empty".
- [ ] More issues than fit the terminal: the first lines and "… N more", never a line cut mid-way on a terminal (the Tasks section's rule).
- [ ] The first refresh, then 59 s and 61 s later: one GitHub call, then a second only after 60 s.
- [ ] The GitHub call fails: "queue: unreadable", the rest of `status` shows.
- [ ] A non-terminal (piped output): nothing is cut.

**Size:** M (confirmed 2026-11-02 once the open points were answered): a new section in `src/status/`, golden-tested like the Tasks section, the lock/log read and a one-minute cache of one GitHub call.

**Depends on:** #320 (part 1).

### Group (decision 286, the user 2026-11-04)

Built with #375 as one spec task (`sdlc/specs/loop-parts-2-3-375-376.md`), order #375 then #376, one PR closing both. This issue adds **decision 294** (reserved, gap G41) recording the state source above. Group size about L (M + M).
