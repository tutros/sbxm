# Decisions

Running log of design decisions for model_compare. See `idea.md` for the original brief.

## Round 1 (2026-09-24)

1. **Sandbox contents:** Each sandbox runs a full agent harness (as Docker Sandboxes does today), with no loss of functionality. Comparisons can vary both harness and model, e.g. two models on the same harness plus a third model on a different harness.
2. **Docker mechanism:** Use Docker Sandboxes (`sbx`) for now. Investigate the `bollard` crate later.
3. **Captured output:** Capture all three: final answer, workspace diff, and full transcript. Not every eval applies to every comparison (e.g. no cosine similarity on code diffs).
4. **Eval criteria:** Both a free-text rubric and executable checks. JEV support will likely be added later.
5. **Starting state:** Each contestant gets either an empty directory or a fresh copy of an optional seed directory, under `<base>/<project>/runs/<run-id>/<contestant>/`.
6. **Execution:** Comparisons run headless and in parallel, with a timeout and a cost/token cap per contestant. Standalone sandboxes are interactive.
7. **Providers/auth:** Anthropic, OpenAI and Google at first; OpenRouter and Pi later. Prefer OIDC auth and fall back to API keys when OIDC won't work. Keys are defined with `sbx secrets`.
8. **v1 scope:** Milestone 1 is sandbox lifecycle (create/reuse a project sandbox from the common config). Model comparison is milestone 2.

## Round 2 (2026-09-24)

9. **JEV** = Jev, TypeSafe's System One model (docs: https://docs.typesafe.ai/llms.txt). **Pi** = the Pi AI coding harness (not a provider).
10. **Contestant** = `(harness, model, optional config variant)`.
11. **Config across harnesses:** Option (c) for now: shared instructions file, shared egress/secrets, hooks/guardrails per harness. Long-term goal is (a): one canonical config with per-harness adapters. Warn loudly when a harness can't support a configured feature.
12. **Caps/auth:** Caps are best effort (harness flags where available); the timeout is the only guaranteed limit. Standalone sandboxes can use existing subscriptions. Comparisons may later require API keys plus budgets (undecided).
13. **Headless `sbx`:** Unknown. Prompts can be passed to a sandbox, but headless runs are untested. Run a spike at the start of milestone 2, and check the `sbx` CLI surface during milestone 1.
14. **Diff capture:** Always snapshot fresh: copy files, new git repo, one baseline commit. No seed history or remotes. Security and isolation are core goals.
15. **Repetitions:** `--repeat N` is in the data model from day one; default 1.
16. **Timeout/crash:** Keep partial output and mark it `timed_out`. Checks and judges still evaluate it.
17. **Backend:** Shell out to `sbx` behind a `SandboxBackend` trait; `bollard` can become a second backend later.

## Round 3 (2026-09-24)

