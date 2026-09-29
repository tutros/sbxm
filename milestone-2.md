# Milestone 2: comparisons, then folding issue-workers into sbxm

Goal: run 2–4 contestants (harness × model, [10]) headlessly and in parallel on the same task ([6]),
each in a fresh throwaway sandbox ([95]) seeded or empty ([5]), capturing answer, diff, transcript and
usage ([3]), then evaluating with executable checks, a rubric-based LLM judge, cosine similarity and a
human-readable report — Jev is added once that pipeline works end to end ([99]). This is **M2a**. Once
it ships, **M2b** folds `scripts/issue-workers.ps1`'s dispatch/review/fix-round workflow into sbxm
itself, reusing M2a's headless-execution primitive instead of the script's raw `sbx exec` calls ([88][90][94]).
One plan, two sub-milestones ([92][110]); M2b's detail here is a rough outline, refined once M2a's
primitive exists to build on.

Constraints come from `decisions.md` (numbers in brackets refer to it), especially the milestone-2
planning round [92]–[117] and the S5 spike results under "Spike results".

## Plan-level choices

**P1. `run` subcommand set**
→ `sbxm run <config>` (launch a comparison), `sbxm run init [path]` (write a starter run-config,
completing [96]), `sbxm run show <run-id>` (print a run's results as a human-readable report, [100]).
A `run list` (past runs under `.sbxm/runs/`) can be added later if it turns out to be needed; the first
three cover launch → inspect end to end.

**P2. Run ID format**
→ `<date>-<6 lowercase hex>`, e.g. `2026-09-29-a1b2c3`: sortable by date, collision-resistant enough for
one host, short enough to type in `run show <run-id>`.

**P3. The LLM judge runs through the same shared headless primitive as contestants — confirmed**
The judge needs an LLM call (rubric + answer, optionally diff, per [20]) but no tools or multi-turn
behavior. Rather than writing a second, provider-specific HTTP client with its own auth, the judge runs
as a throwaway sandbox too — same primitive, same `sbx`-managed secrets and egress, just a different
prompt and a harness/model named in `[eval.judge]`. This keeps sbxm's "no enforcement, no credential
handling of its own" role [36] intact for the judge as much as for contestants, and it fits the existing
`sbx`-managed auth/egress model directly. No longer open: settled as decision [116], ahead of slice 11
(the LLM-judge slice) where it's built.

**P4. Cosine similarity is local and out of the sandbox model entirely**
`fastembed` ([23]) runs on the host, no network, no secrets, text answers only ([23]). It doesn't touch
`SandboxBackend` at all.

**P5. Antigravity's mandatory-instructions file path is unverified**
Decision 37's per-harness convention (an always-loaded home file) was checked for Claude, Codex, Gemini
and Pi in M1, but never for Antigravity — the S5 spike only exercised its headless `-p` mode, not the
kit's `files/home/` behavior. → Verify empirically in the slice that adds the `Antigravity` harness
adapter (slice 3 below), the same way M1 slices 18–20 each did their own real-`sbx` check.

**P6. Executable checks run inside the contestant's sandbox before it's removed**
A `[[eval.checks]]` entry is `{id, command}`; each runs via `SandboxBackend::exec` inside the still-alive
contestant sandbox (after the headless run, before teardown) and is scored pass/fail by exit code.
Running it in the same sandbox (not a fresh one) means it sees exactly the files the agent left behind,
with no extra copy step.

**P7. `reqwest` is added in blocking mode**
The crate is entirely synchronous today (no `tokio`). Jev's HTTP client ([9], "HTTP API only... use
`reqwest`") uses `reqwest`'s blocking feature so no async runtime is introduced crate-wide. Only
`src/eval/jev.rs` touches it.

## Structure

New and changed modules, in dependency order. Everything in the existing crate layout (`milestone-1.md`)
is unchanged except where noted.

| Module | Responsibility |
|---|---|
| `backend` (changed) | `SandboxBackend` gains `Send + Sync` (needed for slice 6's parallel orchestration — `FakeBackend`'s current `RefCell` fields move to `Mutex`, `SbxBackend` is already a stateless unit struct) and two methods: `fn exec(&self, sandbox: &str, spec: &ExecSpec) -> Result<ExecOutput>`, shelling to `sbx exec [-w <workdir>] <sandbox> <argv...>` in `SbxBackend` (`ExecSpec { argv: Vec<String>, workdir: Option<String>, stdin: Stdin }`, `Stdin { Closed, Piped(String) }` — Codex needs `Piped(String::new())` to unblock its stdin read, [S5]; a shell one-liner like an `eval.checks` command is wrapped by the caller as `argv: vec!["sh", "-c", command]`, not a separate mode on `ExecSpec`), and `fn skills(&self) -> Result<serde_json::Value>` (`sbx skills ls --json`, opaque like `KitValidation.warnings` — sbxm only needs to record it verbatim, [46]). `FakeBackend` gets a scriptable exec log and a scriptable `skills` response. `exec` is the one primitive every headless run, executable check and judge call is built on. |
| `harness` (changed) | `Harness` gains `Antigravity` [103], pinned to an immutable kit tag [104]. Gains the headless-execution surface [94]: `headless_argv(prompt, opts) -> Vec<String>` (the inner command `exec` runs), `stdin(opts) -> Stdin` (Codex: `Piped(String::new())`, others: `Closed`, [S5]), `git_repo_workaround(is_git_repo) -> Option<&'static str>` (Codex: `"--skip-git-repo-check"` when not a repo, [113]), `budget_flag(budget_usd) -> Option<Vec<String>>` (Claude: `["--max-budget-usd", ...]`; Codex/Antigravity: `None`, [S5] — the orchestrator warns loudly when a run sets `budget_usd` and this returns `None` for a contestant, the same warning pattern as `Harness::unsupported` [11][98]), `parse_headless_output(raw: &str) -> Result<HeadlessResult>` per harness's actual event shape ([S5]: Claude's `stream-json`, Codex's and Antigravity's NDJSON). `HeadlessResult { status: RunStatus, answer: String, transcript: String (raw NDJSON/stream-json, kept as-is), usage: Usage }`, `RunStatus { Completed, TimedOut, Failed(String) }` — exit 124 (or 137 after `--kill-after`) is `TimedOut`, not an error: `parse_headless_output` is best-effort on truncated output (the last assistant text seen so far as `answer`, whatever usage events arrived, both possibly empty) so [16]'s partial output is returned and still evaluated; `Failed` covers a non-timeout non-zero exit or unparseable output, `Usage { input_tokens, output_tokens, cost_usd: Option<f64> }` ([105]: `None` for Codex/Antigravity). |
| `headless` (new) | The shared primitive [94] itself: `run(backend, sandbox, workdir, harness, prompt, opts, timeout) -> Result<HeadlessResult>`, returning `Ok` with `status: TimedOut` (never `Err`) on a timeout, and composing `Harness::headless_argv` + `stdin`/`git_repo_workaround`/`budget_flag` + the uniform `timeout --kill-after` wrapper [114] + `SandboxBackend::exec` (via `ExecSpec { argv, workdir: Some(workdir), stdin }`) + `Harness::parse_headless_output`. Used by contestants, the judge (P3) and, in M2b, issue workers. |
| `run::config` (new) | `RunConfig` schema (below): `Task`, `Contestant`, `RunLimits { timeout, budget_usd, cpus, memory, repeat: u32 }` (`repeat` defaults to 1, in the data model from day one per [15], not bolted on later), `EvalConfig`. Parsing and validation: contestant count 2–4 [109], `repeat >= 1`, harness restricted to `{Claude, Codex, Antigravity}` [103] (a config-time error naming the excluded harness and why, same pattern as [11]), seed dir checks reusing `src/seed.rs`'s `reject_links` [102]. |
| `run::kits` (new) | Per-run kit generation, reusing `kit::all`/`kit::write` and `backend.validate_kit` exactly as `commands/new.rs` does ([95]: a run always gets a fresh kit from the current merged config). Computes the effective config hash per harness with the run's `cpus`/`memory` overrides applied [55][108], writes the `common` and `harness-<h>` mixins to `.sbxm/runs/<run-id>/kits/<hash-prefix>/` (never mounted [112]), validates each before any sandbox exists, and hands `run::orchestrate` the ordered kit args for `CreateSpec`. The kit refs recorded in `run.json` come from here. |
| `run::orchestrate` (new) | For each `(contestant, repeat index)` pair, in parallel (`std::thread::scope`, needing `SandboxBackend: Send + Sync` above; with the default `repeat = 1` this is exactly one sandbox per contestant, same as before repeat existed): create a throwaway sandbox [95] from `run::kits`' validated kits, named `sbxm-run-<run-id>-<contestant-idx>-<repeat-idx>` sized from `GlobalConfig.resources`, overridable per run [108]; prepare the workspace (seeded: copy + fresh git repo + baseline commit via a new `seed::seed_contestant`, [102]; unseeded: empty directory, [113]); run the headless primitive; run executable checks (P6) before teardown; remove the sandbox always, including on timeout ([16]: partial output is kept and still evaluated). **From slice 8 onward**, as soon as a `(contestant, repeat)` pair's headless run and checks finish (success, timeout or error alike), `run::results` writes its `answer.md`/`diff.patch`/`transcript.jsonl` to disk immediately — before the judge or cosine evaluators run at all, so a failure in evaluation (or in another pair) never loses already-captured output [16][25]. Evaluators append to `evals.json` afterward. Slices 6–7 build the orchestration and workspace-prep logic itself without persistence yet — see the slice table. |
| `run::results` (new, writing starts in slice 8) | Writes, per `(contestant, repeat index)`, `answer.md`, `diff.patch`, `result.json` (`status` and `usage`, so a timed-out pair is marked `timed_out` [16]), `transcript.jsonl` (or `.txt` if a harness's raw output isn't NDJSON) — written immediately per pair, not batched to the end of the run — and later `evals.json`, all under `.sbxm/runs/<run-id>/<contestant>/<repeat-idx>/`; a copy of the run-config (rubric included) at `.sbxm/runs/<run-id>/run-config.toml`; and one `run.json` for the whole run, **written early** (right after the run ID is generated, before any sandbox exists) with the merged config hash per harness [55], profile name, kit refs, `sbx version()`, `sbx skills()` output [46] and `started_at`, then **updated in place** with `completed_at` once every pair and evaluator has finished — so an interrupted or partially-evaluated run still keeps its identity and config hash on disk, the same "never lose what's already captured" principle as the per-contestant results [16][25]. This extends decision 28's "every sandbox and run records a hash of the fully merged config" to runs. None of this is mounted [112]. Diff: seeded contestants use `git diff` against the baseline commit [14]; unseeded ones use `git diff --no-index` between an empty snapshot and the final workspace, without the workspace ever becoming a git repo [113] — **exit code 1 from `git diff --no-index` means differences were found, not a command failure**; only other exit codes are errors. |
| `eval::rubric` (new) | Shared criterion types [18]: `Criterion { id, kind: PassFail | Scale { levels }, weight, notes }`, used by both the LLM judge and (in a later milestone) human review scoring. |
| `eval::judge` (new) | Runs the rubric through the shared `headless` primitive in a throwaway sandbox (P3), anonymizing contestants as A/B/C [21] and warning when the judge's harness/provider matches a contestant's. Parses the judge's structured JSON response into per-criterion scores. |
| `eval::cosine` (new) | `fastembed`-based similarity between contestants' text answers only [23], no sandbox involved (P4). |
| `eval::jev` (new, added after slice 11) | `reqwest` blocking client to Jev's HTTP API [9], mapping rubric criteria to Noul/Choice/Score questions [20], splitting diffs per file to stay under the 32k-token budget. |
| `cli` (changed) | `Run { config: PathBuf }`, `RunInit { path: Option<PathBuf> }`, `RunShow { run_id: String }` under a `Run`/top-level trio (P1). |
| `commands::run`, `commands::run_init`, `commands::run_show` (new) | One file per new subcommand, following the existing one-file-per-command convention. |

