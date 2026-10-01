# Unattended batch: M2a slices 6-12b (2026-09-30)

Written at the user's request ("run whichever slices you can unattended; define success criteria; don't run in an
endless loop"). This is the in-session batch form of `sdlc-implementation` rules 5 and 8, made explicit. The log at the
bottom is updated after every slice.

## Why

Slice 5a is done, so the chain 6 -> 7 -> 8 -> 9 -> 10 -> 11 is unblocked, and 12a/12b follow. The user is away, so every
choice below is pre-decided; if a choice can't be, the slice stops and is recorded as Blocked with the question.

## Scope

Attempted, in this order, one at a time: **6, 7, 8, 9, 10, 11, 12a, 12b**.

Not attempted, and why:
- **12 (cosine):** needs the new dependency `fastembed` (ONNX runtime plus a model download); a new dependency is a stop
  condition. Its result (`evals.json` field) is not needed by 12a.
- **13 (end-to-end check):** manual, real `sbx`, the user's.
- **14 (Jev):** blocked on spike S9.

A slice depends on the previous ones: if one stops, the batch stops there. Nothing is skipped past a stopped slice.

## Environment facts (verified this session unless marked)

- Branch `m2a-implementation` (pushed); slices 0-5a done. Windows, PowerShell/Git Bash. `sbx` 0.43.0 logged in; secrets
  stored: `anthropic`, `openai`. **No `google` secret, and Antigravity can't authenticate in a fresh sandbox** (decision
  124f, open), so every real check uses Claude and Codex only; Antigravity is covered by fake-backend tests.
- Real-`sbx` tests use `SBXM_REAL_BASE_DIR=E:\sbxm-it` (never `C:`; decision 56). Models: `claude-haiku-4-5-20251001`,
  `gpt-5.6-luna` with tiny prompts.
- The Bash tool collapses double backslashes (a hook blocks them); write such files with Write/Edit.
- Permission denials by the auto-mode classifier are not to be worked around (see stop rules).

## Success criteria

**Per slice (all must hold before its commit):**
1. The slice's row in `sdlc/milestone-2.md` ("Test focus" column) is covered by tests that were seen failing first for the
   expected reason, and now pass.
2. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --no-fail-fast` all pass
   (evidence: the counts, "N ok, 0 failed").
3. If the slice touches `sbx`: its `#[ignore]` real test passes once (one fix and one re-run allowed), leaves no
   sandbox (`sbx ls --json` lists only pre-existing ones) and no folder under `E:\sbxm-it`.
4. Docs in step: `AGENTS.md` layout, `README.md` if user-visible, `sdlc/decisions.md` for details the plan left open.
5. Committed (imperative subject, `Co-Authored-By` line) and pushed to `origin/m2a-implementation`. No merge, no PR,
   nothing on `main`.
6. The log below has the slice's commit, test counts and the **manual test steps for the user**.

**Batch done when:** all eight slices meet the per-slice criteria, or a stop rule fires (then the log says which slice,
which rule, the evidence and the exact question for the user). Either way the batch ends; nothing retries by itself.

## Pre-decided judgment calls

- Details the plan left open (like decision 125) are recorded as new numbered decisions in `sdlc/decisions.md` and listed in
  the log. Anything that contradicts an existing decision, changes security posture, adds cost, or is a preference only
  the user can give is a **stop**, not a decision.
- Real checks use 2 sandboxes (Claude, Codex) where the plan says 3, because Antigravity can't authenticate (124f).
- Run IDs are `<date>-<6 hex>` per P2; results live under `<base>/.sbxm/runs/<run-id>/` and the mounted workspace
  under `<base>/runs/<run-id>/<contestant>/<repeat>/` per the plan.

## Budget

- Per slice: at most 3 red->green cycles per test group; a real test at most twice (one re-run after a fix).
- Real model calls: at most 12 per slice, tiny prompts, cheap models. At most 4 sandboxes alive at once.
- Whole batch: at most the 8 slices above. No open-ended polling: background jobs are waited on once.