18. **Rubric format:** Structured. Each criterion has `id`, `kind` (`pass_fail` → Jev Noul, `scale` with described levels → Jev Score), `weight` and optional free-text `notes` for the LLM judge and humans. All evaluators score against the same criteria.
19. **Scoring mode:** Score each contestant separately by default and rank in code. Direct comparison is optional (the LLM judge's winner call), with randomized order and blind labels.
20. **Evaluator inputs:** Jev sees the final answer plus the diff (split per file if over the 32k budget), never the transcript. The LLM judge sees the answer plus the diff, and optionally a transcript summary. Humans see everything. **Also allow Jev-specific criteria** (raw Noul/Choice/Score questions defined alongside the shared rubric). Expect to revisit this as eval-writing practice matures.
21. **Judge fairness:** Judge model is set per comparison, with a warning when it shares a provider with a contestant. Contestant labels are anonymized (A/B/C) for the judge and humans; the mapping is revealed only in the final report.
22. **Human review:** CLI first (or just readable reports); a local web page later.
23. **Embeddings:** Local (`fastembed`): no secrets, no cost, deterministic. Text answers only.
24. **Results storage:** Files are the source of truth (`runs/<run-id>/<contestant>/{answer.md, diff.patch, transcript.jsonl, evals.json}`). SQLite can be added later as a rebuildable index.
25. **Harness adapters:** We define an adapter per harness (headless command, transcript parsing, native config file names), whether or not `sbx` supports the harness natively. Pi runs through the official Pi kit for `sbx`.

## Round 4 (2026-09-24)

26. **sbx kits:** Unknown whether a kit can carry instructions, hooks, egress rules and secret references. Part of the milestone 1 spike. If it can, compile the common config into a kit instead of applying it at launch.
27. **Config format:** TOML with three layers: global (`~/.config/sbxm/config.toml`) → named profile (`default`, plus others such as `strict`) → project (`<base>/<project>/sandbox.toml`). Comparison config variants reference profiles.
28. **Config versioning:** The common config lives in its own directory, ideally a git repo. Every sandbox and run records a hash of the fully merged config.
29. **Egress:** Deny by default. The harness adapter automatically adds its provider's domains. Check `sbx` network policy support in the spike.
30. **Secrets:** Config names secrets and never contains values. Verify each named secret exists before launch. Secret values are never written to results. Scan transcripts and diffs for secret values if `sbx` exposes them; otherwise rely on `sbx` redaction.
31. **Existing project:** Attach if the sandbox is running, recreate it if gone. If the merged config hash changed, warn and require `--rebuild`.
32. **M1 commands:** `new`/`open <project>`, `list`, `stop`, `rm` (deletes the directory only with `--purge` plus confirmation), `config show`, `doctor`.
33. **Project names:** `[a-z0-9-]` only. Reject `..`, path separators and reserved Windows names.
34. **Name:** CLI, binary and crate are `sbxm`.
35. **Testing:** Tests run against a fake `SandboxBackend` by default; real-`sbx` tests sit behind a feature flag or `--ignored`.

## Round 5: after spikes S1–S4 (2026-09-24)

36. **sbxm's role:** `sbx` enforces egress, secrets and isolation. sbxm compiles its TOML config into v2 kits, calls `sbx create --kit …`, and keeps host-side state (projects, runs, config hashes) plus evals. sbxm does no enforcement of its own.
37. **Mandatory vs. reference instructions:** Each per-harness mixin (`requires.agent`) writes mandatory rules to that harness's always-loaded user-level instructions file (e.g. `~/.claude/CLAUDE.md` via `files/home/`). The kit `agentInstructions` field is only for optional reference material, because `sbx` surfaces it on demand. Paths for Codex, Gemini CLI and Pi are unverified.
38. **`sbx env`:** Not used for now; sbxm calls `sbx create` directly. Generating `sbxenv.yaml` may come later.
39. **Kit format risk:** Accepted. Generate v2 kits from a single module, and `doctor` enforces a minimum `sbx` version (currently 0.43.0).

## Round 6: milestone 1 plan (2026-09-24)

40. **Metadata location (P1, changes paths in 5, 24, 27):** The workspace is `<base>/<project>`. All sbxm metadata lives in `<base>/.sbxm/<project>/` (`sandbox.toml`, `state.json`, generated kits, and in M2 `runs/`), which is never mounted into a sandbox, so agents can't read or edit their own config.
41. **Sandbox names (P2):** `sbxm-<project>-<harness>`. One project can have one sandbox per harness.
42. **Repo directory (P3):** Rename `model_compare/` to `sbxm/` (done by the user outside the session, since VS Code holds the folder open). Git repo initialized on `main` (2026-09-24); no commits yet. The scaffold in milestone 1 step 1 will be the first commit, together with the planning docs.

## Implementation (2026-09-24)

43. **Starter config (`config init`, slice 1):** Writes `<config_dir>/config.toml` and `<config_dir>/profiles/default/profile.toml`, where `<config_dir>` is `SBXM_CONFIG_DIR` or `~/.config/sbxm`. `profiles_dir` defaults to `<config_dir>/profiles` (so the config dir is the directory to version), and `base_dir` defaults to `~/sbxm-projects` (under home, not AppData). The starter profile references no files. If either file exists, nothing is written.
44. **Project name length (slice 2):** At most 40 characters, so `sbxm-<project>-<harness>` stays well under the 63-character hostname limit (longest harness today: `gemini`, 52 characters total).
45. **Testability and state shape (slice 3):** The crate is lib + bin. Command functions take the config dir and a `&dyn SandboxBackend`, so tests run them in-process with `FakeBackend` (no env vars, no test switches in the binary). `state.json` is `{"sandboxes": {"<harness>": {sandbox, workspace, created_at}}}` (unix seconds), keyed by harness per decision 41; profile, hash, `sbx` version and kit paths are added by the slices that produce them. Real-`sbx` tests are `#[ignore]` in `tests/real_sbx.rs` and read `SBXM_REAL_BASE_DIR`.
46. **Skills (2026-09-24):** `sbx` keeps its own skills store (filled explicitly with `sbx skills import`/`add`), mounted at each agent's skills dir with `sbx create --skills readonly|readwrite|off` (default `readonly`). It does not expose the host's `~/.claude/skills` directly. A profile setting `skills.store = "readonly" | "off"` (default `readonly`) maps to `--skills`. `readwrite` is rejected, because an agent could plant skills that every other sandbox loads. The store serves Claude, Codex, Copilot, Cursor, Droid and OpenCode, not Gemini CLI or Pi. **Later:** profile-owned skills (`skills.dir`) copied into each harness mixin's `files/home/<skills dir>`, versioned and included in the config hash, with `--skills off` for that sandbox; pending spike S6. **M2:** record `sbx skills ls --json` with each run so store drift is visible.
47. **Seeding `new` (slice 4):** `--seed` only creates a new project (an existing workspace is an error). The seed is checked before anything is written: it must be a directory, must not contain the base dir (the copy would recurse), and must not contain symlinks or junctions (following one could copy files from outside the seed into the sandbox). Contents are copied as-is, including any `.git`; the fresh-repo-with-baseline-commit rule applies to M2 comparison seeds.
48. **Pi and `kit.allowedSources` (after S6):** `sbx` only installs kits from `kit.allowedSources` (default `["docker.io/"]`); the Pi kit is on `github.com/docker/`. sbxm never changes `sbx` settings. `doctor` reports it when a profile or command needs Pi, and `new`/`open --harness pi` fail before creating anything. Like other sbxm errors, the message says what's wrong and how to fix it, built from the current setting, e.g.: `the Pi kit's source github.com/docker/ is not in sbx's kit.allowedSources; allow it with sbx settings set kit.allowedSources '["docker.io/","github.com/docker/"]' (this trusts every kit from github.com/docker/)`.
49. **Harness home files (after S6):** Verified: Claude loads `~/.claude/CLAUDE.md` (canary answered by `claude -p`, also after restart); Codex puts `~/.codex/AGENTS.md` in its prompt (`codex debug prompt-input`, no model call); Gemini CLI uses `~/.gemini/GEMINI.md` (shipped docs and code; loading unverified, no credentials). Skills dirs: Claude `~/.claude/skills/`; Codex `~/.codex/skills/` and `~/.agents/skills/`; Gemini `~/.gemini/skills/` and `~/.agents/skills/` (`.agents` wins on a name clash). A mixin's `files/home/.claude/CLAUDE.md` and skills survive create and restart, but the Claude kit **replaces** a mixin's `~/.claude/settings.json` (no merge), so hooks can't ship as that file; the route is open (S6b). With an empty skills store, `--skills readonly` mounts nothing. Pi paths are unverified (blocked by [48]).

## Open research spikes
- **S1–S4:** Done 2026-09-24; see "Spike results" below.
- **S6:** Done 2026-09-24 (partial); spec `spikes/S6.md`, evidence `spikes/S6-results.md`, conclusions in decisions 48 and 49.
- **S6b (before slice 14):** Does a non-empty skills store get mounted over a harness skills dir and hide mixin skills (needs one canary skill added to the store, then removed)? What's a working route for Claude hooks, given the kit replaces `settings.json` (`setup.install` merge, `settings.local.json`, managed settings)? Do the Codex and Gemini kits also replace `~/.codex/config.toml` / `~/.gemini/settings.json` from a mixin? Pi paths and loading, once `kit.allowedSources` allows it [48]. Spec to be written and approved before running (sdlc-implementation rule 5).
- **S5 (M2):** Headless runs per harness inside `sbx` (Claude Code, Codex, Gemini CLI, Pi): prompt in, run to completion, extract answer + transcript + token usage (decision 13). Leads: the v3 kit spec has `agent-sessions@1` (`prompt: ["-p", "{{.Prompt}}"]` for claude), but `sbx` 0.43 has no CLI verb for it. Likely route is `sbx exec <sandbox> <harness-cli> <headless flags>`.

## Spike results (2026-09-24, sbx v0.43.0, Windows)

**S1: CLI surface**
- Lifecycle: `create`, `run` (creates if missing, then attaches), `exec` (**starts a stopped sandbox**), `stop`, `rm -f` (non-interactive; also deletes sandbox-scoped secrets and kit policies), `ls --json` (name, id, agent, status, last_used_at, workspaces).
- `ls --json` does **not** show kits or env, so sbxm must keep its own host-side state (sandbox → project, profile, config hash).
- Sandbox names: letters, digits, `-`, `.`; `default` is reserved. Our `[a-z0-9-]` rule is a subset, and we must also reject `default`.
- Defaults are all host CPUs and 16 GiB memory. Comparisons must pass `--cpus`/`-m` explicitly.
- Built-in agents: claude, codex, copilot, cursor, devin, docker-agent, droid, gemini, kiro, opencode, shell. **Pi is a kit**: `docker/sbx-kits-contrib/pi` (image `docker.io/sbx/pi-image`, Anthropic only: API key or OAuth).
- Workspace is mounted at a path mirroring the host (`E:\x\ws` → `/e/x/ws`).
- **Windows gotcha:** a workspace under `%TEMP%` (AppData) fails with `ERROR: failed to run sandbox container`; the same create on `E:\` works. `doctor` should check the base dir location. Disk: each harness image is several hundred MB (14 GiB free at test time).
- Experimental commands: `sbx env` (declarative `sbxenv.yaml` with plan/approve, kits, secrets, host lifecycle commands, `-y` to auto-approve) and `sbx kit`.

**S2: Kits (decision 26: yes, kits can carry the common config)**
- `sbx` 0.43 reads local directory kits in **schemaVersion "2"** format. v3 (`capabilities:`) needs a BuildKit build step and is rejected as a plain directory. Spec: `docker/sbx-kits-contrib/spec/SPEC-v2.md`.
- A v2 `kind: mixin` can carry `permissions.network`, `environment.variables`, `credentials`, `setup.install`/`setup.startup` commands, `agentInstructions`, `volumes`, `ports`, and a `files/home/` + `files/workspace/` tree. Verified live: env vars, install hook and network allow all applied.
- `requires.agent: <harness>` pins a mixin to one harness, and composing it on another is an error. This is how per-harness hooks mixins work (e.g. the ggshield kit wires a Claude Code hook).
- Composition: `sbx create --kit A --kit B <agent> <path>`, merged in flag order; remote kit refs must be pinned by commit SHA or digest.
- **Caveat:** mixin `agentInstructions` are surfaced *progressively*. The harness profile (e.g. CLAUDE.md, written as a container-only sibling of the workspace) gets an index telling the agent to read kit files "only when a kit becomes relevant… do not read them all upfront." That's not strong enough for mandatory rules.

**S3: Egress (decision 29: native)**
- Global `default-deny-all` policy. Blocked requests get HTTP 403. A mixin's `permissions.network.allow` becomes a policy scoped to that sandbox (`source: kit`). Verified: allowed host 200, other hosts 403, same host without the mixin 403.
- Built-in harness kits already allow their own provider domains, so sbxm doesn't need to add them.
- `sbx policy log [--json]` records every allowed and blocked request per sandbox, which gives an egress audit trail per contestant.

**S5 (Claude only, partial): headless via `sbx exec` works**
- Tested: `sbx exec -w <workspace-in-container> <sandbox> claude -p "<prompt>" --model <id> --max-turns N --output-format stream-json --verbose`. Exit 0 in about 6 s; the file written by the agent appeared on the host workspace.
- `exec` sessions *do* get the proxy environment (`HTTPS_PROXY`), and the claude kit already sets `apiKeyHelper` (proxy-managed key) and `bypassPermissions`. Init event reports `apiKeySource=apiKeyHelper`, `permissionMode=bypassPermissions`.
- The stream-json output has everything we capture: `system/init` (model, cwd, session_id), `assistant` events with tool calls (the transcript), and a final `result` event with `result` (final answer), `num_turns`, `duration_ms`, `total_cost_usd`, `usage` and per-model `modelUsage`.
- Cap flags: `--max-turns` (works, not listed in `--help`) and `--max-budget-usd` (real dollar cap).
- **Killing `sbx exec` on the host does NOT stop the process inside the sandbox.** Timeouts must be enforced inside the sandbox: `sbx exec <sandbox> timeout --kill-after=<grace> <limit> <harness cmd>`. Verified: exit code 124, process gone. Fall back to `sbx stop` if needed.
- Still to test: Codex, Gemini CLI, Pi headless modes and their output formats; subscription (OAuth) auth in exec context; parallel sandboxes.

**S4: Secrets (decision 30: mostly handled by sbx)**
- Values never enter the sandbox. Service secrets (anthropic, openai, google, openrouter, github, …) and custom secrets use placeholders that the proxy swaps on requests to allowed hosts. Verified: a filesystem-wide search inside the sandbox found no secret value.
- There is no command to read values back. `sbx secret ls --json` returns scope, service/env and a **masked** value, which is enough for an existence check. Secret scanning of outputs isn't needed; only placeholders can leak.
- Secrets apply at sandbox creation (existing sandboxes don't pick up new ones). Sources can be `--ref op://…` (1Password), AWS Secrets Manager or `--command`. OpenAI OAuth is supported: `sbx secret set openai --oauth`.

### Jev facts relevant to design (from docs, Jev 1.13)
- HTTP API only (`POST https://api.typesafe.ai/v1/systemone`, Bearer auth). SDKs exist only for Python and JS, so Rust calls it with `reqwest`.
- Primitives: **Noul** (P(yes)), **Choice** (one of ≤255 options + distribution), **Score** (2–10 ordered levels, probability-weighted). Returns typed answers + probabilities/confidence, not text.
- Limits: 64k tokens per request; **32k for `state` + longest question**. Text only. $0.042 per 1M input tokens; output tokens are free.
- Fits evals via composite scoring: one question per rubric dimension, with weights applied in code.
