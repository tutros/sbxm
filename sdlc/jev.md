# Jev: status, roles and what is left (2026-10-03)

Jev is TypeSafe's System One model (docs: https://docs.typesafe.ai/llms.txt, Jev 1.13 when this was written). It returns
typed answers with probabilities and confidence, not text: **Noul** (P(yes)), **Choice** (one of up to 255 options plus a
distribution) and **Score** (2 to 10 ordered levels, probability-weighted). This file is the one place that says where Jev
stands. Nothing here is a decision: the roles in section 2 are the user's stated direction (2026-10-03), the design in
section 4 is a proposal. The API statements were checked against a real token on 2026-10-03 (spike S12,
`sdlc/spikes/S12.md`); what S12 did not test is listed in section 3.

## 1. Status in one paragraph

Jev is **not built and not started**. Slice 14 of the M2a plan (the Jev evaluator) was deferred twice (decisions 99 and
138). The two blockers that existed are cleared: how the token reaches the call (spike S9, merged in PR #65) and cosine
(PR #69, which touched the same files). **Spike S12 ran on 2026-10-03 with a real token and the API works as documented,
with five corrections to the draft** (`model` is required, three error shapes, a 400 for oversize, a tokens-per-character
measurement, small run-to-run noise). What is left before code: the user confirms the design and the section 5 questions,
the draft issue (`sdlc/specs/jev-evaluator-14a.md`, updated with S12's corrections) is filed, and the work is built. Until
then there is no `[eval.jev]` key; it is an unknown key and refused.

## 2. The roles Jev is meant to play

1. **A second evaluator over the shared rubric (the plan today, slice 14a).** Next to the LLM judge, scoring each
   contestant independently on the rubric's criteria, from the task, the final answer and the diff, never the
   transcript (decision 20). `pass_fail` criteria become Noul questions and `scale` criteria become Score questions
   (decision 18).
2. **The primary classifier for evaluating responses (the user's direction, 2026-10-03).** This is a bigger role than the
   plan's "second evaluator" and has consequences that are not decided yet (section 5): which ranking is the headline,
   how stable its answers must be, and which model version a result came from.
3. **Eventually, the basis of a model router (the user's direction, 2026-10-03):** classify a problem, then choose which
   model should work on it. The input is different from evaluation: it is the **task text**, before any model has
   answered, and the output is a choice among models (a Choice question) with a distribution, not a score for an answer.
   Nothing for this exists in the plan or in decisions; it needs its own planning.

## 3. What is known

From the docs:
- `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer <key>`, body `{model, state, questions}`; answers
  keyed by question id; `usage` returned. Errors: 401 bad key, 422 validation, 429 rate limit (retry with exponential
  backoff), 529 overloaded. (S12 confirmed all but the last two, which it did not provoke.)
- Limits: 64k tokens per request and **32k for `state` plus the longest question**; text only. Price: $0.042 per 1M input
  tokens, output tokens free.
- Composite scoring pattern: one Score or Noul question per dimension, weights applied in our code. A Score runs from 0 to
  (levels - 1), so `score / (levels - 1)` is the 0-1 mapping `eval::rubric` already uses.
- SDKs exist for Python and JS only; Rust calls the HTTP API with `reqwest` (decision 9, plan P7).

Verified (spike S9, `sdlc/spikes/S9.md`, PR #65, with a **fake** token on `sbx` 0.46.0):
- `sbx secret set-custom` lets the token reach the call without being readable on the host or in a sandbox: the proxy
  swaps a placeholder for the real value in request **headers** to the one named host, and nowhere else (not a body, not a
  query string, not another host).
- A secret scoped to one sandbox (`--sandbox NAME`) can be created before that sandbox exists, and sets the placeholder
  env var inside it. A global secret would put the placeholder in every sandbox.
- A global and a scoped secret with the same env name and host make `sbx create` fail (`400 invalid custom secrets`).
- `sbx secret ls` shows the first 8 and last 4 characters of a `--value` secret, so the token must be stored with
  `--command` (a helper that prints it) or `--ref`, never `--value`. `set-custom` is experimental in `sbx`.

Verified with a **real token** (spike S12, `sdlc/spikes/S12.md`, 2026-10-03, about 40 calls):
- The endpoint and the Bearer header work through the proxy swap; a call takes 0.8 to 1.0 s. The real secrets were
  untouched, and the token was never read by the assistant.
- `model` is **required**. `jev-latest` is answered as `jev-1.13.0`, and that name can be sent explicitly: so a result can
  record, and a run can pin, the exact version.
- Noul and Score answers match the docs. **Choice** needs `criteria` as an object (id -> description) and answers
  `{choice, confidence, probabilities}`: the shape a router would use.
- Errors: 401 `authentication_error`, 400 `api_usage_error` (unknown model), 400 `max_tokens_exceeded` (no message, no
  count), 422 with a list that **echoes the whole request** (so never log a raw error body).
- Tokens: a fixed overhead of about 295 input tokens per request, about 4.0 characters per token for prose and 3.0 to 3.6
  for a code diff (the design's 1 per 3 is safe). 130,000 characters of prose was accepted, 160,000 was not.
- Determinism: a Noul answer was identical in five calls; a Score varied slightly (1.80 to 1.86 of 2, confidence 0.70 to
  0.79). Not deterministic, but small in this sample.

Still not verified:
- The 429 and 529 behavior (not provoked), HTTPS redirects, concurrent calls, `state` as an array, Scores with more than
  3 levels, and the exact size boundary.
- **Whether Jev agrees with the LLM judge and with humans.** This is the evidence that decides whether it can be the
  primary classifier, and nothing has measured it yet.
- Stability over days and across model versions (only `jev-1.13.0` was seen), and whether `jev-latest` moves silently.
- Whether any Choice question picks sensible models for a task: only a toy task type was tried.

## 4. The proposed design (slice 14a)

Written in full in [`sdlc/specs/jev-evaluator-14a.md`](specs/jev-evaluator-14a.md) (the unfiled issue draft). In short:
- The call runs from a **throwaway evaluator sandbox that runs one fixed `curl` command**, never an agent (decisions 112
  and 120), with egress to `api.typesafe.ai` only and a secret scoped to that sandbox and removed after the call.
- `[eval.jev]` names the token source (`token_command` or `token_ref`, never a value) and an optional `model`; it needs a
  rubric. The token never reaches config, results, errors or logs; a canary test enforces it.
- One candidate per request, so nothing is blinded or shared between contestants. A diff that does not fit the budget is
  split per file and the chunk results are combined (a provisional rule).
- Results go in each pair's `evals.json` under `jev`; Jev gets **its own ranking beside the judge's, never blended**
  (disagreement is information). 429 and 529 retry with backoff, 401 never; a failed evaluation never fails the run.
- Jev-specific raw questions (`[[eval.jev.questions]]`) are slice 14b, a later issue (decision 20).
- The proposed decision is numbered **168** here. The draft and the S9 text called it 167, but decision 167 is now
  cosine's `evals.json` key; use the next free number when it is written into `sdlc/decisions.md`.

## 5. What the "primary classifier" and "router" direction changes (for the interview, not decided)

- **Which ranking is the headline?** The proposal keeps the judge's and Jev's rankings separate and unblended. If Jev is
  the primary classifier, the output needs a clear headline, with the judge as a cross-check, or the reverse. That is the
  user's call.
- **Stability.** A primary classifier needs known variance. S12 should measure it (same request repeated), and the answer
  decides whether `--repeat` becomes mandatory for Jev scoring and whether results should show a spread.
- **Reproducibility.** Record the exact model name and the Jev version with every result (`evals.json` and `run.json`),
  and find out whether a pinned model exists. Without that, a score from one month cannot be compared with another.
- **Calibration against the judge and humans.** The plan already has a human-review report. Comparing Jev's answers with
  human verdicts on the same pairs is the evidence that it is fit to be primary; the run folders already hold what is
  needed (answers, diffs, rubric, the judge's scores).
- **The router needs data the runs can provide.** Each `sbxm run` already saves the task, every contestant's
  harness and model, and its scores. Across many runs that is a labeled dataset of "which model did well on which kind of
  task". To make it usable, rubric ids and task descriptions should stay consistent across runs, and a run should record a
  task type or tags (not in the run model today). This is a suggestion for planning, not a requirement anyone has set.
- **A different input and output.** Evaluation classifies answers against a rubric; routing classifies a task into a model
  choice. It uses the same HTTP call but needs a different question builder, a place to record routing decisions, and a
  way to check them (did the routed model do well?). Plan it as its own milestone after Jev evaluation works, not inside
  slice 14.
- **Cost is not the constraint.** At $0.042 per 1M input tokens, a 28k-token request costs about $0.001. The limits that
  matter are the 32k `state` budget, stability and reproducibility.

## 6. The path from here

1. **Done: a real Jev token.** It is behind a helper script that prints it (outside any workspace); it is passed as
   `set-custom --command`, never `--value`, and the assistant never reads the token file.
2. **Done: spike S12** (2026-10-03, `sdlc/spikes/S12.md`).
3. **User: confirm the design** (section 4, with S12's five corrections) and decide the section 5 questions that affect it
   (headline ranking, how many calls per score given the small noise, pinning the model version). Then write the decision
   into `sdlc/decisions.md` with the next free number.
4. **Optional, before relying on Jev as the primary classifier: a calibration run.** Score the pairs of a few existing
   runs with Jev, the judge and a human, and compare. S12 measured the API, not whether Jev's verdicts are good.
5. **File the slice 14a issue** from the draft (its corrections from S12 are listed at the top of the draft) and build it
   with `sbxm task` (the PR-review gaps in `sdlc/evals-workflow-notes.md`, G16 onwards, apply: expect to fix review
   findings by hand until they are solved).
6. **Later:** slice 14b (raw questions), then a planning interview for the router (the Choice shape is confirmed).

## 7. Where things are

- Plan: `sdlc/milestone-2.md` row 14 and its "Spike S9" section; decisions 9, 18, 19, 20, 36, 99, 112, 120, 138 and the
  "Jev facts" section of `sdlc/decisions.md`.
- Spike results: `sdlc/spikes/S9.md` (token path, fake token) and `sdlc/spikes/S12.md` (the real API). Issue draft:
  `sdlc/specs/jev-evaluator-14a.md`.
- Gaps found around custom secrets (G11 to G15): `sdlc/evals-workflow-notes.md`.
- Config in the README: not yet; `[eval.jev]` does not exist.
