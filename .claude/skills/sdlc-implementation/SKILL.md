---
name: sdlc-implementation
description: How code is written in this project - vertical slices, test-first (TDD), smallest possible change per step, and a commit after every green step. Use whenever implementing a feature, a milestone step, or a bug fix, before writing any production code.
---

# Implementation phase

Six rules govern every code change. They apply to features, milestone steps and bug fixes alike.

## 1. Build in vertical slices

A slice is one thin, user-observable behavior that runs end to end: CLI → domain logic → backend. It's never one layer on its own.

- Pick the next slice by asking: *what is the smallest thing a user could run and see work?* Example: `sbxm new <project>` rejecting an invalid name, *before* any kit generation exists.
- A slice touches only the parts of each layer it needs. Leave the rest of a module unbuilt until a later slice needs it.
- Tests use `FakeBackend`. The real `sbx` backend for a slice gets an `#[ignore]` integration test.
- If a plan lists work layer by layer (e.g. "build the whole config module, then the kit module"), re-cut it into slices before starting, and tell the user the order changed.

State the slice in one sentence before starting it: *"After this slice, running X produces Y."*

## 2. Test first (TDD)

For each slice, repeat red → green → refactor:

1. **Red:** write the test cases that define the behavior *before* the production code exists. Run them and confirm they fail **for the expected reason** (assertion failure or missing function, not a typo or broken setup). Show the user the failing output.
2. **Green:** write the least code that makes those tests pass. Don't add behavior no test demands.
3. **Refactor:** clean up with all tests passing. Behavior doesn't change here, so no new tests are needed.

Rules:
- Test cases come from the slice's acceptance criteria and the edge cases in the plan/decisions (e.g. every name-validation rejection case). List them before writing them.
- Test public behavior (CLI output, return values, files written, backend calls recorded by `FakeBackend`), not private internals.
- A bug fix starts with a test that reproduces the bug.
- Never weaken, delete or `#[ignore]` a failing test to get to green. If a test is wrong, say so and fix it explicitly as its own step.

## 3. Commit after every green step

After each green (or refactor) step, and only when **all** of these pass:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

commit that step. One commit = one small, working change.

- Commit message: imperative subject that says what behavior changed (`Reject reserved Windows names in project names`), plus a short body only if the reason isn't obvious.
- Never commit with failing tests, lint errors or unformatted code. Never skip hooks.
- Don't commit a red (failing-test) state on its own; the failing tests go in the same commit as the code that makes them pass.
- Stage files explicitly. Don't commit generated artifacts, secrets or local state.

## 4. Smallest change at each step

- Each red → green cycle adds **one** behavior: typically one test or a few closely related ones.
- If a step needs more than about one screen of new production code, split it.
- Don't refactor unrelated code, rename things or add dependencies "while you're there". Put it on a list and raise it as its own step.
- Prefer a hard-coded or simple implementation first, and generalize only when the next test forces it.

## 5. Unattended work needs a written, approved task spec

Rules 1–4 assume the user can see every step. Before any work that the user won't check off step by step, write a
task spec and get the user's approval **before** starting. This applies when:
- launching a subagent or background task (spike, research, parallel slice),
- skipping the interactive loop (e.g. running several slices without reporting between them),
- making a change larger than rule 4 allows (many files, several behaviors, or a big refactor in one step).

The spec is a file (e.g. `sdlc/spikes/<id>.md` for spikes) and must contain:

| Section | Contents |
|---|---|
| **Why** | What it unblocks (slices, decisions) and what to read first. |
| **Environment facts** | Verified facts the worker would otherwise rediscover or get wrong: available credentials, paths to use and to avoid, tool versions, known gotchas. Mark hypotheses as unverified. |
| **Questions or tasks** | Each with a method (concrete commands, in order) and the **acceptance criteria**: what evidence proves it done. |
| **Statuses** | Each item ends as Answered/Done, Partial (gap stated) or Blocked (reason stated). A belief without evidence is not an answer. |
| **Budget** | Hard limits: tool calls, wall time, model or paid API calls, concurrent sandboxes, disk. What to do when one is hit (stop, clean up, report). |
| **Stop rules** | Same error twice → record as Blocked and move on. Forbidden commands (interactive auth, anything that changes global or shared state). Limits on web research. No scope creep: new questions go to a Follow-ups list. |
| **Side effects** | Allowed write locations and files; naming prefix for anything created; no git state changes unless the spec says so; nothing outside the prefix is deleted. |
| **Cleanup** | Steps that always run, including after failure, plus commands whose output proves it. |
| **Output contract** | One results file with a fixed template (create it with the spec), plus the shape of the final message. The worker doesn't edit `sdlc/decisions.md`, `sdlc/milestone-1.md` or `CLAUDE.md`; the main session merges conclusions. |
| **Exit criteria** | When the work is done: every item has a status, cleanup is confirmed, the results file is complete. |

Pre-decide every judgment call the worker would otherwise have to ask about: a background worker can't ask questions
mid-run. If a decision can't be pre-decided, the answer is Blocked with the question stated, not a guess.

When the work returns: check the claims against the evidence in the results file, report to the user what was
answered, what's blocked and whether cleanup was confirmed, and only then record decisions.

**Lighter form for in-session batches.** Running several small slices back to back in the main session, where every
step is still a TDD commit the user can review, doesn't need a spec file. Agree these stop conditions in chat instead,
and post a short report after each slice:
- Stop and ask if a slice needs a new decision, departs from the plan, or needs a new dependency.
- Stop if any real-`sbx` check fails, or if anything would touch paths outside temp dirs and the real-test base dir.
- Stop if a slice's scope grows beyond its row in the plan.
- Checks only the user can do (e.g. attaching to an interactive agent) end the batch with exact steps for the user.

## 6. Merging worktrees

Unattended work runs in a git worktree. Nothing merges automatically; the main session does it, the same way every time.

