---
name: sdlc-code-review
description: How code is reviewed in this project - pick a scope (a slice's commits, a worktree branch before merging, or a whole milestone), check it against the plan, decisions and project conventions with evidence for every finding, rank findings, and turn each fix into a TDD step. Use after finishing a slice, before merging a worktree, at the end of a milestone, or when the user asks for a review.
---

# Code review phase

A review reads a finished change and reports what's wrong with it, with evidence. It doesn't fix anything itself:
every fix goes back through the `sdlc-implementation` skill (a bug fix starts with a test that reproduces it).

## 1. When to review, and the scope

| Trigger | Scope (`<base>..<head>`) | Depth |
|---|---|---|
| A slice is done, before reporting it | The slice's commits: `git log --oneline <before-slice>..HEAD` | Checklist sections 3–6 on the diff |
| Before merging a coding worktree (implementation rule 6) | `main..<branch>` | Full checklist, plus the merge-ready criteria in rule 6 |
| End of a milestone, or the user asks | The milestone's commits, or what the user names | Full checklist, plus the cross-cutting sweep in section 4 |

State the scope in one line before starting: *"Reviewing `abc123..def456` (slice 18c, 7 commits)."*

Read first: `git diff --stat <base>..<head>`, then the full diff, then the slice's row in the milestone plan and every
decision the commits cite. Read each changed function whole, not just its diff hunk: most real bugs sit next to the
change, not in it.

## 2. Evidence rules

- Every finding cites `file:line` (or a commit) and says **what happens**, not what might: a failing input, the wrong
  output, the command that shows it. Run it if you can (a test, `cargo run -- …` against a temp `SBXM_CONFIG_DIR`,
  a `grep`). A belief without evidence is a *question*, and is labeled as one.
- Never run anything outside the rules of the implementation skill: no real config, no paths outside temp dirs and
  the real-test base dir, no `sbx` setting, secret or policy changes.
- Quote the decision or plan row a finding relies on (`decision 11`, `slice 20 row`).

## 3. Checklist: does it do what was agreed?

- **Plan and decisions.** The change does what its slice row says, and nothing it doesn't. Any departure from
  `decisions.md` has a new numbered decision the user confirmed. New behavior no decision covers is a finding, even
  if it looks right.
- **Silent drops (decision 11).** Every configured setting either takes effect or produces a loud warning. Look for:
  settings that are written or accepted but never read (e.g. a key `config init` writes that no code uses), unknown
  keys that deserialize without error, features skipped for some harnesses without a warning.
- **Check before acting.** All validation happens before the first write or backend call. Each rejection has a test
  asserting nothing was created and `FakeBackend` recorded no call.
- **Errors and hints** follow `<problem>; <fix>` on one line and name the exact path, setting or command. The fix
  they suggest actually works for the case at hand (e.g. includes `--harness` when the harness isn't the default).

## 4. Checklist: is it complete everywhere?

The most common miss in this project is a change applied in one place but not in its siblings. Sweep for them:

- **Per-harness code.** When anything depends on the harness, `grep` for hard-coded `"claude"`, `Harness::Claude`,
  and `match` arms on `Harness`; every command (`new`, `open`, `stop`, `rm`, `list`, `config show`, `doctor`) and
  every message or hint must handle every harness.
- **Parallel commands.** A rule added to one command (a check, a flag, a message) is needed in the others that do the
  same thing, e.g. `new` and `open` both create sandboxes; `doctor` must check what `new` would do.
- **State and hash.** Anything that changes what a sandbox is built from joins the config hash (decisions 55, 69), and
  anything stored in `state.json` survives old state files (`#[serde(default)]`).
- **Docs.** `README.md` (user-facing behavior), `AGENTS.md` (code layout), `milestone-1.md` (progress) and
  `decisions.md` are updated in the same change. Example commands in docs are ones that actually run.

## 5. Checklist: tests

- Tests were written first: each behavior commit contains its tests, and the report showed them failing for the
  expected reason.
- Tests check public behavior (CLI output, return values, files written, `FakeBackend` calls), not internals.
- No test was weakened, deleted or `#[ignore]`d to get green. An existing expectation that changed was changed on
  purpose and says why in the commit (e.g. it encoded the bug).
- Edge cases from the decisions each have a test (every rejection rule, every harness).
- Snapshot updates (`insta`) were read line by line, not accepted wholesale.
- If the change touches `sbx` interaction, the `#[ignore]` real-`sbx` test covers it, was run, and cleaned up (unique
  `sbxm-sbxm-it-<pid>-…` names, removal in `Drop`).

## 6. Checklist: safety and hygiene

- **Security.** Secret values never reach output, files or tests. sbxm changes no global `sbx` state. Paths from
  config or state are re-validated before use; deletions keep the guards (canonical parent check, no links,
  confirmation with the exact path). Nothing sbxm-owned lands inside a mounted workspace.
- **Code.** Matches the surrounding code's naming, comment density and idioms; no dead code, no unrelated refactors,
  no new dependency without a decision. Duplicated knowledge (the same list or rule in two places) is a finding.
- **Commits.** Each commit is one green step with `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test` passing; messages say what behavior changed. Verify by running the checks on `<head>`.

## 7. Findings and report

Rank each finding:

| Severity | Meaning | What happens next |
|---|---|---|
| **Must fix** | Wrong behavior, a silent drop, a security or data-loss risk, a missing test for a stated rule | Fixed before the slice is reported or the branch merged |
| **Should fix** | Incomplete sweep, misleading message or doc, duplicated knowledge | Fixed now, or put in `milestone-1.md` carry-over items with the user's OK |
| **Question** | Suspicion without evidence, or a gap no decision covers | Asked the user, one at a time, with a recommendation |

Write each as: severity, `file:line`, what happens (with the evidence), why it matters (decision or convention), and
the smallest fix. For more than five findings, put them in `reviews/<date>-<scope>.md` and keep the chat report short.

Report to the user: the scope, the counts per severity, each must-fix in one line, and the first question if there is
one. Then fix must-fix items through the implementation skill (test first, one commit each) and re-run the review on
the fix commits only.

## Independent reviews

When the user asks for a fresh-eyes review, run it as a read-only subagent: give it the scope, this skill, and the
output format above. It doesn't edit files or commit; the main session checks its evidence before reporting any of
its findings (as with spike results).
