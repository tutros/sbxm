---
name: sdlc-implementation
description: How code is written in this project - vertical slices, test-first (TDD), smallest possible change per step, and a commit after every green step. Use whenever implementing a feature, a milestone step, or a bug fix, before writing any production code.
---

# Implementation phase

Five rules govern every code change. They apply to features, milestone steps and bug fixes alike.

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

The spec is a file (e.g. `spikes/<id>.md` for spikes) and must contain:

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
| **Output contract** | One results file with a fixed template (create it with the spec), plus the shape of the final message. The worker doesn't edit `decisions.md`, `milestone-1.md` or `CLAUDE.md`; the main session merges conclusions. |
| **Exit criteria** | When the work is done: every item has a status, cleanup is confirmed, the results file is complete. |

Pre-decide every judgment call the worker would otherwise have to ask about: a background worker can't ask questions
mid-run. If a decision can't be pre-decided, the answer is Blocked with the question stated, not a guess.

When the work returns: check the claims against the evidence in the results file, report to the user what was
answered, what's blocked and whether cleanup was confirmed, and only then record decisions.

## Loop summary

```
pick slice (1 sentence) → list test cases →
  for each case: red (see it fail) → green (minimal code) → refactor → checks pass → commit
→ slice done: run the #[ignore] real-sbx test if the slice touches sbx → report
```

When a slice is done, report to the user: what now works, the commits made, and anything deferred.

## Project context

- The plan and constraints are in `milestone-1.md` and `decisions.md`. Follow them; if code needs to depart from a decision, stop and ask, then record the new decision.
- Tests never need Docker by default. Tests against real `sbx` are `#[ignore]` and must use a base dir **outside** `%TEMP%`/AppData (see Spike results S1).
