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
| A slice is done, only when the user asks or the slice is risky (implementation rule 8; milestones are reviewed once, at the end) | The slice's commits: `git log --oneline <before-slice>..HEAD` | Checklist sections 3–6 on the diff |
| Before merging a coding worktree (implementation rule 6) | `main..<branch>` | Full checklist, plus the merge-ready criteria in rule 6 |
| A plan or docs-only branch (milestone plan, decisions) | `main..<branch>` | Sections 2 and 3, judged by the must-fix bar in section 7 |
| End of a milestone (the normal review; implementation rule 8), or the user asks | `main..<milestone branch>`, or what the user names | Full checklist, plus the cross-cutting sweep in section 4; record in `reviews/<date>-milestone-<n>.md` |

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
- **Every changed behavior has a test that fails without it.** For each production hunk, name the test that exercises
  it. Where practical, prove it: revert or break the hunk (e.g. flip a condition, drop a line), run that test, and
  confirm it fails. A test that still passes doesn't cover the change. When a test isn't feasible, the finding must
  state why (e.g. an interactive terminal prompt, or real `sbx` needing Docker), and a real-`sbx` test or a manual
  step with exact commands covers it instead.
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
- **Minimal diff.** The diff is the smallest one that produces the intended behavior. Every changed line is needed for
  that behavior or its tests. Findings include reformatting or renaming untouched code, refactors mixed into a
  behavior commit, and abstractions, options or parameters no test needs. **Check:** for each hunk, ask what breaks if
  it's reverted. If no test fails and no agreed behavior is lost, the hunk is a finding.
- **Simple code.** Each needed change takes the simplest form that works: plain functions and data over new traits,
  generics, builders or layers; straightforward control flow over clever chains; an existing helper or the standard
  library over new machinery. Added complexity needs a present reason, such as a second real caller or a stated
  requirement, not an anticipated one. **Check:** could the same tests pass with fewer types, branches or
  indirection? If an obviously simpler version exists, the current code is a finding, and the simpler version is the
  fix.
- **Code.** Matches the surrounding code's naming, comment density and idioms; no dead code, no unrelated refactors,
  no new dependency without a decision. Duplicated knowledge (the same list or rule in two places) is a finding.
- **Commits.** Each commit is one green step with `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test` passing; messages say what behavior changed. Verify by running the checks on `<head>`.

## 7. Findings and report

Rank each finding:

| Severity | Meaning | What happens next |
|---|---|---|
| **Must fix** | Meets the bar in "What counts as must-fix" below | Fixed before the slice is reported or the branch merged |
| **Should fix** | Incomplete sweep, misleading message or doc, duplicated knowledge | Fixed now, or put in `milestone-1.md` carry-over items with the user's OK |
| **Question** | Suspicion without evidence, or a gap no decision covers | Asked the user, one at a time, with a recommendation |

### What counts as must-fix

A finding is **must fix** only if it has evidence *and* at least one of these holds:

1. **Wrong behavior now:** the code or plan produces a wrong result, a silent drop (decision 11), a security or
   data-loss risk, or contradicts a decision the user confirmed.
2. **Costly to reverse:** left alone, it locks in a schema, data model, on-disk layout, public CLI/config surface or
   dependency that is expensive to change once code, tests or users depend on it.
3. **Nothing later will catch it:** the slice's own TDD, real-`sbx` check or review wouldn't surface it.

Everything else is **should fix** (or a nit), however true it is. These are never must-fix:

- A missing test case for a rule the change already states. Tests are written first in each slice, so this is
  should-fix at most in a plan, and must-fix only in code that ships without a test for a stated rule.
- Wording, stale numbers or ranges, formatting, naming preferences, and "the plan could say more here".
- A detail an implementer would settle naturally inside the slice (an exact field list, an internal helper, an
  error message), unless the gap forces a wrong or costly-to-reverse design.
- Speculative edge cases with no failing input, or hardening beyond the stated requirements.
- Anything that is a preference between two workable designs: that is a **question**, not a finding.

When reviewing a **plan or docs**, apply this bar to design consequences, not completeness: a plan is allowed to
leave slice-level detail to the slice. Cite the criterion (1, 2 or 3) on every must-fix. A review with zero
must-fix findings is a good result, so don't pad. Put each nit in one closing line, not its own finding.

### When to stop reviewing