## Stop rules

Stop the whole batch, record it in the log, and leave the tree clean and committed (or stashed nowhere: uncommitted
work is either committed on green or reverted to the last commit), when:
- the same error happens twice after a fix;
- a slice needs a new dependency, a new decision that contradicts an existing one, or a departure from its plan row;
- a real-`sbx` check fails after its one re-run, or anything would touch a path outside temp dirs and `E:\sbxm-it`;
- a slice grows beyond its plan row (more than about one screen of new production code per red->green step is split;
  needing a new module the plan doesn't name is a stop);
- a tool call is denied by the permission classifier (never worked around);
- a check only the user can do is next (interactive attach, browser sign-in).

Forbidden: `git push --force`, pushing to `main`, opening PRs, `sbx rm` of a sandbox not created by this batch,
changing `sbx` settings or secrets, `gh` writes, editing `.claude/settings*.json`.

## Side effects

Writes only: this repo's working tree on `m2a-implementation`, `E:\sbxm-it` (temp dirs, deleted after), the system temp
dir. Sandboxes created by this batch are named `sbxm-it-<pid>-*` or `sbxm-run-*` and are removed by the tests.

## Cleanup (always, including after a failure)

`sbx ls --json` shows only `sbxm-sbxm-m2-claude`; `E:\sbxm-it` is empty; `git status` is clean.

## Log

| Slice | Status | Commit | Tests (ok/failed binaries) | Real check | Notes |
|---|---|---|---|---|---|
| 5a | Done (before the batch) | 7b0be8b | 34 / 0 | `run_kits_validate_against_real_sbx` passed | |
| 6 | Done | see `git log` (3 commits, "Script the fake backend", "Add run IDs and the per-pair sandbox orchestrator", "Add 'sbxm run <config>'") | 37 / 0 | `run_against_real_sbx` passed: Claude + Codex in 2 parallel sandboxes, both PONG, both removed, scratch folder gone | Decision 126. Fixed a slice-2 gap (crash with no output reported "no result event"). |

| 7 | Done | see `git log` ("Add hardened seeding and diff capture", "Wire seeding and diffs into the orchestrator") | see commit | `run_diffs_against_real_sbx` passed: Claude + Codex, seeded and unseeded, diffs match their edits, one commit and no remote in seeded workspaces, no sandbox or scratch left | Decision 127: git runs against a host-owned git dir because a planted `.git/config` runs commands on the host (control-tested). |

| 8 | Done | see `git log` ("Save each pair's results and run.json as they happen") | see commit | `run_against_real_sbx` (extended) passed: run.json has completed_at and both harness hashes, each pair has result/answer/transcript/diff | Decision 128. "Kill one contestant mid-run" covered by a gated fake test, not a real kill. |

| 9 | Done | see `git log` ("Add 'sbxm run show'") | see commit | none (plan: none) | Decision 129 (added a `--diff` flag). |

| 10 | Done | see `git log` ("Run executable checks in each pair's sandbox") | see commit | `run_checks_against_real_sbx` passed: Claude + Codex seeded; two checks passed, one failed with its stderr captured, one timed out at 3 s; diffs unaffected; nothing left behind | Decision 130. The plan's "cargo test-style" check is shell-based because the sandboxes have no Rust toolchain. |

| 11 | Done | see `git log` ("Add the LLM judge") | see commit | `run_judge_against_real_sbx` passed: Claude + Codex judged by a real Claude judge in its own sandbox; A/B mapping stored, all criteria scored, provider-sharing warning shown, no sandbox left | Decision 131. The prompt travels as a file (Windows command-line limit). The judge keeps its harness's tools (not enforceable to remove). |

| 12a | Done | see `git log` ("Rank contestants from the judge's scores") | see commit | none (plan: none) | Decision 132. |

| 12b | Done | see `git log` ("Build kits per profile for per-contestant profiles") | see commit | `run_profiles_against_real_sbx` passed: two Claude contestants with different `env.WHO` profiles each read their own value; run.json/result.json record the profiles; nothing left behind | Decision 133. Removed the obsolete test that asserted the "refused until 12b" limitation (its coverage is now `tests/run_profiles.rs`). |

### Manual test steps

**Slice 12b** (per-contestant profiles):
1. Create a second profile, e.g. `<config_dir>\profiles\strict\profile.toml` with `[env]` `WHO = "strict"`, and give the `default` profile `WHO = "default"`.
2. In a run-config, give the second contestant `profile = "strict"`, and use the prompt "Run `printenv WHO` in the shell and reply with exactly its output."
3. `cargo run -- run <config>`: contestant 0 answers `default`, contestant 1 answers `strict`. `cargo run -- run show <run-id>` marks `contestants[1] ... (profile strict)`.
4. `Get-Content <Results folder>\run.json` lists both kit sets (`harnesses[]` with `profile` and different `config_hash`).
5. An unknown profile name is refused before anything is written.
6. Automated: `cargo test --test run_profiles` and `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_profiles_against_real_sbx -- --ignored --nocapture`.

## Batch result (2026-09-30)

All eight slices in scope (6, 7, 8, 9, 10, 11, 12a, 12b) met the per-slice criteria; no stop rule fired. Not attempted, as planned: slice 12 (cosine; needs the new `fastembed` dependency), 13 (your manual end-to-end check) and 14 (Jev; blocked on spike S9).

Open for you (from decisions 124 and 125): how run sandboxes authenticate Antigravity (124f) and, until then, the provisional `google` secret requirement for an Antigravity contestant.

Departures worth a look: decision 127 (host `git` runs against a host-owned git dir because a contestant can plant a `.git/config` that runs commands on the host; the plan's literal commands run the same way but against that dir), decision 131's limit (the judge keeps its harness's tools), and the first commit of slice 7 not compiling on its own (the tip of each slice is green).

**Slice 12a** (ranking): needs a run with a judge (slice 11 steps).
1. After `cargo run -- run <config>` the block `Ranking (judge scores 0-1, repeats averaged)` is printed before `Results:`, best contestant first, e.g. `1. contestants[1] codex/gpt-5.6-luna: 1.00 (1/1 repeats scored)`.
2. `cargo run -- run show <run-id>` prints the same block under its header.
3. Re-rank without re-running: open `<Results folder>\run-config.toml`, change a rubric `weight`, then run `cargo run -- run show <run-id>` again: the scores change, nothing else is touched.
4. Automated: `cargo test --test eval_score`.

**Slice 11** (LLM judge): needs the `anthropic` and `openai` secrets.
1. Add to a run-config (after the contestants): two `[[eval.rubric]]` entries (one `kind = "pass_fail"`, one `kind = "scale"` with `levels = ["poor", "fair", "good"]`) and `[eval.judge]` with `harness = "claude"` and a model. Use a small prompt such as "In one sentence, explain what a mutex is."
2. `cargo run -- run <config>`: after the contestant lines there is `Judge claude/<model>: repeat 1/1 scored 2 contestants`, and a `warning:` that the judge shares a provider with a Claude contestant.
3. `cargo run -- run show <run-id>` prints under each contestant `Judge (candidate A): ...` with scores and reasons: that is the anonymous label the contestant was judged under.
4. Files: `<Results folder>\judge\0\judge.json` (label to contestant), `reply.txt` (the judge's reply), `0\0\evals.json` (the `judge` key). While it runs, `sbx ls` shows one `sbxm-run-<id>-judge-0` sandbox after the contestants' are gone.
5. Automated: `cargo test --test eval_judge --test eval_judge_run --test run_judge_command` and `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_judge_against_real_sbx -- --ignored --nocapture` (about 45 s).

**Slice 10** (executable checks):
1. Add to a run-config (after the contestants): `[[eval.checks]]` with `id = "has-hello"` and `command = "test -f hello.txt"`, and one with `command = "sleep 120"` and `timeout = "3s"`. Use a prompt like "Create a file hello.txt containing the word hi".
2. `cargo run -- run <config>`: each contestant's line ends with `; checks 1/2 passed`.
3. `cargo run -- run show <run-id>` lists `passed has-hello` and `failed <id> (timed out after 3s)`; `Get-Content <Results folder>\0\0\evals.json` has the details and the output tail.
4. Automated: `cargo test --test run_checks` and `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_checks_against_real_sbx -- --ignored --nocapture` (about 45 s).

**Slice 9** (`sbxm run show`): needs a finished run (slice 6/8 steps) or use the demo below.
1. Take the run id from a run (`Run <id>` on the first line), then `cargo run -- run show <id>`; add `--diff` for the full patches.
2. Expect the header, then each contestant with `repeat 1/1: completed (...)`, `Answer:` and `Diff:` blocks.
3. Errors: `cargo run -- run show ../x` says it isn't a run id; `cargo run -- run show 2026-01-01-aaaaaa` says no such run.
4. Automated: `cargo test --test run_show`.

**Slice 8** (results on disk): after any `sbxm run <config>` (see slice 6's steps), the last line printed is `Results: <folder>`.
1. `dir <folder>`: `run.json`, `run-config.toml`, `kits\`, and one folder per contestant index with a folder per repeat inside.
2. `Get-Content <folder>\run.json` shows `started_at`, `completed_at`, `sbx_version`, `harnesses[].config_hash`. `Get-Content <folder>\0\0\result.json` shows `status`, `usage`.
3. `Get-Content <folder>\0\0\answer.md` is the contestant's final answer; `diff.patch` its changes; `transcript.jsonl` the raw agent output.
4. Interrupt a run (Ctrl+C while contestants are running): `run.json` exists with `"completed_at": null`, and any pair that had finished already has its files. (A Ctrl+C can leave `sbxm-run-<id>-*` sandboxes behind: `sbx ls`, then `sbx rm -f <name>`.)
5. Automated: `cargo test --test run_results` and `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_against_real_sbx -- --ignored`.

**Slice 7** (seeding and diffs; results are not saved to disk until slice 8, so use the automated real test):
1. `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_diffs_against_real_sbx -- --ignored --nocapture` (about 70 s; needs the `anthropic` and `openai` secrets). It prints two runs; both must show `completed` for both contestants.
2. Offline: `cargo test --test run_workspace` (seeding, diffs, and the hostile-`.git/config` test) and `cargo test --test run_seeded`.
3. By hand, the seeding part: `cargo run -- run E:\sbxm-it\demo\run.toml` with `seed = "<a folder with a git repo>"` in the file, then look at `<base_dir>\runs\<id>\0\0`: `git log` shows one commit `baseline`, `git remote` is empty.

**Slice 6** (`sbxm run <config>`): needs the `anthropic` and `openai` secrets (`sbx secret ls`).
1. `cargo run -- run init E:\sbxm-it\demo\run.toml`, then edit the prompt to something small and put cheap models in (`claude-haiku-4-5-20251001`, `gpt-5.6-luna`) and `timeout = "3m"`.
2. `cargo run -- run E:\sbxm-it\demo\run.toml` prints `Run <id>` and one line per contestant (completed, timed out or failed).
3. While it runs, `sbx ls` in a second terminal shows two `sbxm-run-<id>-<n>-0` sandboxes; afterwards they are gone.
4. Workspaces stay: `dir <base_dir>\runs\<id>\`. Kits: `dir <base_dir>\.sbxm\runs\<id>\kits\`.
5. Clean up: `Remove-Item -Recurse <base_dir>\runs, <base_dir>\.sbxm\runs` (only those two), and `E:\sbxm-it\demo`.
6. Automated version: `$env:SBXM_REAL_BASE_DIR='E:\sbxm-it'; cargo test --test real_sbx run_against_real_sbx -- --ignored --nocapture`.