## Interfaces

### Run-config schema

A standalone file [107], not merged with any profile or project config:

```toml
[task]
prompt = "Implement feature X per spec.md"
seed = "./seed-dir"              # optional; omit for an empty workspace [5]

[run]
profile = "default"              # applies to every contestant in this run [115]
timeout = "10m"
budget_usd = 2.00                # best-effort per contestant [98]
cpus = 2                         # optional override of GlobalConfig.resources [108]
memory = "4g"                    # optional override [108]
repeat = 1                       # optional; default 1, in the data model from day one [15]

[[contestants]]
harness = "claude"
model = "claude-opus-5-5"

[[contestants]]
harness = "codex"
model = "gpt-5.6-sol"

[[contestants]]
harness = "antigravity"
model = "gemini-3-pro"

[[eval.checks]]
id = "tests-pass"
command = "cargo test"

[[eval.rubric]]
id = "correctness"
kind = "pass_fail"
weight = 1.0

[[eval.rubric]]
id = "code-quality"
kind = "scale"
levels = ["poor", "fair", "good", "excellent"]
weight = 0.5
notes = "Idiomatic for the language, no dead code"

[eval.judge]
harness = "claude"
model = "claude-opus-5-5"
```

Contestant harness is restricted to `{claude, codex, antigravity}` [103]; `gemini` and `pi` are config
errors naming why (Gemini: deprecated upstream and blocked by egress on this setup, [103]; Pi: deferred,
[93]). Contestant count must be 2–4 [109]. `eval.checks` and `eval.rubric` are both optional lists (a run
can use either, both or neither — an empty `eval` just captures answer/diff/transcript with no scoring).

