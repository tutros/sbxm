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
- Decision 292: reserve the number of any decision the issue adds now, in the body ("this adds decision N", gap
  G41; a new decision is a new file in `sdlc/decisions/`, decision 303), and have its new tests go in a file or section of the issue's own, never appended at the end of a shared
  test file: two open PRs that append at the same place conflict once one merges.
- Decision 286 (an experiment, two groups): issues that touch the same code and are to be done in order go to one
  worker as one spec (`task run --spec`), with every issue's acceptance checks; the PR closes all of them. Each
  issue is readied as above first.
- Decision 296 (an experiment): **same spot, so run them in order.** Before an issue goes into a session or a night
  run, name the files and the spots it will edit (a module list, a section of `sbxm status`, a match arm, the end of a
  shared doc), and compare them with every open PR and every other issue in the same run. Two that edit the same spot
  either go to one worker as one spec (decision 286) or run one after the other, the second started only after the
  first merges. They never run side by side.
- Decision 295: **re-check a `ready-for-worker` issue right before its task starts** (and before an unattended
  list): read the body back whole; check every file, function, flag and behavior it names against today's `main`;
  list the PRs merged since it was labeled that touch its area (`gh pr list --state merged --search
  "merged:>=<label date>"`); and check its acceptance list agrees with its own answers. A point that no longer holds
  goes back to the user as a question, with the label removed until it is answered. Ready issues just in time:
  only those for the next session or night run, not the whole backlog, so less code changes under them.

## 3. Check the machine

- Location first: `--local`/`--cloud`, else `[sandbox] location` in `sbxm-task.toml`. Every `sbx` command below
  takes `--cloud` for a cloud task (`sbx --cloud secret ls`, `sbx --cloud rm`); disk and templates are local only.
- Local: free space on the drive holding `sbx`'s state (`<sbxm> doctor`): a Rust worker needs about 15-20 GiB above
  `min_free_disk` (about 19 GiB per worker in milestone 4, 2026-10-30 retro; issue 343's worker took about 15 GB on
  2026-10-31 and its sandbox went read-only). Free it by removing finished workers' sandboxes (stage 4), never by
  moving `sbx` state.
- Local templates (G1): a strict template is stale when the profile changed since it was built; rebuild it:
  `<sbxm> template build <profile> --harness <h> --warm <a clean checkout of main>`, one harness at a time (after a
  profile change, `just deploy-profiles` first). When only `main`'s lock files changed, the first task rebuilds it
  from `main` by itself (decision 290, about 4.5 min per harness). A task whose own branch changes `Cargo.lock`
  stops with a "Needs a decision" hand-back (decision 291): the user approves the crate, it goes onto `main` in a
  small PR of its own, then the task is resumed. Never warm a template from a worker's or a PR's commit.
- Secrets: each provider the task needs is listed (an OAuth login, not a stray API key).

## 4. Run

- No separate real check needed: `<sbxm> task run --issue N --worker-harness claude --reviewer-harness codex
  [--local]`, in the background. It runs the worker, gates, reviews and fix rounds without a pause.
- A real check needed (decision 273: the issue depends on an external tool, or it has an ignored real test):
  `<sbxm> task start --issue N ...`; when it ends, run the check (ask first if it costs money or model calls),
  append its result to `issue.md` in the task's folder (the reviewer reads that file), then `<sbxm> task review
  --issue N`.
- Once the worker's commits are saved (`worker completed, N commit`), its local sandbox may go: `sbx rm -f <name>`.
  A refused review goes on with the command its refusal names (`<sbxm> task resume --issue N` once a handed-back
  crate is on `main`, decisions 235, 291; `<sbxm> task review --issue N` after freeing disk).

## 5. Review loop

- Stop at the first round with 0 must-fix (decision 274). After 2 rounds that still have must-fix, stop and ask
  the user. A fix round hands back what it can't verify or settle (decision 272): that is the user's question.
- Fix rounds run in the sandbox (`<sbxm> task resume --issue N --rounds N`), not by hand. A hand fix ticks the
  boxes it meets in the PR's review comment, names the commit, and gets a narrow re-review (decision 251).

## 6. Finish

- `<sbxm> task finish --issue N`. A line in `result.md` that looks like a secret is refused: test values must be
  obviously fake; reword the line in the task folder and finish again.
- Should-fix and question findings: `<sbxm> task file-findings --issue N --create` (each issue gets a size).
- CodeQL skips `tests/` (`.github/codeql/codeql-config.yml`, issue 336); an alert on fake test values elsewhere:
  dismiss it as "used in tests". Secret-shaped test values stay obviously fake.

## 7. Merge

- An open PR that conflicts with `main` after another PR merged: `<sbxm> task fix --pr N` merges `main` in its
  sandbox, resolves, gates, pushes, and its in-task review covers the merge narrowly (decision 292). It needs a
  must-fix finding to work from, or else GitHub reporting the PR conflicting or behind its base (then a head-covering
  review with 0 must-fix is enough).

- Low risk (the reviewer's level and the path-rule floor), 0 must-fix, and the review covered the PR's head
  (`gh pr view N --json headRefOid`): `gh pr merge N --auto --merge --match-head-commit <reviewed sha>` (decisions
  266, 280). Before any later push: `gh pr merge N --disable-auto`, push, review the new head, arm again.
- Medium or high risk: the user merges. While a review runs say **don't merge yet**; when done say **ready to
  merge**, and mark a draft ready yourself (`gh pr ready N`).
- Watch CI without being asked (decision 297, an experiment): after every push to a PR and every arm, start a
  background watch that ends on the PR merged, any check failed, or the PR closed; after a merge, watch `main`'s
  run of the merge commit too. On a failure, read the log (`gh run view <id> --log-failed`) the same turn: a runner
  fault or a known flake gets one re-run of the failed job (`gh run rerun <id> --failed`) and an issue if it is new;
  a real failure on an open PR is fixed on its branch (auto-merge disarmed first) and gets a narrow re-review; a
  real failure of the merge commit's run on `main` is a break after the merge: stage 8 and decision 280 (pause
  auto-merge, the user decides), never a push to the merged PR. Tell the user what failed and what was done.

## 8. After the merge

- `gh pr view N --json state` says `MERGED` before anything is removed ("merged" has come early twice).
- `<sbxm> task rm --merged --yes`; remove the worktree and local branch (GitHub deletes the remote one). Never
  chain a `task rm` after a fetch of its result: check the fetched branch first.
- A break found after an auto-merge (CI red on `main`, a revert, a later must-fix traced to it) pauses auto-merge:
  tell the user, who decides (decision 280).

## 9. Follow-ups and handoff (decision 278)

- Every promise made to the user that isn't done in the same turn goes to GitHub at once: a checkbox under
  `## Follow-ups` in its open home issue, else an issue labelled `follow-up` with a size. Done means ticked or closed.
- An agent's memory holds only pointers and facts local to one machine. Before a session ends, write a short
  handoff: what is done, in flight (log paths), next.
