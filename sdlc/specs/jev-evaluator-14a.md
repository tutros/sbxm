<!-- Draft of the GitHub issue for M2a slice 14a (Jev), kept here so it is not lost. NOT FILED. Written 2026-10-02 and edited 2026-10-03 only to renumber the proposed decision from 167 to 168, because decision 167 became cosine's evals.json key. Spike S12 ran on 2026-10-03 with a real token (sdlc/spikes/S12.md); its corrections are the block directly below and override the text further down where they differ. Before filing: confirm the design with the user (sdlc/jev.md section 5) and fold the corrections into the body. -->

> **Corrections from spike S12 (real token, 2026-10-03; they override the text below):**
> 1. **`model` is required in every request** (omitting it is a 422). Send `jev-latest` (answered as `jev-1.13.0`) or a pinned name such as `jev-1.13.0`; record the **response's** `model` in `evals.json` and `run.json`.
> 2. **Error shapes:** 401 and 400 carry `detail.error_type` (plus `message`, except `max_tokens_exceeded`); 422 carries a list whose `input` **echoes the whole request**. Never write a raw error body to a result, log or warning; keep `error_type`, or `loc` and `msg`.
> 3. **Oversize is a 400 `max_tokens_exceeded`** (not a 422), with no token count. 130,000 characters of prose (31,958 input tokens in all) passed; 160,000 did not.
> 4. **Token estimate:** about 4.0 characters per token for prose and 3.0 to 3.6 for a code diff, plus a fixed overhead of about 295 tokens per request and about 50 to 70 per extra question. The 1-per-3 estimate and the 28k cap stay safe.
> 5. **Answers are not deterministic:** a Noul was identical in five calls, a Score varied slightly (1.80 to 1.86 of 2, `confidence` 0.70 to 0.79). Record `confidence`; whether to average several calls is the user's decision.
> 6. **Choice** (for slice 14b and a router): `criteria` is an object id -> description; the answer is `{choice, confidence, probabilities}`.
> 7. Not tested: 429 and 529 retries (the retry rules below are still from the docs), redirects, concurrent calls, `state` as an array.

