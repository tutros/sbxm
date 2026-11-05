# Spec: the unattended blockers, #402, #403 and #404 as one task

Decision 286's first group (an experiment, the 2026-11-04 retro): three issues that touch the same code and are to
be done in order go to one worker as one spec, with every issue's acceptance checks, one review loop and one PR.
The issues' bodies are copied below in full (the worker can't read GitHub); their open points were answered with the
user on 2026-11-03 and nothing is left to decide.

## How the three fit together

- **Order:** #402 first, then #403, then #404. #402 and #403 share one stop: a task whose own branch changes
  `Cargo.lock` under a strict profile stops with a clear message and a "Needs a decision" hand-back. Build that
  stop once, in #402's part, and have #403's part reuse it (prompt text and the hand-back wording).
- **Decision numbers, reserved now (gap G41, #404's rule (a)):** #402 adds **decision 290** (refines 203(b): the
  automatic rebuild from `main` and the branch-changed-lock stop); #403 adds **decision 291** (a worker never adds a
  crate; it hands back, and the user approves every new crate); #404 adds **decision 292** (the "merge the base"
  step in `task fix --pr N` and the two conflict-avoiding rules). Use exactly these numbers.
- **New tests go in files of their own** (#404's rule (b)), e.g. `tests/template_auto_rebuild.rs`,
  `tests/task_crate_handback.rs`, `tests/task_fix_merge_base.rs`, never appended at the end of a shared test file.
- **Commits:** at least one commit per issue, each message naming its issue; the last commit's message ends with
  `Fixes #402`, `Fixes #403` and `Fixes #404`, one per line.
- **Done means:** every acceptance point and boundary case of all three issues has a named test (or, where a test
  can't reach it, says so under "Not checked here" in `result.md`), plus `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` passing. `README.md`, `docs/workflow.md` and the decisions are
  updated in the same change.

### #403's boundary cases, with the expected outcome

#403 lists four cases; each makes the branch's `Cargo.lock` differ (a direct dependency, a dev-dependency, a
version bump and a crate with a build script all change the root package's entry or add packages), so in each the
worker doesn't make the change and the task stops with the hand-back naming the crate, its version and why it is
needed. A test per case checks that the branch-changed-lock stop fires on such a lock diff.

## Issue #402: Strict tasks stop after a Cargo.lock change until a template is rebuilt by hand (adds decision 290)

**Size:** M (confirmed 2026-11-03): one automatic rebuild path from a clean `main` checkout before sandbox creation, plus a stop for a branch-changed lock; reuses `template build`, its build lock and tag cleanup. Needs a numbered decision refining 203(b).

### Why

Blocker for unattended runs (the user, 2026-11-03). A `Cargo.lock` change makes every warm strict template stale (decision 203(b)). Each strict sandbox is then refused until someone runs `template build --warm` by hand for each harness. The `task loop` (#320) would stop at the first such task.

### What happened (2026-11-03)

- PR 393 (#320) adds the `ctrlc` crate, so its `Cargo.lock` differs from the warm codex template's. `task review --pr 393` was refused as stale.
- The workaround `--profile dev` (not strict) failed at the gates: cargo exit 127, no toolchain in that profile. The review is still blocked.
- Once PR 393 merges, `main`'s lock differs from both warm templates (claude and codex). Every strict task after it is refused until both are rebuilt by hand.

### Answered (the user, 2026-11-03; no open points left)

1. **Rebuild automatically only from `main`.** A warm build runs `cargo` (build scripts included) over the lock it is given, in a build sandbox with network, and the result becomes the template of every later sandbox. Warming from agent-written code (a worker's tip, an unmerged PR head) could plant files in the cache that every later task inherits. So:
   - When `main`'s `Cargo.lock` differs from a strict profile's warm template, sbxm rebuilds the template warmed from a clean checkout of `main`'s tip (host-owned, as `task::repo` makes them), then goes on.
   - When a task's **own branch** changes the lock (a worker adding a crate, or a PR like 393), it **stops with a clear message** and the loop moves on. How a new crate gets approved is #403.
   - Note: on 2026-11-03 the session warmed the codex template from PR 393's branch by hand. That was low risk (one known crate, added by the session), but it isn't a safe default.
2. **Warmed from:** always `main`'s tip at the moment the stale lock is noticed (follows from 1).
3. **When:** lazily. The first task that needs the template finds it stale, rebuilds it, then goes on. There is no merge watcher. Measured once (2026-11-03): a warm codex build took about 4.5 min, start to finish. Two harnesses is roughly 9-10 min (an estimate) per merged lock change.

### Boundary cases (decision 244)

- Lock unchanged: no rebuild.
- Changed by one line: a rebuild.
- Changed only in line endings: no rebuild (decision 238).
- Two tasks needing the same rebuild at once: one build, the other waits on the build lock.
- A task whose own branch changes the lock while `main` matches the template: no rebuild, the task stops with the message.
- `main` changed the lock and the task's branch changes it again: rebuild from `main`, then the task stops for its own change.
- A rebuild that fails: the task stops, with the `template build` command named.

Related: decisions 200, 203, 216, 235; #320; the gap "a worker can't add a dependency" (filed alongside).

## Issue #403: A worker can't add a crate under a strict profile (gap G1) (adds decision 291)

**Size:** S (changed from M, 2026-11-03): option (a) is prompt text plus the branch-changed-lock stop that #402 also needs; build it with or after #402.

### Why

Blocker for unattended runs (the user, 2026-11-03). A worker in a strict sandbox can't add a crate: there is no crates.io access at runtime (decision 200). Gap G1 in `sdlc/evals-workflow-notes.md` covers it, and decision 203 left it out of milestone 3. Today the session adds the crate by hand.

### What happened (2026-11-03)

#320's Ctrl-C support needed the `ctrlc` crate. The worker couldn't fetch it, so the session added it by hand with the user's OK (commit cf4750a on PR 393). That change then made the templates stale (see the stale-template issue filed alongside).

### Answered (the user, 2026-11-03; no open points left)

1. **The worker doesn't add the crate.** It stops with a "Needs a decision" hand-back (decision 283) naming the crate, the version and why it's needed. The task parks and the loop moves on. Options (b) and (c) are rejected:
   - (b) would warm a template from agent-written code, which #402's answer rules out.
   - (c) per-task crates.io egress would weaken strict mode's promise (decision 218).
2. **The user approves every new crate.** The crate goes onto `main` in a small PR the user merges. #402's automatic rebuild then refreshes the template, and the parked task is resumed. Revisit (session approval within limits, such as an existing indirect dependency or a dev-only crate) if these hand-backs pile up.

What the change needs: the worker prompt says what to do when a crate is missing (stop, hand back, don't edit `Cargo.toml`/`Cargo.lock`). A gate or check refuses a branch that changes `Cargo.lock` under a strict profile, with the hand-back's wording; it overlaps #402's stop and should share it.

### Boundary cases (decision 244)

- A crate already in `Cargo.lock` as a transitive dependency but not a direct one.
- A dev-dependency only.
- A version bump of an existing crate.
- A crate with a build script.

Related: G1, decisions 200, 203, 218, 283; #220.

## Issue #404: Open PRs conflict with each other once one merges; each needs a hand merge (adds decision 292)

**Size:** M (confirmed 2026-11-03): a merge-the-base step in `task fix --pr N` with its narrow review, plus two prompt/skill rules.

### Why

Blocker for unattended runs (the user, 2026-11-03). Several PRs open at once conflict with each other as soon as one merges. Each conflict needs a hand merge, a full local build and test (about an hour on this machine), and a re-review of the merge (decision 251). Last night's queue left 6 open PRs; 3 of them conflicted during the next morning's merges.

### What happened (2026-11-03)

| PR | Conflicted after | Files | Kind |
|---|---|---|---|
| 394 (#379) | 391 merged, then 396-399 | `tests/task_review_pr.rs`, `README.md`, `docs/workflow.md`, `sdlc/decisions.md` | both sides appended tests at the same file end; one README row edited on both sides; decisions 282 and 283 appended at the same place |
| 401 (#60) | 399, then 400 | `README.md`, `tests/task_finish_cmd.rs` | one README row edited on both sides; tests appended at the same end |
| 393 (#320) | 391 and later | not yet resolved | |

Every conflict was mechanical: keep both sides whole, or merge a README row word by word. None needed judgement about behavior.

### Answered (the user, 2026-11-03; no open points left)

1. **A worker resolves.** `task fix --pr N` (#313) gains a "merge the base" step: merge `main` in the sandbox, resolve, run the gates, push, then an automatic narrow review of the merge (decision 251). Evidence, 2026-11-03:
   - 5 hand merges, about 1-1.5 h each, mostly the local Windows build.
   - Every conflict was mechanical.
   - One (PR 394 with PR 400's lock) broke two tests, which the gates caught.
2. **Fewer conflicts at the source,** as rules in the worker prompt and the `sdlc-issue-flow` skill:
   - (a) **Reserve the decision number when the issue is readied:** the issue body says "this adds decision N" (gap G41). The session adds that line while answering the open points.
   - (b) **New tests go in a section or file of the issue's own,** never appended at the end of a shared test file.
   - README rows edited by two PRs: no rule; they merge word by word.
3. **Order:** the loop doesn't avoid tasks whose files overlap an open PR's. **Trigger** to build it: a worker's merge of the base fails its gates twice in one week.

### Boundary cases (decision 244)

- A clean merge: no fix round.
- A conflict only in docs.
- A conflict whose resolution breaks a test: the gates catch it, and it gets a fix round.
- A PR whose base moved twice before its merge.

Related: #313, #320, decision 251, G41.
