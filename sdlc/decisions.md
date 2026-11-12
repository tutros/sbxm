# Decisions

Running log of design decisions for model_compare. See `sdlc/idea.md` for the original brief.

Each numbered decision is a file of its own in `sdlc/decisions/` (decision 303), named `NNNN-<slug>.md`: the number
padded to 4 digits, so the folder sorts by number, then a short lowercase slug of its title (`[a-z0-9-]`, at most 50
characters). The file holds the entry; its first line is the title line (`303. **Title (date, ...):** text`). The
folder is the index: nothing lists the decisions again. This file keeps the rounds (each names its decisions), the open
research spikes and the spike results.

A new decision is a new file in `sdlc/decisions/`, with the next free number: one above the highest file, or the
number an issue already reserved for it. `cargo test --test decisions` checks the file names and numbers, and that
every reference to a decision ("decision N", or `[N]` in `sdlc/`) resolves to a file.

## Round 1 (2026-09-24)

Decisions 1-8: `sdlc/decisions/`.

## Round 2 (2026-09-24)

Decisions 9-17: `sdlc/decisions/`.

## Round 3 (2026-09-24)

Decisions 18-25: `sdlc/decisions/`.

## Round 4 (2026-09-24)

Decisions 26-35: `sdlc/decisions/`.

## Round 5: after spikes S1–S4 (2026-09-24)

Decisions 36-39: `sdlc/decisions/`.

## Round 6: milestone 1 plan (2026-09-24)

Decisions 40-42: `sdlc/decisions/`.

## Implementation (2026-09-24)

Decisions 43-79: `sdlc/decisions/`.

## Milestone 2, first decisions (2026-09-27, before planning)

Recorded early at the user's request; planning proper starts later and may refine them.

Decisions 80-91: `sdlc/decisions/`.

## Round 7: milestone 2 planning (2026-09-29)

Decisions 92-138: `sdlc/decisions/`.

## Round 8: milestone 2b planning (2026-10-01)

Decisions 139-198: `sdlc/decisions/`.

## Round 9: templates milestone planning (2026-10-15, issue 211)

Planning interview for sandboxes created from a prebuilt `sbx` template, one question at a time, from spike S11
(`sdlc/spikes/S11.md`).

Decisions 199-246: `sdlc/decisions/`.

## Round 10: spike S14, Docker Cloud Sandboxes (2026-10-23, issue 245)

Decisions 247-248: `sdlc/decisions/`.

## Round 11: the second retro (2026-10-23)

Decisions 249-251: `sdlc/decisions/`.

## Round 12: milestone 4 planning, cloud sandboxes (2026-10-23, issue 310)

Planning interview for running `sbxm task`'s sandboxes on Docker Cloud Sandboxes, one question at a time, from spike
S14 (`sdlc/spikes/S14.md`) and decision 248; spike S17 (`sdlc/spikes/S17.md`) ran during the interview. The plan is
`sdlc/milestone-4.md`.

Decisions 252-265: `sdlc/decisions/`.

## Round 13: auto-merge (2026-10-24)

Decision 266: `sdlc/decisions/`.

## Round 14: milestone 4's capture run (2026-10-24)

Decision 267: `sdlc/decisions/`.

## Round 15: PR 322 review (2026-10-27)

Decisions 268-270: `sdlc/decisions/`.

## Round 16: the third retro (2026-10-30)

Number 271 is taken by issue 330's HTTP transport (`ureq`), recorded on the parked branch of PR #339.

Decisions 272 onwards: `sdlc/decisions/`. New decisions go there too, each as a new file.

## Round 17: auto-merge guardrails (2026-11-10)