**Slice:** M2a slice 14a (deferred by decisions 99 and 138; the plan's slice 14 is split here: 14a is this issue, 14b is "Jev-specific raw questions" in a later issue). See `sdlc/milestone-2.md` row 14, `sdlc/spikes/S9.md` (merged; read it first), decisions 9, 18, 19, 20, 36, 112, 120, 138 and 168 (below, proposed).

## Problem

`[eval.jev]` is an unknown key today. The plan wants Jev (TypeSafe's System One model) as a second evaluator over the **shared rubric**, next to the LLM judge: structured answers with probabilities and confidence, scored per candidate independently. Two problems made it hard until now, and both are answered:

1. **The token** must not be readable on the host or in a sandbox (decision 36). Spike S9 verified that `sbx secret set-custom` does this: the proxy swaps a placeholder for the real value in request **headers** to one named host, and nowhere else.
2. **The 32k-token limit** on `state` plus the longest question: Jev never receives a transcript (decision 20), and a big diff is split.

## Jev's API (from https://docs.typesafe.ai/api.md, 2026-10-02; checked against a real token in spike S12 on 2026-10-03, see the corrections at the top)

```
POST https://api.typesafe.ai/v1/systemone
Authorization: Bearer <API_KEY>        Content-Type: application/json
{ "model": "jev-latest", "state": "<string|object|array>",
  "questions": { "<id>": { "type": "noul",  "instructions": "...", "criteria": {"true": "...", "false": "..."} },
                 "<id>": { "type": "score", "instructions": "...", "criteria": ["level 0", "level 1", "..."] } } }
-> { "model": "...", "answers": { "<id>": { "type": "noul", "noul": 0.95 },
                                  "<id>": { "type": "score", "score": 1.05, "legend": {...}, "probabilities": {...}, "confidence": 0.81 } },
     "usage": { "input_tokens": 296, "output_tokens": 20 } }
errors: 401 bad key, 422 validation, 429 rate limit (retry with exponential backoff), 529 overloaded
```

Composite scoring (docs `patterns/composite-scoring`): one Score or Noul question per dimension, weights applied **in our code**. A Score's `score` runs from 0 to (levels - 1), so `score / (levels - 1)` is the same 0-1 mapping `eval::rubric` already uses.

## Design (decision 168, proposed)

- **Where the call runs:** in a throwaway evaluator sandbox that runs one **fixed command**, never an agent (decisions 112, 120). Create it the way `eval::judge::judge_repeat` creates the judge's sandbox (own kits, own workspace under the run's roots, removed afterwards, a removal failure is kept as `remove_error` and never loses the result). Name: `sbxm-run-<run id>-jev-<repeat>`. Its egress is `api.typesafe.ai` only, from a built-in evaluator profile (not the contestants' profile).
- **The command:** sbxm writes `request.json` into the evaluator's workspace; the sandbox runs under the in-sandbox `timeout` (as `run::checks` does): `curl -sS -X POST https://api.typesafe.ai/v1/systemone -H "Authorization: Bearer $JEV_API_KEY" -H "Content-Type: application/json" --data @request.json -o response.json -w "%{http_code}"`. The exec's stdout is the status code. sbxm reads `response.json` back with the link-free, size-capped reader `repo::read_agent_file` uses. The token appears only as a header, which is the only place the proxy swaps it.
- **The secret:** scoped to that sandbox and created **before** it: `sbx secret set-custom --sandbox <name> --host api.typesafe.ai --env JEV_API_KEY --placeholder sbxm-jev-<run id>-<repeat> --command <token_command>` (or `--ref <token_ref>`), never `--value`, never an environment variable of sbxm's. Removed after the call, on every path, by `sbx secret rm --placeholder <p> --sandbox <name> -f` (a positional argument is read as a *service name*; `--all` removes the real secrets: never use it).
- **Config:** `[eval.jev]` with exactly one of `token_command` (an absolute path to a helper outside any workspace and the temp dir, as `sbx` requires) or `token_ref` (`op://...` or an ARN), plus optional `model` (default `jev-latest`). It requires a non-empty rubric. Unknown keys are errors. The value is never in config, results, errors or logs; a test proves a canary token printed by a fake helper never appears in any file under the run's roots.
- **Question mapping:** a `pass_fail` criterion becomes a Noul (`criteria.true` = the criterion's notes or id as satisfied, `criteria.false` = not satisfied); its 0-1 answer is used as is (a probability, so it is continuous where the judge's is 0 or 1). A `scale` criterion becomes a Score whose `criteria` are the rubric's levels in order; the result is `score / (levels - 1)`, clamped to 0-1. `instructions` carry the criterion id, its notes and the task text. Keep `confidence` (Score answers) per criterion in `evals.json`; it is recorded and shown, not used in scoring (provisional).
- **What Jev sees:** the task, the contestant's final answer and its diff, one candidate per request (so there is nothing to blind and nothing to leak between contestants), never the transcript. **Budget:** state plus the longest question must stay under 32k tokens; estimate conservatively at 1 token per 3 characters and keep 28k as the cap. If the diff does not fit, split it per file into chunks, one request per chunk (each with the task and the answer), and combine each criterion's chunk results by the mean weighted by chunk size in characters. Record `chunks` in `evals.json`. The combination rule is provisional.
- **Errors:** 429 and 529 retry with exponential backoff (2 s, 4 s, 8 s, then give up), each retry being a fresh exec in the same sandbox; 401 is a one-line "token rejected" naming `token_command`/`token_ref` and never retried; 422 is recorded with the response's message. A failed evaluation is `{"status": "error", "error": "<one line>"}` for that pair; it never fails the run or changes the judge's scores.
- **Results and ranking:** `evals.json` key `jev`: `{"status": "ok", "model": ..., "chunks": n, "criteria": {"<id>": {"score": 0.82, "confidence": 0.7}}, "usage": {...}}`. `eval::score` ranks by **each evaluator separately** (the judge's ranking is unchanged; add a Jev ranking beside it, same weights and rules: weight-normalised mean over the criteria it scored, repeats averaged, unscored excluded and listed). They are **not blended**: disagreement between them is information. `run` and `run show` print both. `score::load` returns a ranking when either evaluator ran.

## What to build

1. **`SandboxBackend` additions and the fake:** `custom_secrets() -> Result<Vec<CustomSecret>>`, `set_custom_secret(&CustomSecretSpec)` and `remove_custom_secret(placeholder, sandbox)`; `SbxBackend` runs the verified commands; the listing parser is tested against a captured fixture of `sbx secret ls --json` (shape in `S9.md`: a `custom_secrets` array of `{scope, targets[], env, placeholder, secret?}`); `FakeBackend` records the calls in its ordered log and can script a failure for each.
2. **Config** (`[eval.jev]`, validation as above) and its tests in the style of `tests/run_config.rs`.
3. **Preflight (before any write or backend create):** `sbx` is new enough for `set-custom` (it is experimental: pin the minimum version you verify and say so); the helper at `token_command` exists and is an absolute path (do not run it: `sbx` verifies it at `set-custom`); and no global custom secret with the same env name and host exists (it makes `sbx create` fail with `400 invalid custom secrets`; refuse and print the removal command).
4. **`src/eval/jev.rs`:** request building (mapping, chunking), response parsing (validate every answer type and range; a missing question is "unscored", never 0), the per-repeat runner (secret, sandbox, exec with retries, removal on every path), results writing through `results::merge_evals`.
5. **`src/eval/score.rs` and display:** a second ranking as described; `run show` reads only saved files.
6. **Docs:** README (`[eval.jev]`, how to store the token with `--command` or `--ref` and **not** `--value`, because `sbx secret ls` shows 12 characters of a `--value` secret; what the numbers mean), `sdlc/milestone-2.md` row 14, AGENTS.md. Add decision 168 as written above to `sdlc/decisions.md`, marked proposed.

## Acceptance criteria

- [ ] `[eval.jev]` loads with `token_command` or with `token_ref`; both, neither, a relative `token_command` path, an unknown key, or an empty rubric are refused in one line naming the file; one test each.
- [ ] Preflight refuses, before any write or backend create (test asserts no run folder and no `create` in the fake's log): an `sbx` that is too old, a missing helper path, and an existing global secret with the same env and host.
- [ ] With the fake backend and a fake HTTP result, a run with `[eval.jev]` creates the scoped secret **before** the sandbox, runs one exec, removes the sandbox and then the secret; the fake's ordered log shows exactly that order. On a failed exec, a 401, a timeout and a failed sandbox create, the secret is still removed (one test each).
- [ ] The request JSON for a rubric with a `pass_fail` and a `scale` criterion matches a golden file (Noul and Score shapes above); the Score's `criteria` are the levels in order; the answer text and diff are in `state`, the transcript never is (test asserts a transcript sentinel is absent).
- [ ] Response parsing: Noul 0.95 gives 0.95; Score 1.05 over 3 levels gives 0.525; out-of-range values are clamped; a missing answer is unscored (never 0); a malformed response is an `error` result.
- [ ] A diff over the 28k-token estimate is split per file; chunk results combine as the size-weighted mean; `chunks` is recorded; a single file bigger than the budget is truncated with `"truncated": true` (never sent whole).
- [ ] 429 and 529 retry with the stated backoff (the fake clock or an injected sleeper is asserted, no real sleeping in tests) and stop after three retries; 401 is never retried and names the config key.
- [ ] `evals.json` keeps its other keys (checks, judge, and cosine if present) when `jev` is merged.
- [ ] Two rankings: the judge's output is byte-identical to today's when `[eval.jev]` is absent (existing tests unchanged); with both configured, `run` and `run show` print both rankings, never a blended number; with Jev only, `run show` still prints a ranking.
- [ ] A canary token printed by the fake helper never appears in any file under the run's roots, in an error message or in a warning (test scans the run folders).
- [ ] `#[ignore]` real-`sbx` test (like `tests/real_sbx.rs`): creates and removes a scoped custom secret around a sandbox on a header-echo host, using a fake token, and proves the placeholder is swapped in the header only; and an `#[ignore]` live test, run only with `SBXM_TEST_JEV_TOKEN_COMMAND` set, makes one real Jev call against a two-criterion rubric. State in `result.md` that these two were not run in the sandbox.
- [ ] No test needs the network, `sbx` or a token; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass.
- [ ] README, milestone plan row 14, AGENTS.md and decisions (168 proposed) updated; `result.md` says what was and was not run.

## Out of scope (later issues)

- **14b:** Jev-specific raw questions, `[[eval.jev.questions]]` with `noul`/`choice`/`score` (decision 20).
- A `[[secrets.custom]]` table in profiles, `doctor` checks for orphan custom secrets, and `sbxm rm` removing scoped secrets (workflow notes G11, G13, G15 in `sdlc/evals-workflow-notes.md`).
- `sbxm exec` (G14).

**Depends on:** none known (S9 is merged)
**Related:** #63 (cosine): both change `src/commands/run.rs`, `src/eval/score.rs` and `run show`; start this one after #63 has merged to avoid conflicts.
