# M2b PRD/spec experiment: did it pay off? (decision 140)

Written 2026-10-02 at the end of M2b, from the review records. **This is an assessment, not a decision.** The `sdlc-*`
skills are unchanged until the user decides what to do with it (decision 164).

## What was tried

M2a was planned with one document (`sdlc/milestone-2.md`). M2b was planned with a PRD (`sdlc/prd-m2b.md`: what and why,
success criteria S1-S9) and a spec (`sdlc/spec-m2b.md`: exact behavior, every rule becomes a test), with the milestone
plan keeping only work order, checklist and risks. The hope was less duplication and fewer review findings about missing
behavior.

## Evidence

Findings counted per review record (observations excluded; a finding is a finding or a question):

| Review | Must-fix | Should-fix | Questions | Total |
|---|---|---|---|---|
| M2a, end of milestone (no PRD/spec; baseline) | 2 | 1 | 1 | 4 |
| M2b slice 5 (parallel) | 0 | 4 | 2 | 6 |
| M2b slice 7 (parallel) | 0 | 5 | 2 | 7 |
| M2b slice 10 (parallel) | 2 | 5 | 2 | 9 |
| M2b slice 13, end-to-end check | 2 | 8 | 0 | 10 |
| M2b, PR 57 independent review, round 1 | 4 | 2 | 1 | 7 |
| **M2b total** | **8** | **24** | **7** | **39** |

(M2A-3 in the baseline, "one commit is not a green step", is a process finding; counted under should-fix.)

How I classified the 39 M2b findings (subjective; the reviewers' own wording is in the records):

- **A spec section or decision names the thing violated or left out: 5.** PR 57 M-2 (a flag is lost between phases; spec
  5.2, decisions 139, 149), M-3 (on-demand gates test a revision review and finish never see; spec 5.2, 6, 7),
  M-4 (a record is written after the work it must name; decisions 145, 153), S-1 (`task status` omits commits ahead,
  a field the spec lists) and S-2 (the experiment report itself, decision 140). By their titles, the slice 5, 7 and 10
  findings are robustness and security cases, not omitted spec behavior (I read the titles and summaries, not every
  finding body).
- **Documents disagreeing with each other: 3.** Slice 13 S-5 (the `gates --dry-run` hint omits the required `--issue`),
  S-6 (the checklist promises "every skip reason", the spec's rule 6 stops at the cap) and S-8 (the README says
  `--issue` starts exactly the issues named, the spec refuses a blocked one).
- **Behavior the spec was silent about: about 31.** Failure modes, interrupted runs, links and paths an agent
  controls, hung or killed processes, output limits, secrets in text, Windows quirks, environment between this
  machine and a sandbox (slice 13 M-1, M-2 and PR 57 M-1 are examples). These dominate every review.

M2a's baseline had one finding of the first kind (M2A-2: `[eval.cosine]` accepted and silently ignored). The baseline
review covered 22 commits; the PR 57 review covered 66.

## What I conclude (with the caveats)

1. **Fewer findings about missing behavior: probably yes, but weakly.** 5 of 39 findings in M2b against 1 of 4 in M2a is
   not a difference one can lean on; the surface was larger, M2b had five review passes where M2a had one, and I
   chose the classification. What did change is that the independent PR review checked a 66-commit diff against the
   spec and found one omitted field and a few violated rules, not a pile of forgotten features.
2. **The spec worked as a review checklist.** Reviewers cite spec sections and decision numbers as the standard, which
   made findings short and checkable (and made them easy to turn into tests).
3. **Duplication did not go down.** Behavior is now stated in the spec, the README, the milestone checklist and the
   command help, and three findings are those copies drifting apart (S-5, S-6, S-8). The split removed repetition
   between PRD and plan but added a fourth place to keep in step.
4. **The spec did not cover what reviews mostly find.** About four in five findings are failure modes and trust
   boundaries nobody had written down. A PRD/spec pair does not by itself prompt for them.

## Options for the user (nothing is applied)

- **A. Keep the split for features and name the steps in `sdlc-planning`** (PRD after the first interview rounds, spec
  after the spikes). Cost: skill text; the evidence for A is points 1 and 2.
- **B. A, plus a required "failure modes and trust boundaries" section in the spec template** (what an agent controls,
  what happens when a process is killed mid-step, what happens when a command hangs or a path is a link). This aims at
  point 4, the largest class. My recommendation.
- **C. B, plus: README, checklist and command help cite spec sections instead of restating behavior**, and a
  documentation consistency check where a phrase is testable. Aims at point 3.
- **D. Drop the split.** The evidence doesn't support this, but the sample is one milestone.

One more milestone run both ways would settle points 1 and 3; one run per variant is all the evidence there is.
