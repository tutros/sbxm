# Which questions to ask Jev: evaluating a response and routing a prompt (planning draft, 2026-10-03)

Status: **planning notes from an interview with the user, not decisions.** Nothing here is built. Context: `sdlc/jev.md`
(where Jev stands) and `sdlc/spikes/S12.md` (the API against a real token). The user's position (2026-10-03): Jev's
Noul, Score and Choice answers are taken to separate good from bad answers, to track a ground truth and to be stable, so
no calibration run of those properties is wanted now. **The open problem is the questions themselves:** which questions,
and which kinds, make Jev a good classifier for evaluating an LLM's response and for routing a prompt to a model.

## 1. The real problem we use to design them

**"Resolve a GitHub issue of this repo with a headless coding agent"** (the `sbxm task` workflow). Why this one:
- Every issue is structured text (`Where`, `What happens`, `Fix`) with a checklist of **acceptance criteria**; each one is
  already a yes/no question.
- There is an objective ground truth beside Jev: the gates (`cargo fmt --check`, `clippy`, `test`) and the independent
  review's `Must-fix findings: <n>`.
- Real material exists: open issues #58 to #61 (concurrency, recovery, input sanitising, validation: four kinds of task)
  and the history, including the two issue 63 worker runs, whose reviews found concrete defects.

## 2. What "best" means (user's decision 1, 2026-10-03)

Taken from the user, restated here; the formalisation in the last bullet is ours and needs the user's OK.
- **Inputs:** other benchmarks, plus our own comparisons of agents on one task (`sbxm run`). For each **type of task**,
  the models that win on acceptable results form the shortlist.
- **Scaled by cost:** tokens, plus the dollars those tokens cost (input and output priced separately).
- **Not a fixed ranking.** The top models will change. A model that does well for a task type is kept until a newer or
  cheaper one gives acceptable results at better value; the routing table is data that gets re-run, not a constant.
- **Proposed formalisation (to confirm):** for each task type, among the models whose acceptance rate is within a
  tolerance *T* of the best (value of *T* to be chosen, for example 5 percentage points), route to the one with the lowest
  expected cost per accepted result (cost per run divided by the acceptance rate). Time is a tie-break, not a goal.

## 3. What "accepted" means

Two levels, both recorded for every run (so the choice of bar is a reporting decision, not a code change):
- **Acceptable:** all gates pass, and Jev's must-have criteria (section 4) are above a bar.
- **Ideal (the goal):** acceptable **and** no must-fix findings in the independent review. The user expects this to be hard
  to reach; it is a target to measure progress against, not the gate for routing at first.

## 4. Draft: criteria for evaluating a response

Jev sees the issue text, the diff and the worker's `result.md`, one candidate per request, never the transcript
(decision 20). Verdicts that a tool can decide (tests, formatting) stay with the gates; Jev takes the judgment ones.
- **One Noul per acceptance criterion in the issue.** Example: "Does the diff satisfy: a closing keyword in embedded
  review text is rendered inert?" These are the must-haves for "acceptable".
- **Fixed dimensions, asked on every issue:**
  - Score: how fully does the diff implement the issue's `Fix`?
  - Noul: does the diff change anything not asked for? (scope discipline)
  - Noul: does a changed or added test exercise the failure path the issue describes?
  - Noul: does `result.md` claim something the diff does not do? (honesty)
  - Noul: does the change touch security-sensitive code (trust boundary, secrets, paths, process spawning)?
- What is not asked: style opinions the gates already enforce.

## 5. Draft: features for routing a prompt

Jev sees only the task text (the issue), before any model has run. It classifies the task; it cannot know how good our
models are. **The map from these features to a model comes from our own outcomes** (acceptance and cost per model per
kind of task), so routing needs a table that the comparison runs fill in. Asking Jev "which model?" directly is kept only as
a baseline the learned table has to beat.
- Choice: task type (bugfix, refactor, new feature, docs, tests only, security hardening, other).
- Score: scope size (about how many files), ambiguity of the specification, how much cross-module context is needed, risk,
  how hard the tests are to write.
- Noul: needs a new dependency? needs a design decision the issue does not make? depends on Windows-specific behavior?
- Each feature is recorded with every run, next to the model, the outcome and the cost, so the table can be learned later.
  (Today a run saves the task and the scores, but no task features.)

## 6. Tracking cost: what the harnesses give us

Checked in the saved fixtures and in `src/headless.rs` (small "pong" and tool runs only; larger runs, subagents and
multi-model sessions were not looked at):

| Harness | Tokens in the output | Dollars | Model name | What `Usage` keeps today |
|---|---|---|---|---|
| Claude Code | `input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens` | **`total_cost_usd` in the final result** | yes | one summed input figure (the cache split is added up and lost), output, and `cost_usd` |
| Codex | `input_tokens`, `cached_input_tokens`, `output_tokens`, `reasoning_output_tokens` | none | not seen in the fixtures | input and output summed; the cached and reasoning figures are dropped |
| Antigravity | `input_tokens`, `output_tokens`, `total_tokens` per step and in the final result | none | yes (`gemini-3.8-flash-low`) | input and output |

What follows (to be decided and planned):
- **Tokens can come from the harness logs for all three**, but `Usage` flattens them. Pricing needs the split (cache reads
  and writes and reasoning tokens are priced differently from fresh input and output), so `Usage` should keep every
  figure the harness reports, plus the model name, instead of two sums.
- **Dollars only come natively from Claude Code.** For Codex and Antigravity (and to cross-check Claude) they need a
  **price table**: dollars per million tokens per model, for input, cached input and output, with an effective date. No
  harness is an authoritative price source, and prices change as new and cheaper models arrive, so this is a versioned
  file in config that each run records the version of. That is not an external service, only a maintained table.
- **Subscription logins make dollars notional.** Most harness accounts here are OAuth subscriptions, so no dollars are
  actually charged per run; a dollar figure is list-price equivalent (Claude Code's own `total_cost_usd` is presumably
  computed from list prices; neither that nor whether it is reported under a subscription login was checked). Whether the routing cost is list-price dollars,
  quota used, or tokens alone is the user's call.
- **Wall time** is measured by sbxm itself (it needs no harness figure).
- Jev's own calls are cheap and report `usage` ($0.042 per 1M input tokens), so the classifier's cost can be included.

## 7. Open decisions for the user

1. **Confirm** the tolerance rule in section 2 (route to the cheapest model within *T* of the best) and choose *T*.
2. **Cost unit:** list-price dollars, tokens only, or quota share (section 6).
3. **Confirm** the two accepted levels (section 3) and the draft question sets (sections 4 and 5), after seeing them on the
   four open issues.
4. **Where the benchmark input comes from** (section 2: "other benchmarks"): which ones, and how they map to our task types.

## 8. Next steps

1. The user answers section 7.
2. Apply the question sets to #58 to #61 by hand, as Jev requests (a scratch script, like spike S12), to see what the
   answers look like on real tasks, before any sbxm code.
3. Run the existing `sbxm run` comparison on those tasks with the three harnesses, keeping usage and task features, so the
   routing table has its first rows.
4. Then plan the code: a richer `Usage`, a price table, task-feature capture, and the Jev evaluator (slice 14a).