**Spike or research worktrees** (the worker doesn't commit):
1. Check scope: `git -C <worktree> status` shows only the results file changed. Anything else → tell the user before using the results.
2. Check the evidence behind every Answered claim, and re-run the cleanup checks yourself instead of trusting pasted output.
3. Copy the results file into the main tree and commit it. Record conclusions in `sdlc/decisions.md` (and the plan, if slices change) after showing the user.
4. `git worktree remove <path>` and `git branch -D <branch>`; confirm with `git worktree list`.

**Coding worktrees** (the worker commits green TDD steps):
1. Merge-ready means: the spec's exit criteria are met, the branch is clean, and it doesn't touch `sdlc/decisions.md`, `sdlc/milestone-1.md` or `CLAUDE.md` (the main session owns those).
2. Rebase onto the latest `main`, checking every commit:
   `git rebase main --exec "cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test"`.
   Fix a conflict or a red commit in *that* commit, so each commit stays green.
3. `git merge --ff-only <branch>`. No merge commits, no squashing (it erases the red → green record).
4. Re-run the checks on `main`, plus the `#[ignore]` real-`sbx` tests if the work touched `sbx`.
5. Update docs, report to the user (commits, tests, deferred items), then remove the worktree and branch.
6. Merge one branch at a time; rebase each remaining branch onto the new `main` before it lands.

## 7. Working a GitHub issue

Review findings are GitHub issues with acceptance criteria (code review skill, section 8). To work one:

1. **Read it and check it still applies.** Re-run its evidence on the current code (the permalinks point at the
   reviewed commit, not `HEAD`). If it no longer reproduces, say so on the issue with the evidence and ask the user
   before closing it. **Check its dependencies:** every issue under "Depends on" must be closed first; if one is
   open, work that one first or ask the user. Read the "Related" issues too, so the change doesn't undo or duplicate
   theirs.
2. **The acceptance criteria are the test cases.** List them as the slice's test cases (rule 2) and start red: the
   issue's test fails for the reason the issue describes. A criterion that can't become an automated test needs its
   manual check spelled out (exact command, expected output).
3. **Work it like any slice:** smallest change, green, checks, one commit per step. The commit that completes the
   issue ends its message with `Fixes #<n>` on its own line, so the push closes it.
4. **Done means every criterion is met, with evidence.** Before the last commit, go through the criteria one by one:
   run each check and keep the output. After pushing, comment on the issue with one line per criterion and its
   evidence (test name, command output, commit), and tick the boxes. If a criterion turns out wrong or impossible,
   don't quietly drop it: say why on the issue and get the user's OK first.
5. **No GitHub access** (e.g. inside a sandbox): work from the issue text the user or review file provides, keep
   `Fixes #<n>` in the commit, and report the per-criterion evidence in chat so it can be posted later.

## 8. The milestone flow (how M1 was built; M2 follows it)

A milestone is built as one batch, reviewed once at the end. It is the lighter form of rule 5 applied to a whole milestone:

1. **One branch per milestone** (M2a: `m2a-implementation`), pushed for backup. Slices are commits on it in plan
   order, not one branch or PR per slice.
2. **Each slice** runs the loop below (red → green → checks → commit, real-`sbx` test if it touches `sbx`) and ends with a
   short report: what now works, the commits, deferred items. No per-slice review and no per-slice PR.
3. **Rule 5's stop conditions apply throughout.** Also stop for a slice-level review when the user asks, or when a slice
   is risky enough (security guards, destructive operations) that waiting for the end isn't safe.
4. **At the end of the milestone,** one full review of `main..<milestone branch>` with the `sdlc-code-review` skill
   (full checklist plus the cross-cutting sweep), written to `sdlc/reviews/<date>-milestone-<n>.md`. Its findings become
   GitHub issues (`must-fix`, `should-fix`, `question`).
5. **Fixes go through rule 7,** one issue at a time, TDD, `Fixes #<n>`, each through a branch and PR that the independent
   reviewer checks before the user merges (never push to `main`).
6. **The milestone lands** as a PR from the milestone branch after the end-of-milestone review's must-fix issues are
   fixed or the user accepts them.

## Loop summary

```
pick slice (1 sentence) → list test cases →
  for each case: red (see it fail) → green (minimal code) → refactor → checks pass → commit
→ slice done: run the #[ignore] real-sbx test if the slice touches sbx → report (no per-slice review, rule 8)
… next slice …
→ milestone done: one full review (sdlc-code-review, end-of-milestone row) → issues → fix each (rule 7) → PR
```

When a slice is done, report to the user: what now works, the commits made, and anything deferred.

## Project context

- The plan and constraints are in `sdlc/milestone-1.md` and `sdlc/decisions.md`. Follow them; if code needs to depart from a decision, stop and ask, then record the new decision.
- Tests never need Docker by default. Tests against real `sbx` are `#[ignore]` and must use a base dir **outside** `%TEMP%`/AppData (see Spike results S1).

## Code conventions from past decisions

- **Check before acting.** Validate every input and precondition before the first write or backend call, so a failed command changes nothing. Each rejection gets a test asserting that nothing was created and no backend call was made (decisions 33, 47).
- **Error messages say what's wrong and how to fix it,** in one line: `<problem>; <fix>`, naming the exact path, setting or command (e.g. `base dir X does not exist; create it or change base_dir in Y`). Hints in normal output follow the same form (decision 48).
- **sbxm never changes `sbx` settings or other global state** (settings, secrets, skills store, policies). When they block something, tell the user the exact command and what it trusts or changes (decision 48).
- **Deleting or overwriting user files** needs extra guards, each with a test: the path is re-derived from validated input, must sit where expected (canonical parent check), must not be a symlink or junction, and the user confirms with the exact path shown. Tests delete only inside temp dirs.
- **Parsers of `sbx` output** are unit-tested against output captured from the real `sbx` (`src/backend/fixtures/`), noting the `sbx` version.
- **Real-`sbx` tests** create only uniquely named resources (`sbxm-sbxm-it-<pid>-…`), clean up in `Drop` even on failure, and never touch other sandboxes. Checks that need an interactive terminal are done by the user, with exact steps in the report.
- **Escape characters:** write files containing `\\` (JSON fixtures, Windows paths, escapes in generated code) with the Write/Edit tools, not shell heredocs (see CLAUDE.md).
