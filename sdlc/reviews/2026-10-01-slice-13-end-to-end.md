# Slice 13: end-to-end check of M2a (2026-10-01)

Run from a build of `m2a-implementation` at `64053ed` (decision 138 merged), `sbx` 0.46.0, base dir `E:\sbxm-projects`,
secrets `anthropic`, `openai` and `google` stored. Cosine and Jev are deferred [138], so they weren't exercised.
The task, seed and configs were throwaway files outside the repo (`E:\sbxm-projects\e2e-task\`): a Python
`top_words` stub with six unit tests, three contestants (Claude `claude-opus-5-5`, Codex `gpt-5.6-sol`,
Antigravity `gemini-3.1-pro-high`), one executable check (`python3 -m unittest`), a two-criterion rubric and a
Claude judge.

| Checklist item | Result | Evidence |
|---|---|---|
| `sbxm run` creates three sandboxes and removes them | Pass | Run `2026-10-01-b97a8b` exited 0; `sbx ls` afterwards showed none of the run's sandboxes (judge sandbox removed too) |
| `run.json` has `started_at` and `completed_at` | Pass | `2026-10-01T15:25:26Z` and `2026-10-01T15:26:45Z`; per-harness `config_hash`es and `sbx_version` present |
| Per-contestant result files | Pass | `<contestant>/0/` has `answer.md`, `diff.patch`, `evals.json`, `result.json`, `transcript.jsonl`; `judge/0/` has `judge.json`, `reply.txt`, `transcript.jsonl` |
| Mounted workspaces show the edited files | Pass | `runs/<id>/{0,1,2}/0/wordcount.py` edited |
| `repeat = 2` | Pass | Run `2026-10-01-e4498f`: pairs `0/0 0/1 1/0 1/1 2/0 2/1` plus `judge/0`, `judge/1`; each contestant `2/2 repeats scored; checks 2/2 passed` |
| `sbxm run show <run-id>` | Pass | Prints status, answer, diff summary, checks and judge scores per contestant; ranking claude 1.00, codex 1.00, antigravity 0.89 (run 1) |
| Timeout path | Pass | Run `2026-10-01-568d63` (`timeout = "20s"`): all three `status: timed_out`, partial output kept (Claude transcript 67 KB, Codex partial answer), the check still ran on the partial workspace and failed (6 errors) [16] |
| No run-config or rubric content readable from a contestant sandbox | Pass | Run `2026-10-01-cb1740`: canaries in the run-config comment and the rubric notes. Each contestant listed `/e` (only `sbxm-projects`, containing only `runs`), found no `run*.toml` or `run.json`, and the only files containing the canaries were its own session transcript (the probe prompt names them) |

## Observations (none block the merge)

- The timeout run timed out all three contestants because the task is shared, so "the other contestants unaffected"
  rests on run 1, where contestants finish independently.
- Agent runs leave `__pycache__/*.pyc` in `diff.patch` (binary adds). Noise, not a defect.
- One judge reply said it could not rerun the tests and relied on the contestant's report; the judge sees the answer
  and diff, not the sandbox, by design.
- Decision 137 held: Antigravity ran in sandboxes made by `sbxm run` with only the `google` secret.
- Not run here: the `#[ignore]` real-`sbx` test suite (`tests/real_sbx.rs`) and issue #32 (real-`sbx` coverage for
  `exec` and `skills`), which stays open as should-fix.