Decisions 316: `sdlc/decisions/` (issue #460's planning interview).

## Round 18: local and cloud tasks at once (2026-11-11)

Decision 317: `sdlc/decisions/` (issue #561's planning interview; evidence from #390).

## Round 19: the 2026-11-11 retro (2026-11-11)

Decisions 318-320: `sdlc/decisions/` (`sdlc/retros/2026-11-11-retro.md`).

## Open research spikes
- **S1–S4:** Done 2026-09-24; see "Spike results" below.
- **S6:** Done 2026-09-24 (partial); spec `sdlc/spikes/S6.md`, evidence `sdlc/spikes/S6-results.md`, conclusions in decisions 48 and 49.
- **S6b:** Done 2026-09-25 (partial); spec `sdlc/spikes/S6b.md`, evidence `sdlc/spikes/S6b-results.md`, conclusions in decisions 59–61. Q1 (non-empty skills store) blocked and left open [60]. Pi moved to a later spike before slice 20 (needs `kit.allowedSources` [48]).
- **S7 (Pi):** Done 2026-09-27 (all questions answered); spec `sdlc/spikes/S7.md`, evidence `sdlc/spikes/S7-results.md`, conclusions in decisions 73–75.
- **S9 (M2, before slice 14):** how the Jev token reaches the Jev call — custom `sbx secret` service and host versus a host env-var fallback [36][120]; spec in `sdlc/milestone-2.md`. **Run 2026-10-02** with a fake token (PR #65, `sdlc/spikes/S9.md`): the token reaches the call through `sbx secret set-custom` scoped to an evaluator sandbox, and the proxy swaps a placeholder only in headers sent to the named host; the proposed design is not yet a decision. A spike with the real token (S12) comes before slice 14. Deferred with slice 14 [138].
- **S10 (M2b, git bundle from a sandbox):** Done 2026-10-01 (passed), recorded in decision 161.
- **S8 (GitHub access from a sandbox):** out of M2b [141]; written 2026-09-27, not run; spec `sdlc/spikes/S8.md`. Can issue workers [80–82] push, open PRs and edit issues through `sbx`'s proxy with a repo-scoped token, and do branch protection, no Workflows permission and a narrow allowlist hold? Q1–Q2 need no token; Q3–Q4 need a scratch repo and a stored `github` secret, which the repo owner doesn't plan to set up (a fork's owner may).
- **S5 (M2):** Headless runs per harness inside `sbx`. Done: Claude, Codex, Antigravity (see "Spike results" below) [93][103]. Gemini CLI is blocked/deprecated upstream and dropped as an M2a harness [103]. Pi deferred. Prompt in, run to completion, extract answer + transcript + token usage (decision 13). Leads: the v3 kit spec has `agent-sessions@1` (`prompt: ["-p", "{{.Prompt}}"]` for claude), but `sbx` 0.43 has no CLI verb for it. Route used: `sbx exec <sandbox> <harness-cli> <headless flags>` (Claude, Codex) or `sbx run --kit <pinned> <agent> <workspace>` + `sbx exec` (Antigravity).
- **S15 (templates milestone, Pi):** not run yet [204]; spec `sdlc/spikes/S15.md` (approved by the user 2026-10-15), results template `sdlc/spikes/S15-results.md`. Does a template saved from a `pi` sandbox work with `sbx create -t TAG --pull never` plus sbxm's kits, given that Pi's `agent_arg` is a kit reference of its own [73]: does the sandbox start, does Pi answer headless, do the always-loaded `AGENTS.md` and the provider secret still work, and does the kit's allow list apply? Decides whether Pi joins the milestone.

## Spike results (2026-09-24, sbx v0.43.0, Windows)

**S1: CLI surface**
- Lifecycle: `create`, `run` (creates if missing, then attaches), `exec` (**starts a stopped sandbox**), `stop`, `rm -f` (non-interactive; also deletes sandbox-scoped secrets and kit policies), `ls --json` (name, id, agent, status, last_used_at, workspaces).
- `ls --json` does **not** show kits or env, so sbxm must keep its own host-side state (sandbox → project, profile, config hash).
- Sandbox names: letters, digits, `-`, `.`; `default` is reserved. Our `[a-z0-9-]` rule is a subset, and we must also reject `default`.
- Defaults are all host CPUs and 16 GiB memory. Comparisons must pass `--cpus`/`-m` explicitly.
- Built-in agents: claude, codex, copilot, cursor, devin, docker-agent, droid, gemini, kiro, opencode, shell. **Pi is a kit**: `docker/sbx-kits-contrib/pi` (image `docker.io/sbx/pi-image`, Anthropic only: API key or OAuth).
- Workspace is mounted at a path mirroring the host (`E:\x\ws` → `/e/x/ws`).
- **Windows gotcha:** a workspace under `%TEMP%` (AppData) fails with `ERROR: failed to run sandbox container`; the same create on `E:\` works. **Corrected 2026-09-25 [56]:** any workspace on `C:` fails on this machine, not only AppData. `doctor` should check the base dir location. Disk: each harness image is several hundred MB (14 GiB free at test time).
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

**S5 (Claude done 2026-09-24; Codex and Antigravity done 2026-09-29 [93][103]; Gemini CLI blocked; Pi deferred): headless via `sbx exec` works**
- Claude: tested `sbx exec -w <workspace-in-container> <sandbox> claude -p "<prompt>" --model <id> --max-turns N --output-format stream-json --verbose`. Exit 0 in about 6 s; the file written by the agent appeared on the host workspace.
- `exec` sessions *do* get the proxy environment (`HTTPS_PROXY`), and the claude kit already sets `apiKeyHelper` (proxy-managed key) and `bypassPermissions`. Init event reports `apiKeySource=apiKeyHelper`, `permissionMode=bypassPermissions`.
- The stream-json output has everything we capture: `system/init` (model, cwd, session_id), `assistant` events with tool calls (the transcript), and a final `result` event with `result` (final answer), `num_turns`, `duration_ms`, `total_cost_usd`, `usage` and per-model `modelUsage`.
- Cap flags: `--max-turns` (works, not listed in `--help`) and `--max-budget-usd` (real dollar cap).
- **Killing `sbx exec` on the host does NOT stop the process inside the sandbox.** Timeouts must be enforced inside the sandbox: `sbx exec <sandbox> timeout --kill-after=<grace> <limit> <harness cmd>`. Verified: exit code 124, process gone. Fall back to `sbx stop` if needed.
- **Codex** (codex-cli, `docker/sandbox-templates:codex-docker`): `codex exec [-m <model>] --json --skip-git-repo-check "<prompt>"` (`--skip-git-repo-check` is required for a workspace that isn't a git repo, which an unseeded M2 contestant's empty directory won't be — decision 5 — so the headless-run primitive must always pass it, or always `git init` first). `sbx exec` attaches a stdin that `codex exec` then blocks reading even when a prompt argument is given ("Reading additional input from stdin…"); piping empty stdin (`"" | sbx exec …`) unblocks it, so the primitive must close/redirect stdin explicitly. `--json` prints NDJSON events: `thread.started` (`thread_id`), `turn.started`, `item.completed` (`item.type: "agent_message"`, `text`: the final answer — tool calls presumably appear as other item types, unverified), `turn.completed` (`usage`: `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens` — no dollar-cost field, unlike Claude's `total_cost_usd`). `-o/--output-last-message <FILE>` gives a clean final-answer extraction alternative to parsing the last `item.completed`. No built-in budget-cap flag found in `--help` (only `-m`, `-c` config overrides, `-s/--sandbox`, `--enable/--disable` features) — Codex likely falls into decision 98's "no budget flag, timeout-only" branch. Timeout verified: `sbx exec <sandbox> timeout --kill-after=5 3 codex exec --json --skip-git-repo-check "<long task>"` → exit 124.
- **Gemini CLI is blocked, not just untested** [103]: `gemini -p "<prompt>" --output-format stream-json --yolo --skip-trust` (flags exist and look right — `-o/--output-format` takes `text|json|stream-json`, same shape as Claude's) but the built-in `gemini` sbx agent's kit grants **no Google network permissions at all**: `sbx policy log` showed `generativelanguage.googleapis.com:443` and `play.googleapis.com:443` denied by default-deny, and even interactive OAuth sign-in failed the same way (`oauth2.googleapis.com:443` denied) — there is no working route to a live Gemini call on this setup, with or without a stored `google` secret (none was stored; the sign-in failure means one couldn't be obtained either). This contradicts decision 29's assumption ("built-in harness kits already allow their own provider domains") for Gemini specifically; Claude's and Codex's kits do pre-allow their provider hosts (re-confirmed: Codex's PONG test above actually reached OpenAI). **Consequence [103]:** Gemini CLI is dropped as an M2a contestant harness in favor of Antigravity; M1's `--harness gemini` remains structurally intact (sandbox creation, home files, skills-store warning all still work) but has never made a real Gemini API call, on this machine — worth a GitHub issue (must-fix or question) once a session with GitHub access can file one, since it affects M1 users too, not just M2.
- **Antigravity** (`agy`, kit `docker.io/sbx/antigravity-kit:latest`, agent name `antigravity` per `sbx run`) [103]: not a built-in `sbx` agent — needs `sbx run --kit docker.io/sbx/antigravity-kit:latest antigravity <workspace>` (pinned-tag pattern like Pi, decisions 73–75; `latest` used here, pinning to be decided in the milestone plan). Its kit *does* pre-allow Google hosts via `forward-bypass` policy rules (not the plain proxy path): `oauth2.googleapis.com`, `www.googleapis.com`, `play.googleapis.com`, `lh3.googleusercontent.com`, `daily-cloudcode-pa.googleapis.com`, `antigravity-cli-auto-updater-*.run.app`, `antigravity-unleash.goog` — proof a correctly-built kit can pre-allow these, unlike the built-in `gemini` agent. Headless invocation: `agy -p "<prompt>" --output-format stream-json --dangerously-skip-permissions` (also has `--effort low|medium|high|max`, `--model`, its own `--print-timeout` as an alternative to `sbx exec timeout`, and `--input-format stream-json` for multi-turn NDJSON input). Verified live: NDJSON events `init` (`conversation_id`, `tools` list, `permission_mode`), `step_update` (per turn, `state: DONE`, `agent_response` steps carry `text_delta` and per-step `usage`), and a final `result` event with `status: "SUCCESS"`, `response` (final answer), `duration_seconds`, `num_turns`, and `usage` (`input_tokens`, `output_tokens`, `thinking_tokens`, `cache_read_tokens`, `total_tokens` — no dollar cost, same gap as Codex). Workspace must be off `C:` (decision 56 applies here too — first attempt on `C:\Users\james\antigravity` failed with the same "workspace directory was not mounted" error). Timeout/kill behavior not yet verified (its own `--print-timeout` may make `sbx exec timeout` unnecessary — for the milestone plan to decide which to rely on).
- Still to test: Pi headless mode and output format (deferred, decision 93); Antigravity's timeout/kill behavior; parallel sandboxes running concurrently.

**S4: Secrets (decision 30: mostly handled by sbx)**
- Values never enter the sandbox. Service secrets (anthropic, openai, google, openrouter, github, …) and custom secrets use placeholders that the proxy swaps on requests to allowed hosts. Verified: a filesystem-wide search inside the sandbox found no secret value.
- There is no command to read values back. `sbx secret ls --json` returns scope, service/env and a **masked** value, which is enough for an existence check. Secret scanning of outputs isn't needed; only placeholders can leak.
- Secrets apply at sandbox creation (existing sandboxes don't pick up new ones). Sources can be `--ref op://…` (1Password), AWS Secrets Manager or `--command`. OpenAI OAuth is supported: `sbx secret set openai --oauth`.

**S17 (2026-10-23, `sbx` v0.46.0, cloud): network rules can be removed from a running cloud sandbox**
- `policy rm network --sandbox <name> --resource <host> -f` and `policy deny network --sandbox <name> <host>` exit 0,
  print the new list and take effect at once, also for a host of the agent's built-in list. Answer 4 B holds
  [255]. Write-up: `sdlc/spikes/S17.md`.

**S16 (2026-10-29, `sbx` v0.46.0, `agy` 1.4.10): Antigravity runs on a placeholder token with a host-refreshed secret**
- A placeholder token file plus a scoped `set-custom --command` secret on two hosts gives a working headless `pong`;
  `agy` never refreshes or validates the token itself; `sbx` calls the helper per request. Adopted as [270].
  Write-up: `sdlc/spikes/S16.md`.

**S18 (2026-11-02, `sbx` v0.46.0, local): the two tested harnesses can't use `sbx`'s MCP gateway yet**
- The gateway serves a registered server (`sbx mcp add`, chosen per sandbox by `--static-mcp` or `mcp load`) to a raw
  client, but never answers a notification; Claude Code times out after 30 s (inferred: on that notification). Codex
  fails separately: it ignores the image's `headers`/`type` keys and never sends the bearer header. Antigravity was
  not re-run; #287's hang is plausibly the same gateway fault (inferred). Claude Code also reaches the account's
  claude.ai connectors through Anthropic's MCP proxy, past the allow list; `ENABLE_CLAUDEAI_MCP_SERVERS=false` removes
  them (#381). Answers #332's point 1. Write-up: `sdlc/spikes/S18.md`.

### Jev facts relevant to design (from docs, Jev 1.13)
- HTTP API only (`POST https://api.typesafe.ai/v1/systemone`, Bearer auth). SDKs exist only for Python and JS, so Rust calls it with `reqwest`.
- Primitives: **Noul** (P(yes)), **Choice** (one of ≤255 options + distribution), **Score** (2–10 ordered levels, probability-weighted). Returns typed answers + probabilities/confidence, not text.
- Limits: 64k tokens per request; **32k for `state` + longest question**. Text only. $0.042 per 1M input tokens; output tokens are free.
- Fits evals via composite scoring: one question per rubric dimension, with weights applied in code.