### Command behavior

| Command | Behavior |
|---|---|
| `sbxm run init [path]` | Writes a starter run-config (default path `./run.toml`) that's valid as written: two contestants uncommented (e.g. Claude and Codex, satisfying the 2–4 minimum, [109]) plus a third, Antigravity, commented out with a note on how to enable it. Refuses to overwrite, like `config init` [43]. |
| `sbxm run <config>` | Validate (incl. `repeat >= 1`, [15]) → check every needed provider secret exists before anything is written or created — each contestant's [98][58] **and the `[eval.judge]` harness's** (it may name a provider no contestant uses) → generate run ID [P2] → **generate and validate the run's kits** (`run::kits`; an invalid kit refuses the run with zero sandboxes created) → **write initial `run.json`** (config hash, profile, kit refs, `sbx` version, skills snapshot [46][28], `started_at`) before any sandbox exists → for each `(contestant, repeat index)` pair in parallel: create throwaway sandbox [95], prepare workspace [102][113], run headless [94], run executable checks [P6], capture diff, **write that pair's results to disk**, remove sandbox → once every pair is done, run the judge/cosine evaluators [P3][P4] and append their scores to each `evals.json` → **update `run.json`** with `completed_at` → print the run ID and a one-line summary per contestant (aggregated across repeats when `repeat > 1`). |
| `sbxm run show <run-id>` | Reads `.sbxm/runs/<run-id>/` and prints a human-readable report: per contestant, the answer, eval scores and a diff summary; the anonymized A/B/C mapping [21] is revealed here, not during evaluation. Nothing is created or called. |