Re-review only the fix commits, not the whole change again. Run another round only if the last one found a
must-fix, or the fixes themselves introduced a contradiction, a regression or a new must-fix. Stop, and let the
user merge, when a round finds no must-fix: file the remaining should-fix items and nits as issues instead of
looping. If three rounds in a row keep finding must-fix in the same document, stop and ask the user whether the
scope or the design needs rethinking; more review won't converge it.

Write each as: severity, `file:line`, what happens (with the evidence), why it matters (decision or convention), and
the smallest fix. For more than five findings, put them in `reviews/<date>-<scope>.md` and keep the chat report short.

Then file them as GitHub issues (section 8) and put the issue numbers in the review file.

Report to the user: the scope, the counts per severity, each must-fix in one line, and the first question if there is
one. Then fix must-fix items through the implementation skill (test first, one commit each) and re-run the review on
the fix commits only.

## 8. GitHub issues

Every finding becomes a GitHub issue (decision 79), so the work to fix it has a place and a clear end.

**Check access first**, before writing any issue:
1. `git remote get-url origin` names a GitHub repo, and `gh repo view --json nameWithOwner` gives its name. Use that
   name; don't hard-code one.
2. `gh auth status` shows a logged-in account with `repo` scope.

If either fails (e.g. inside a sandbox with no `gh` login, no `github` secret or no `github.com` egress), don't file
anything and don't try to set up a remote or log in: that's the user's call. Write the findings to the review file,
mark it `Issues: pending (no GitHub access from <where>)`, tell the user, and file them later from where access
works.

**Template.** Every issue has exactly these parts:

```
Title:  <id>: <what happens, in plain words>          e.g. "M1: seed with a link is refused only after kits are written"
Labels: must-fix | should-fix | question

**Where:** permalinks at the reviewed commit: https://github.com/<repo>/blob/<full sha>/<file>#L<a>-L<b>
**What happens:** the behavior, with evidence (the command or test, and its output trimmed to the lines that prove it)
**Why it matters:** the decision or convention it breaks (e.g. "decision 47", "`<problem>; <fix>` convention")
**Fix:** the smallest change that resolves it
**Depends on:** issues that must be done first, each with why (e.g. "#4: this message must match the one #4 sets"),
  or "none known"
**Related:** issues touching the same code or behavior, with the order that avoids rework (optional)
**Acceptance criteria:**
- [ ] <observable result that proves the fix, e.g. "`new` with a linked seed makes no backend calls and creates no `.sbxm/`">
- [ ] A test covering it fails before the fix and passes after (name it, or say which file it goes in)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Docs updated where behavior users see changed (`README.md`), or "no user-visible change"
- [ ] <for sbx-facing changes> the `#[ignore]` real-sbx test covering it passes and cleans up
**Review:** link to the section in `reviews/<date>-<scope>.md`
```

**Acceptance criteria** say when the work is done. Each one is observable and checkable by someone other than the
author: a command and its expected output, a test name, a message's exact text, or a file that must or mustn't
exist. No criterion like "works better" or "is cleaner". The first criteria are specific to the finding; the
standard ones (test, checks, docs, real `sbx`) follow and are dropped only when they can't apply, with a reason. The
issue is done when every box is ticked with its evidence, as described in the implementation skill.

**Dependencies.** Look for them across the whole set of findings, not one issue at a time: two issues editing the
same function, one message that must match another, or a fix that only makes sense after a decision. A question
issue blocks the issues that depend on its answer. Write the link on both issues (`Depends on` on one, `Related` on
the other) so either one leads to the other.

**Content rules:**
- Never include secret values, tokens, credential files or their contents; redact any that appear in output.
- Quote output only as far as it proves the point. The repo is public: replace personal paths and names (e.g.
  your user folder) with placeholders like `<base_dir>` or `~` unless the exact path is the evidence.
- A **question** issue replaces "Fix" and the first criteria with the options and a recommendation. When the user
  answers, edit the issue: record the answer and the new decision number, relabel it `must-fix` or `should-fix`, and
  add its acceptance criteria. A question still open blocks nothing else.
- Write bodies to temp files with the file-write tool and pass `--body-file` (Windows paths contain backslashes, which
  the Bash tool mangles). Delete the temp files afterwards.

## Independent reviews

When the user asks for a fresh-eyes review, run it as a read-only subagent: give it the scope, this skill, and the
output format above. It doesn't edit files or commit; the main session checks its evidence before reporting any of
its findings (as with spike results).
