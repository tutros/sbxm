# Improvement review: {{slug}} ({{date}})

Queued by `sbxm task loop` (decision 330(2) and (3), issue 715): 20 or more PRs merged since the newest improvement
review, and `{{slug}}` is the area of `sdlc/reviews/areas.md` whose newest review is oldest.

## Task

Review the code in this area for changes that would make it easier and safer to change, and write what you find to
one file, `{{output}}`. Commit that file and nothing else: no code, test or docs change, no issue filed. The user sorts
the findings afterwards, filing each one as an issue or not.

- **Scope:** {{paths}}, at the commit you start from. Read the tests, `docs/project.md`, `sdlc/evals-workflow-notes.md`
  and the decisions the code cites, to drop what is already tracked or decided.
- **Run data** (kind 7 only): below, gathered on the host since {{since}}.

## What to look for

1. Duplicated logic that drifted or could: one fact worked out in two places.
2. Too large to change safely: a function or file that grows with nearly every feature.
3. Hidden coupling: a fact one module owns, re-derived in another.
4. Half-done after a stop, kill or restart: state that only lives in memory, or a write that a kill can tear.
5. Tests that pin implementation details instead of behavior.
6. Work done more often than needed (repeated reads, calls or scans), with its cost.
7. Workflow friction the code could remove: patterns in how the work ran, such as PRs resolved side by side, repeated
   review rounds, long waits or stops, that a change to sbxm would prevent. A kind 7 finding needs evidence from the run
   data below (the rows, by task or issue) and from the code that causes it. It isn't limited to the area above: it is
   limited to the run data, and each finding names the code it would change.

## Evidence

Every finding names its place (`path:line`, at the commit you start from) and shows what happens with a concrete input
or a quoted line. A belief without evidence is not a finding. Leave out what a test, a decision or a tracked gap
already covers.

## At most 10 findings

Report at most 10 findings, the most valuable first. There is no must-fix: every finding is a should-fix or a question,
and nothing here blocks a PR.

## The file

`{{output}}`, in this shape:

```
# Improvement review: <the area in words> ({{slug}})

Scope: `<the commit>` (main), {{paths}}
Issues: pending (the user sorts the findings)

## Should fix

### IMP-1 - <one line: what is wrong>

**Where:** `<path:line>`, ...
**What happens:** <the evidence>
**Why it matters:** <the kind, by number, and its cost>
**Fix:** <the change>
**Size:** S, M or L (<why>)
**Depends on:** <issues or findings, or none known>
**Related:** <other findings, if any>
**Acceptance criteria:**
- [ ] <a check, ideally a test that fails before the fix>
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass

## Questions

### IMP-<n> - <a finding that needs the user's choice first>

<the same fields, with **Recommendation:** instead of **Fix:**>

## Summary

<the files read and their sizes, how they were read, a table of findings per kind (1 to 7), and what was looked at
and found worth no change>
```

Number the findings `IMP-1` onwards across both sections; each finding's heading starts with `### IMP-`, so the loop
can count them. Leave a section out when it has none.

## Run data

{{left_out}}

{{run_data}}