## Work order: vertical slices

Follows `sdlc-implementation`: one user-observable behavior end to end per slice, test-first, smallest
steps, commit after every green step. `FakeBackend` gets the new `exec` method before anything depends
on it.

| # | After this slice… | Test focus | Real `sbx` |
|---|---|---|---|
| 0 | `SandboxBackend` gains `Send + Sync`, `exec(sandbox, &ExecSpec) -> Result<ExecOutput>` and `skills() -> Result<serde_json::Value>`; `SbxBackend` shells to `sbx exec [-w <workdir>] <sandbox> <argv...>` (stdin closed or piped per `ExecSpec::stdin`) and `sbx skills ls --json`; `FakeBackend`'s `RefCell` fields move to `Mutex` so it can be shared across threads, and it records `exec`/`skills` calls with scripted `ExecOutput`/JSON responses. | Backend call args (including workdir and stdin mode) and captured stdout/stderr/exit code; existing tests unaffected; a `FakeBackend` shared across two `std::thread::scope` threads compiles and records both calls. | `sbx exec <existing sandbox> echo hi` matches; `sbx skills ls --json` matches. |
| 1 | The `headless` primitive runs a Claude contestant headlessly: builds the argv, wraps with `timeout --kill-after` [114], calls `exec` via `ExecSpec`, parses `stream-json` into `HeadlessResult`. | Snapshot the argv/`ExecSpec` for a given prompt/model/budget/timeout; parse fixtures of captured `stream-json` output ([S5]) into `HeadlessResult`; a scripted exit-124 `exec` with truncated partial stdout returns `Ok` with `status: TimedOut` and the partial answer/usage. | Real PONG-style prompt via the primitive, not raw `sbx exec`. |
| 2 | Same for Codex: stdin closed before `exec`, `--skip-git-repo-check` when the workspace isn't a git repo [113], NDJSON parsing (`thread.started`/`item.completed`/`turn.completed`, [S5]). | Fixtures of captured Codex NDJSON; argv snapshot including the git-repo flag logic; exit-124 with truncated NDJSON gives `TimedOut` plus the partial answer. | Real prompt via the primitive. |
| 3 | `Harness::Antigravity` [103][104]: pinned kit ref, kit-generation mixin (`requires.agent: antigravity`), headless argv/parsing for `agy -p --output-format stream-json --dangerously-skip-permissions` [S5]. Mandatory-instructions path determined empirically [P5]. | Kit snapshot; NDJSON parsing fixture; argv snapshot; exit-124 with truncated NDJSON gives `TimedOut` plus the partial answer. | Sandbox creates from the pinned tag; PONG via the primitive; confirm (or refute) a mandatory-instructions file location. |
| 4 | Run-config parsing and validation: contestant count 2–4 [109], `repeat >= 1` with a default of 1 [15], harness restriction [103], secrets check per contestant and for `[eval.judge]` [98][58], seed dir validation reusing `src/seed.rs` [102]; a contestant whose harness has no budget flag ([S5]: Codex, Antigravity) gets a loud warning when `budget_usd` is set, same pattern as `Harness::unsupported` [11][98]. Nothing is created yet. | One test per validation rule, including `repeat` and the budget-flag warning; a run whose contestants' secrets exist but the judge's doesn't fails with a one-line actionable error, no filesystem writes and no create/exec calls; valid config parses into `RunConfig`. | none |
| 5 | `sbxm run init` writes a starter run-config that's valid as written (two contestants uncommented, one commented, per P1's fixed scaffold); refuses to overwrite. | The written config passes slice 4's validation unmodified; second run errors. | none |
| 5a | Run kits: `run::kits` generates and validates the `common` and `harness-<h>` mixins per harness from the run's profile before anything is created, stored under `.sbxm/runs/<run-id>/kits/`. Nothing is created in `sbx` yet beyond kit validation. | Effective hash includes `cpus`/`memory` overrides and differs per harness; both kit args passed in order (common, then harness); validation happens before any create; an invalid kit fails with zero creates; kit refs are returned for `run.json`. | `sbx` validates a generated run kit (same check as `new`). |
| 6 | `sbxm run <config>` creates one throwaway sandbox per `(contestant, repeat index)` pair in parallel [95] (using the validated kits from slice 5a and `SandboxBackend: Send + Sync` from slice 0; the loop is written for `repeat` from the start, [15], even though every test here uses the default of 1), runs each headlessly, always removes the sandbox afterward (including on error) — no eval yet, no results files yet, just proof the orchestration and cleanup work. | `FakeBackend` call sequence per pair, including a `repeat = 2` case producing two sandboxes per contestant; a failing pair doesn't block or leave behind the others'; sandboxes always removed. | Three real sandboxes appear and disappear; workspace directories remain. |
| 7 | Workspace seeding: seeded contestants get a fresh git repo + baseline commit [14][102]; unseeded stay a plain directory [113]. Diff capture: `git diff` for seeded, `git diff --no-index` for unseeded (exit 1 means differences found, not failure). | Diff correctness for both cases; the exit-1 case doesn't surface as an error; seed validation errors surface before any sandbox is created. | Diff matches what the agent actually changed. |
| 8 | `run.json` is written right after the run ID is generated (config hash per harness [55], profile, kit refs, `sbx` version, `sbx skills ls --json` [46], `started_at`), before any sandbox exists. Each `(contestant, repeat index)` pair's `answer.md`/`diff.patch`/`result.json`/`transcript.jsonl` is written under `.sbxm/runs/<run-id>/<contestant>/<repeat-idx>/` **as soon as that pair finishes** (not batched to the end of the run), plus `.sbxm/runs/<run-id>/run-config.toml` (rubric included). `run.json` is updated with `completed_at` once everything is done. The mounted workspace stays at `runs/<run-id>/<contestant>/<repeat-idx>/` [111][112]. `sbxm run` prints the run ID. | File contents and locations, including the repeat-index path segment and `result.json`'s `status`; `run.json` exists with identity fields even when a later pair or evaluator fails, and gains `completed_at` only on a full run; run-config copy includes the rubric. | Inspect both trees after a real run; kill one contestant mid-run and confirm `run.json` and the others' results are already written. |
| 9 | `sbxm run show <run-id>` prints answers and diffs (no eval yet). | Output snapshot for a fixture run directory. | none |
| 10 | Executable checks [P6]: run inside the contestant's sandbox before removal, pass/fail by exit code, recorded in `evals.json`. | `FakeBackend` exec sequence includes the check command; pass/fail mapping; a `TimedOut` pair still runs its checks and gets an `evals.json` entry. | A real `cargo test`-style check against a seeded contestant. |
| 11 | Rubric + LLM judge [P3]: a throwaway judge sandbox runs the headless primitive with the rubric/answers(+diffs) as its prompt, contestants anonymized A/B/C [21], a warning when the judge shares a provider with a contestant. Parses structured JSON into per-criterion scores in `evals.json`. `run show` reveals the A/B/C mapping. | Anonymization mapping; provider-sharing warning; judge-response parsing (fixture); a `TimedOut` contestant is still judged (its partial answer, marked as timed out to the run report, not to the judge). | A real 2-contestant run with a real judge call. |
| 12 | Cosine similarity [P4]: `fastembed` similarity between contestants' text answers, added to `evals.json`; skipped (not an error) for non-text/diff-only answers. | Similarity score determinism for fixed inputs. | none (fully local). |
| 13 | **End-to-end check (manual, real `sbx`)**, see below. | none | All items pass. |
| 14 | Jev evaluator [99]: `reqwest` blocking client [P7], rubric criteria mapped to Noul/Choice/Score questions [20], diffs split per file to fit the 32k-token budget. | Mocked HTTP responses (no live Jev calls in CI); composite scoring in code. | One real Jev call against a small rubric. |

