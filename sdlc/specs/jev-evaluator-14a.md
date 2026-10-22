<!-- The body of the GitHub issue for M2a slice 14a (Jev). Written 2026-10-02 as a draft, corrected by spike S12 (2026-10-03, sdlc/spikes/S12.md), and confirmed by the user on 2026-10-22 as decision 241, which also answered sdlc/jev.md section 5. File it as the issue body as is. -->

**Slice:** M2a slice 14a (deferred by decisions 99 and 138; the plan's slice 14 is split here: 14a is this issue, 14b is "Jev-specific raw questions" in a later issue). See `sdlc/milestone-2.md` row 14, `sdlc/spikes/S9.md` and `sdlc/spikes/S12.md` (read both first), and decisions 9, 18, 19, 20, 36, 112, 120, 138 and **241** (this design, confirmed).

**Size:** L (a new evaluator: backend secret calls, config, preflight, a sandboxed runner with retries and chunking, a second ranking, docs).

## Problem

`[eval.jev]` is an unknown key today. The plan wants Jev (TypeSafe's System One model) as a second evaluator over the **shared rubric**, next to the LLM judge: structured answers with probabilities and confidence, scored per candidate independently. Two problems made it hard until now, and both are answered:

1. **The token** must not be readable on the host or in a sandbox (decision 36). Spike S9 verified that `sbx secret set-custom` does this: the proxy swaps a placeholder for the real value in request **headers** to one named host, and nowhere else. Spike S12 used it with the real token.
2. **The 32k-token limit** on `state` plus the longest question: Jev never receives a transcript (decision 20), and a big diff is split.

## Jev's API (docs https://docs.typesafe.ai/api.md, checked against a real token in spike S12, 2026-10-03)

```
POST https://api.typesafe.ai/v1/systemone
Authorization: Bearer <API_KEY>        Content-Type: application/json
{ "model": "jev-latest", "state": "<string|object>",
  "questions": { "<id>": { "type": "noul",  "instructions": "...", "criteria": {"true": "...", "false": "..."} },
                 "<id>": { "type": "score", "instructions": "...", "criteria": ["level 0", "level 1", "..."] } } }
-> { "model": "jev-1.13.0",
     "answers": { "<id>": { "type": "noul", "noul": 0.95 },
                  "<id>": { "type": "score", "score": 1.82, "confidence": 0.73, "legend": {...}, "probabilities": {...} } },
     "usage": { "input_tokens": 369, "output_tokens": 33 } }
```

- **`model` is required** (omitting it is a 422). `jev-latest` is answered as `jev-1.13.0` in the response's `model`; an explicit `jev-1.13.0` is accepted; an unknown name is a 400 `api_usage_error`.
- **Errors** (all JSON under `detail`): 401 `{"detail":{"error_type":"authentication_error","message":...}}`; 400 `{"detail":{"error_type":"api_usage_error","message":...}}` or `{"detail":{"error_type":"max_tokens_exceeded"}}` (oversize, no count); 422 `{"detail":[{"type","loc","msg","input"}]}`, where `input` **echoes the whole request** (the contestant's answer and diff). 429 (rate limit) and 529 (overloaded) are from the docs only; S12 did not provoke them.
- **Size:** 130,000 characters of prose (31,958 input tokens) passed, 160,000 did not. Prose is about 4.0 characters per token, a code diff 3.0 to 3.6; each request has a fixed overhead of about 295 tokens plus about 50 to 70 per extra question.
- **Noise:** five identical calls gave the same Noul (0.99) and a Score of 1.80 to 1.86 of 2 (`confidence` 0.70 to 0.79).

Composite scoring (docs `patterns/composite-scoring`): one Score or Noul question per dimension, weights applied **in our code**. A Score's `score` runs from 0 to (levels - 1), so `score / (levels - 1)` is the same 0-1 mapping `eval::rubric` already uses.

## Design (decision 241)

- **Where the call runs:** in a throwaway evaluator sandbox that runs one **fixed command**, never an agent (decisions 112, 120). Create it the way `eval::judge::judge_repeat` creates the judge's sandbox (own kits, own workspace under the run's roots, removed afterwards, a removal failure is kept as `remove_error` and never loses the result). Name: `sbxm-run-<run id>-jev-<repeat>`. Its egress is `api.typesafe.ai` only, from a built-in evaluator profile (not the contestants' profile).
- **The command:** sbxm writes `request.json` into the evaluator's workspace; the sandbox runs under the in-sandbox `timeout` (as `run::checks` does): `curl -sS -X POST https://api.typesafe.ai/v1/systemone -H "Authorization: Bearer $JEV_API_KEY" -H "Content-Type: application/json" --data @request.json -o response.json -w "%{http_code}"`. The header is in **double** quotes inside the sandbox shell so `$JEV_API_KEY` (the placeholder) expands there (S12's first calls were 401 because of single quotes). The exec's stdout is the status code. sbxm reads `response.json` back with the link-free, size-capped reader `repo::read_agent_file` uses. The token appears only as a header, which is the only place the proxy swaps it.
- **The secret:** scoped to that sandbox and created **before** it: `sbx secret set-custom --sandbox <name> --host api.typesafe.ai --env JEV_API_KEY --placeholder sbxm-jev-<run id>-<repeat> --command <token_command>` (or `--ref <token_ref>`), never `--value`, never an environment variable of sbxm's. Removed after the call, on every path, by `sbx secret rm --placeholder <p> --sandbox <name> -f` (a positional argument is read as a *service name*; `--all` removes the real secrets: never use it). Removing the sandbox may already have removed the secret (S12 saw that; S9 saw it orphaned), so a "no custom secret found" answer from that `rm` counts as success.
- **Config:** `[eval.jev]` with exactly one of `token_command` (an absolute path to a helper outside any workspace and the temp dir, as `sbx` requires) or `token_ref` (`op://...` or an ARN), plus optional `model` (default `jev-latest`, always sent, since the API requires it). It requires a non-empty rubric. Unknown keys are errors. The value is never in config, results, errors or logs; a test proves a canary token printed by a fake helper never appears in any file under the run's roots.
- **Calls:** **one call per contestant per repeat** (decision 241 b), plus one per extra chunk when a diff is split. No averaging of repeated calls inside a repeat; `--repeat` is how a user reduces noise.
- **Question mapping:** a `pass_fail` criterion becomes a Noul (`criteria.true` = the criterion's notes or id as satisfied, `criteria.false` = not satisfied); its 0-1 answer is used as is (a probability, so it is continuous where the judge's is 0 or 1). A `scale` criterion becomes a Score whose `criteria` are the rubric's levels in order; the result is `score / (levels - 1)`, clamped to 0-1. `instructions` carry the criterion id, its notes and the task text. Keep `confidence` (Score answers) per criterion in `evals.json`; it is recorded and shown, not used in scoring.
- **What Jev sees:** the task, the contestant's final answer and its diff, one candidate per request (so there is nothing to blind and nothing to leak between contestants), never the transcript. `state` is a string. **Budget:** estimate 1 token per 3 characters, add 300 tokens of overhead and 70 per question beyond the first, and keep the total under a 28k cap. If the diff does not fit, split it per file into chunks, one request per chunk (each with the task and the answer), and combine each criterion's chunk results by the mean weighted by chunk size in characters. Record `chunks` in `evals.json`. A single file bigger than the budget is truncated and recorded with `"truncated": true` (never sent whole).
- **Errors:** 429 and 529 retry with exponential backoff (2 s, 4 s, 8 s, then give up), each retry being a fresh exec in the same sandbox; 401 is a one-line "token rejected" naming `token_command`/`token_ref` and never retried; 400 and 422 are not retried. **A raw error body is never written** to a result, a log or a warning (a 422 echoes the request): keep `error_type` (and `message` for 401 and 400 `api_usage_error`), or each 422 entry's `loc` and `msg` only. A failed evaluation is `{"status": "error", "error": "<one line>"}` for that pair; it never fails the run or changes the judge's scores.
- **Results:** `evals.json` key `jev`: `{"status": "ok", "model": "<the response's model>", "chunks": n, "criteria": {"<id>": {"score": 0.82, "confidence": 0.7}}, "usage": {...}}` (`truncated: true` only when a file was cut). `run.json` gets `jev_models`, the sorted list of distinct response models over the run's pairs, written together with `completed_at` (decision 241 c); absent when the run has no `[eval.jev]`.
- **Ranking and display (decision 241 a):** `eval::score` ranks by **each evaluator separately**, never blended (disagreement is information). The **LLM judge's ranking is the headline** and stays byte-identical to today's; Jev's ranking is printed after it, under its own heading naming the model(s) that answered, with the same weights and rules (weight-normalised mean over the criteria it scored, repeats averaged, unscored excluded and listed). With Jev only, `run show` prints Jev's ranking alone. `score::load` returns a ranking when either evaluator ran. `run` and `run show` print the same thing; `run show` reads only saved files.

## Edge cases (answered)

- **Interrupted or rerun:** the sandbox and placeholder names carry the run id and repeat, so a rerun never meets a previous run's scoped secret; a secret orphaned by a crash is a `doctor` concern for a later issue (G13, out of scope).
- **Two runs at once:** names are per run id, so they never share a sandbox or a placeholder; preflight's global-secret check reads, never writes.
- **A file or folder that is a link:** `response.json` is read with the link-free, size-capped reader; a link or an oversize file is an `error` result for that pair.
- **Output failing midway:** results are written through `results::merge_evals` like the judge's, so a failure after one pair keeps the pairs already saved.

## What to build

1. **`SandboxBackend` additions and the fake:** `custom_secrets() -> Result<Vec<CustomSecret>>`, `set_custom_secret(&CustomSecretSpec)` and `remove_custom_secret(placeholder, sandbox)`; `SbxBackend` runs the verified commands; the listing parser is tested against a captured fixture of `sbx secret ls --json` (shape in `S9.md`: a `custom_secrets` array of `{scope, targets[], env, placeholder, secret?}`); `FakeBackend` records the calls in its ordered log and can script a failure for each.
2. **Config** (`[eval.jev]`, validation as above) and its tests in the style of `tests/run_config.rs`.
3. **Preflight (before any write or backend create):** `sbx` is at least 0.46.0 (the version S12 verified `set-custom` on; it is experimental); the helper at `token_command` exists and is an absolute path (do not run it: `sbx` verifies it at `set-custom`); and no global custom secret with the same env name and host exists (it makes `sbx create` fail with `400 invalid custom secrets`; refuse and print the removal command).
4. **`src/eval/jev.rs`:** request building (mapping, budget, chunking), response parsing (validate every answer type and range; a missing question is "unscored", never 0), the per-repeat runner (secret, sandbox, exec with retries, removal on every path), results writing through `results::merge_evals`, and `jev_models` in `run.json`.
5. **`src/eval/score.rs` and display:** the second ranking as described; `run show` reads only saved files.
6. **Docs:** README (`[eval.jev]`, how to store the token with `--command` or `--ref` and **not** `--value`, because `sbx secret ls` shows 12 characters of a `--value` secret; what the numbers mean; that the judge's ranking leads), `sdlc/milestone-2.md` row 14, AGENTS.md. Decision 241 is already in `sdlc/decisions.md`; do not add another.

## Acceptance criteria

- [ ] `[eval.jev]` loads with `token_command` or with `token_ref`; both, neither, a relative `token_command` path, an unknown key, or an empty rubric are refused in one line naming the file; one test each. `model` defaults to `jev-latest` and is always in the request.
- [ ] Preflight refuses, before any write or backend create (test asserts no run folder and no `create` in the fake's log): an `sbx` older than 0.46.0, a missing helper path, and an existing global secret with the same env and host.
- [ ] With the fake backend and a fake HTTP result, a run with `[eval.jev]` creates the scoped secret **before** the sandbox, runs one exec per contestant per repeat, removes the sandbox and then the secret; the fake's ordered log shows exactly that order. On a failed exec, a 401, a timeout and a failed sandbox create, the secret is still removed (one test each); a "no custom secret found" answer from the removal is not an error.
- [ ] The request JSON for a rubric with a `pass_fail` and a `scale` criterion matches a golden file (Noul and Score shapes above, `model` present); the Score's `criteria` are the levels in order; the answer text and diff are in `state`, the transcript never is (test asserts a transcript sentinel is absent).
- [ ] Response parsing: Noul 0.95 gives 0.95; Score 1.05 over 3 levels gives 0.525; out-of-range values are clamped; a missing answer is unscored (never 0); a malformed response is an `error` result; the response's `model` is recorded.
- [ ] A diff over the budget is split per file; chunk results combine as the size-weighted mean; `chunks` is recorded; a single file bigger than the budget is truncated with `"truncated": true` (never sent whole).
- [ ] 429 and 529 retry with the stated backoff (an injected sleeper is asserted, no real sleeping in tests) and stop after three retries; 401 is never retried and names the config key; 400 and 422 are not retried.
- [ ] A 422 whose body echoes a sentinel from the request leaves that sentinel in no result, log or warning; only `loc` and `msg` are kept.
- [ ] `evals.json` keeps its other keys (checks, judge, and cosine if present) when `jev` is merged; `run.json` has `jev_models` once the run completes.
- [ ] Two rankings: the judge's output is byte-identical to today's when `[eval.jev]` is absent (existing tests unchanged); with both configured, `run` and `run show` print the judge's ranking first and Jev's after it, headed with the model(s), never a blended number; with Jev only, `run show` still prints a ranking.
- [ ] A canary token printed by the fake helper never appears in any file under the run's roots, in an error message or in a warning (test scans the run folders).
- [ ] `#[ignore]` real-`sbx` test (like `tests/real_sbx.rs`): creates and removes a scoped custom secret around a sandbox on a header-echo host, using a fake token, and proves the placeholder is swapped in the header only; and an `#[ignore]` live test, run only with `SBXM_TEST_JEV_TOKEN_COMMAND` set, makes one real Jev call against a two-criterion rubric. State in `result.md` that these two were not run in the sandbox.
- [ ] No test needs the network, `sbx` or a token; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass.
- [ ] README, milestone plan row 14 and AGENTS.md updated; `result.md` says what was and was not run.

## Out of scope (later issues)

- **14b:** Jev-specific raw questions, `[[eval.jev.questions]]` with `noul`/`choice`/`score` (decision 20). S12 found Choice's `criteria` must be an object id -> description.
- A calibration run (Jev, the judge and a human on existing runs), which decides whether Jev can become the headline (decision 241 a).
- A `[[secrets.custom]]` table in profiles, `doctor` checks for orphan custom secrets, and `sbxm rm` removing scoped secrets (workflow notes G11, G13, G15 in `sdlc/evals-workflow-notes.md`).
- `sbxm exec` (G14).

**Depends on:** none (S9, S12 and cosine, #63, are merged).
