---
name: sdlc-issue-flow
description: How one GitHub issue moves from open to merged and cleaned up in this project - ready the issue (open points, size, boundary cases, external facts), check the machine (disk, templates, secrets), run the sbxm task, the real check, the review loop and its stop rules, finish, merge or auto-merge, clean up, and record every promised follow-up on GitHub. Use whenever you take an issue (or a hand-made PR) through to merge, and as the checklist an orchestrator follows.
---

# Issue flow (decision 279, an experiment)

One page of stages, in order. Each names what to check, the command, and when to stop and ask. The other `sdlc-*`
skills hold the detail; this one only connects them. Report progress as "stage N of 9". `<sbxm>` is the recorded
build sbxm tasks run with (a machine's own wrapper; never a fresh `cargo run`).

## 1. Pick

- The user's order, never the newest issue unprompted. One task at a time (two local reviews at once ran out of
  memory); say which work could run in parallel, at most 3 sessions.

## 2. Ready the issue (before any worker)

- Every "to decide" point answered with the user, one at a time with a recommendation, and written into the issue
  **body** (a worker reads the body, not the comments). Then confirm or change the size (body line and label).
- Decision 244: if the change has a limit, a size or an identity, the body lists the boundary cases.
- Decision 273: if it depends on an external tool's behavior a spike recorded, re-run that check by hand first.

## 3. Check the machine

- Free space on the drive holding `sbx`'s state (`<sbxm> doctor`, or the drive itself): a Rust worker needs about
  9 GiB above `min_free_disk`. Free it by removing finished workers' sandboxes (stage 5), never by moving `sbx` state.
- Templates (G1): a changed `Cargo.lock` or profile makes a strict template stale, and the review refuses. Rebuild
  from a clean checkout of `main`: `<sbxm> template build <profile> --harness <h> --warm <checkout>`, one harness at
  a time. After a profile change: `just deploy-profiles` first.
- Secrets: `sbx secret ls` has each provider the task needs (an OAuth login, not a stray API key).

## 4. Run

- `<sbxm> task run --issue N --worker-harness claude --reviewer-harness codex [--local]`, in the background.
- Once the worker's commits are saved (`worker completed, N commit`), its sandbox may go: `sbx rm -f <name>`.
  A refused review (disk, stale template) resumes with `<sbxm> task review --issue N` after the fix.
- Decision 273: run the issue's real check (an ignored test or a by-hand smoke) after the worker, before the
  review, and give the result to the review; if it costs money or model calls, ask first.

## 5. Review loop

- Stop at the first round with 0 must-fix (decision 274). After 2 rounds that still have must-fix, stop and ask
  the user. A fix round hands back what it can't verify or settle (decision 272): that is the user's question.
- Fix rounds run in the sandbox (`<sbxm> task resume --issue N --rounds N`), not by hand. A hand fix ticks the
  boxes it meets in the PR's review comment, names the commit, and gets a narrow re-review (decision 251).

## 6. Finish

- `<sbxm> task finish --issue N`. A line in `result.md` that looks like a secret is refused: test values must be
  obviously fake; reword the line in the task folder and finish again.
- Should-fix and question findings: `<sbxm> task file-findings --issue N --create` (each issue gets a size).
- CodeQL alerts on fake test values: dismiss them as "used in tests".

## 7. Merge

- Low risk (the reviewer's level and the path-rule floor), 0 must-fix, and the review covered the PR's head
  (`gh pr view N --json headRefOid`): `gh pr merge N --auto --merge --match-head-commit <reviewed sha>` (decision
  266). Before any later push: `gh pr merge N --disable-auto`, push, review the new head, arm again.
- Medium or high risk: the user merges. While a review runs say **don't merge yet**; when done say **ready to
  merge**, and mark a draft ready yourself (`gh pr ready N`).

## 8. After the merge

- `gh pr view N --json state` says `MERGED` before anything is removed ("merged" has come early twice).
- `<sbxm> task rm --merged --yes`; remove the worktree and local branch (GitHub deletes the remote one). Never
  chain a `task rm` after a fetch of its result: check the fetched branch first.
- Add the auto-merge log row (`sdlc/batches/2026-10-24-auto-merge.md`) for an auto-merged PR.

## 9. Follow-ups and handoff (decision 278)

- Every promise made to the user that isn't done in the same turn goes to GitHub at once: a checkbox under
  `## Follow-ups` in its open home issue, else an issue labelled `follow-up` with a size. Done means ticked or closed.
- An agent's memory holds only pointers and facts local to one machine. Before a session ends, write a short
  handoff: what is done, in flight (log paths), next.