**End-to-end check (slice 13)**, on a base dir outside AppData:
- `sbxm run init` → edit the starter config to a real 3-contestant task (Claude, Codex, Antigravity) with
  a seed dir, executable checks and a rubric.
- `sbxm run my-task.toml` (default `repeat = 1`) → all three sandboxes appear and are removed;
  `.sbxm/runs/<run-id>/run.json` has `started_at` and `completed_at`; `.sbxm/runs/<run-id>/<contestant>/0/`
  has all three contestants' `answer.md`/`diff.patch`/`transcript.jsonl`/`evals.json`; the mounted
  workspaces at `runs/<run-id>/*/0/` show the actual edited files.
- Re-run with `repeat = 2` (a run-level setting, so every contestant repeats): two sandboxes per contestant, results under both `0/` and `1/`.
- `sbxm run show <run-id>` prints a readable report with real scores and the revealed A/B/C mapping.
- Kill one contestant's task artificially (a deliberately slow prompt) to confirm the timeout path: exit
  124 inside that sandbox, partial output still captured and evaluated [16], the other contestants
  unaffected.
- Confirm no run-config or rubric content is readable from inside any contestant's sandbox.

## M2b (rough outline — refine once M2a ships)

Folds `scripts/issue-workers.ps1` into sbxm ([88][90]), reusing the `headless` primitive (slices 0–3
above) for a worker's own agent run instead of the script's raw `sbx exec claude -p` calls.

