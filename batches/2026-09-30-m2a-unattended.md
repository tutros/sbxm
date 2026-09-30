# Unattended batch: M2a slices 6-12b (2026-09-30)

Written at the user's request ("run whichever slices you can unattended; define success criteria; don't run in an
endless loop"). This is the in-session batch form of `sdlc-implementation` rules 5 and 8, made explicit. The log at the
bottom is updated after every slice.

## Why

Slice 5a is done, so the chain 6 -> 7 -> 8 -> 9 -> 10 -> 11 is unblocked, and 12a/12b follow. The user is away, so every
choice below is pre-decided; if a choice can't be, the slice stops and is recorded as Blocked with the question.

## Scope

Attempted, in this order, one at a time: **6, 7, 8, 9, 10, 11, 12a, 12b**.

Not attempted, and why:
- **12 (cosine):** needs the new dependency `fastembed` (ONNX runtime plus a model download); a new dependency is a stop
  condition. Its result (`evals.json` field) is not needed by 12a.
- **13 (end-to-end check):** manual, real `sbx`, the user's.
- **14 (Jev):** blocked on spike S9.

A slice depends on the previous ones: if one stops, the batch stops there. Nothing is skipped past a stopped slice.

## Environment facts (verified this session unless marked)

- Branch `m2a-implementation` (pushed); slices 0-5a done. Windows, PowerShell/Git Bash. `sbx` 0.43.0 logged in; secrets
  stored: `anthropic`, `openai`. **No `google` secret, and Antigravity can't authenticate in a fresh sandbox** (decision
  124f, open), so every real check uses Claude and Codex only; Antigravity is covered by fake-backend tests.
- Real-`sbx` tests use `SBXM_REAL_BASE_DIR=E:\sbxm-it` (never `C:`; decision 56). Models: `claude-haiku-4-5-20251001`,
  `gpt-5.6-luna` with tiny prompts.
- The Bash tool collapses double backslashes (a hook blocks them); write such files with Write/Edit.
- Permission denials by the auto-mode classifier are not to be worked around (see stop rules).

## Success criteria

**Per slice (all must hold before its commit):**
1. The slice's row in `milestone-2.md` ("Test focus" column) is covered by tests that were seen failing first for the
   expected reason, and now pass.
2. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --no-fail-fast` all pass
   (evidence: the counts, "N ok, 0 failed").
3. If the slice touches `sbx`: its `#[ignore]` real test passes once (one fix and one re-run allowed), leaves no
   sandbox (`sbx ls --json` lists only pre-existing ones) and no folder under `E:\sbxm-it`.
4. Docs in step: `AGENTS.md` layout, `README.md` if user-visible, `decisions.md` for details the plan left open.
5. Committed (imperative subject, `Co-Authored-By` line) and pushed to `origin/m2a-implementation`. No merge, no PR,
   nothing on `main`.
6. The log below has the slice's commit, test counts and the **manual test steps for the user**.

**Batch done when:** all eight slices meet the per-slice criteria, or a stop rule fires (then the log says which slice,
which rule, the evidence and the exact question for the user). Either way the batch ends; nothing retries by itself.

## Pre-decided judgment calls

- Details the plan left open (like decision 125) are recorded as new numbered decisions in `decisions.md` and listed in
  the log. Anything that contradicts an existing decision, changes security posture, adds cost, or is a preference only
  the user can give is a **stop**, not a decision.
- Real checks use 2 sandboxes (Claude, Codex) where the plan says 3, because Antigravity can't authenticate (124f).
- Run IDs are `<date>-<6 hex>` per P2; results live under `<base>/.sbxm/runs/<run-id>/` and the mounted workspace
  under `<base>/runs/<run-id>/<contestant>/<repeat>/` per the plan.

## Budget

- Per slice: at most 3 red->green cycles per test group; a real test at most twice (one re-run after a fix).
- Real model calls: at most 12 per slice, tiny prompts, cheap models. At most 4 sandboxes alive at once.
- Whole batch: at most the 8 slices above. No open-ended polling: background jobs are waited on once.

## Stop rules

Stop the whole batch, record it in the log, and leave the tree clean and committed (or stashed nowhere: uncommitted
work is either committed on green or reverted to the last commit), when:
- the same error happens twice after a fix;
- a slice needs a new dependency, a new decision that contradicts an existing one, or a departure from its plan row;
- a real-`sbx` check fails after its one re-run, or anything would touch a path outside temp dirs and `E:\sbxm-it`;
- a slice grows beyond its plan row (more than about one screen of new production code per red->green step is split;
  needing a new module the plan doesn't name is a stop);
- a tool call is denied by the permission classifier (never worked around);
- a check only the user can do is next (interactive attach, browser sign-in).

Forbidden: `git push --force`, pushing to `main`, opening PRs, `sbx rm` of a sandbox not created by this batch,
changing `sbx` settings or secrets, `gh` writes, editing `.claude/settings*.json`.

## Side effects

Writes only: this repo's working tree on `m2a-implementation`, `E:\sbxm-it` (temp dirs, deleted after), the system temp
dir. Sandboxes created by this batch are named `sbxm-it-<pid>-*` or `sbxm-run-*` and are removed by the tests.

## Cleanup (always, including after a failure)

`sbx ls --json` shows only `sbxm-sbxm-m2-claude`; `E:\sbxm-it` is empty; `git status` is clean.

## Log

| Slice | Status | Commit | Tests (ok/failed binaries) | Real check | Notes |
|---|---|---|---|---|---|