- Run spike S8 (GitHub access from a sandbox, written but not run) once M2a is done — it decides whether
  workers can push/PR/comment themselves through `sbx`'s proxy, or keep M1's host-push model [82].
- Port the dispatcher (host picks issues by label/dependency order, [81]) into an sbxm command, replacing
  the PowerShell script's issue-selection logic.
- Reuse `headless` for the worker's agent run; keep the independent-reviewer step [84][85] as a second
  `headless` call (a different harness/model) rather than a second bespoke code path.
- Streamline what decisions [88] and [90] deferred here: must-fix vs. should-fix handling, the one-fix-round
  policy, and turning every surviving finding into its own GitHub issue automatically (needs S8 or the
  host-push model, depending on what S8 finds).
- Decide `scripts/issue-workers.ps1`'s fate once sbxm covers its job: retire it, or keep it as a thin
  wrapper calling the new sbxm commands.

## Out of scope for M2

- Pi as a contestant harness (S5 deferred it, [93]).
- Gemini CLI's egress gap / deprecation fallout in `--harness gemini` — separate GitHub issue [103], not
  M2 work.
- A local web page for human review [100]; CLI/report stays the interface for both milestones.
- `bollard` backend, `sbx env`, profile-owned skills (carried over from M1's out-of-scope).

## Risks

- **Antigravity is a fast-moving, non-built-in kit.** Mitigation: pin an immutable tag like Pi [104][73];
  if the tag falls behind (Antigravity itself warns Gemini CLI is deprecated in favor of it, so the
  ecosystem is actively shifting), re-pin is a small, isolated change.
- **No dollar-cost reporting for Codex/Antigravity.** Mitigation: token-only usage, no estimation [105] —
  simpler than a price table that goes stale every model release.
- **Judge fairness.** Mitigation: anonymized labels and a provider-sharing warning [21], unchanged from
  the original design.
- **Parallel sandboxes exceeding host resources.** Mitigation: per-contestant sizing from
  `GlobalConfig.resources`, overridable per run [108], not sbx's all-CPUs/16GiB default [S1].
- **Eval rubric leaking into a contestant's own workspace**, letting an agent game its own scoring.
  Mitigation: the workspace/results split mirrors decision 40's project-config isolation exactly [112].
- **The `headless` primitive's abstraction outliving M2a's needs**, per the user's framing in [94]: this
  is treated as a design input (generic naming, no "contestant"-specific coupling in `harness.rs` or
  `headless.rs`), not as license to build speculative features M2a and M2b don't need yet.
