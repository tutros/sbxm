# AGENTS.md

This file provides guidance to coding agents (Claude Code reads it through `CLAUDE.md`) when working with code in this repository.

## Status

Milestones 1 to 4 are merged to `main` (M1 sandbox lifecycle; M2a comparisons and M2b `sbxm task`; M3 prebuilt templates; M4 cloud sandboxes). The goals, the current state and the build plan in flight live in one place, `docs/project.md`; the backlog and the work in flight are in issue #405. `README.md` is the user-facing documentation: keep it in step with behavior changes. Sources of truth:
- `sdlc/idea.md`: original brief
- `sdlc/milestone-1.md`: milestone 1's implementation plan (crate layout, config schema, kit mapping, commands, build order) — done, kept for reference.
- `sdlc/milestone-2.md`: milestone 2's implementation plan (M2a comparisons, done; M2b issue-worker migration, slices 0–13).
- `sdlc/milestone-3.md`: milestone 3's plan, sandboxes from a prebuilt `sbx` template (issue #211, decisions 199-218 and 221); planned 2026-10-15, slices 0 and 0b done 2026-10-16 (decisions 217, 221).
- `sdlc/milestone-4.md`: milestone 4's plan, `sbxm task` sandboxes on Docker Cloud Sandboxes (issue #310, decisions 252-265); planned 2026-10-23, P-choices confirmed 2026-10-24 (decisions 262-265).
- `sdlc/prd-m2b.md` and `sdlc/spec-m2b.md`: M2b's requirements (what and why, success criteria S1–S9) and exact behavior (modules, `sbxm-task.toml`, `task.json`, commands, pipelines, gates, git trust boundary).
- `sdlc/decisions/`: numbered design decisions, one file each (`NNNN-<slug>.md`, decision 303); `sdlc/decisions.md` keeps the rounds, open research spikes and spike results. **Read them before designing anything**, and add new decisions instead of silently departing from them: a new decision is a new file in `sdlc/decisions/`, with the next free number (one above the highest file, or the one its issue reserved).
- `docs/workflow.md`: how the commands fit together, with the use cases and practical notes (disk, binaries, gates). `sdlc/evals-workflow-notes.md`: the log of gaps found while using `sbxm task` (G1 onwards, the issue 63 run log and the next-session agenda); `sdlc/spikes/`: spike write-ups (S9 done, S11 done 2026-10-11: templates work, about 7 s per sandbox instead of 68 s, and an empty allow list for a task that leaves `Cargo.lock` alone; the feature is issue #211, S13 done: the flaky tests of #104 don't show in a Linux sandbox).
- `sdlc/reviews/`: code review records (`<date>-<scope>.md`), in the format the code-review skill defines; `sdlc/specs/`: task specs for work done in a separate session (`sdlc/specs/file-review-issues.md` was built as a PowerShell script, then ported to `sbxm task file-findings`; decisions 134 and 165); `sdlc/batches/`: unattended batch specs and their logs.

## Workflow

The repo is `tutros/sbxm` on GitHub (public since 2026-09-27, `MIT OR Apache-2.0`; `origin`, branch `main`). Code review findings are GitHub issues there (decision 79); close them from fix commits with `Fixes #n`. Every change, docs and planning files included, goes to a branch and a PR after an initial code review, and the independent reviewer runs on the PR before the user merges: for a PR `sbxm task finish` opened, that is the in-task review it posts while the PR's head is the reviewed commit, and `sbxm task review --pr N` for any later commit and every other PR; never push to `main` (decisions 86, 187). A low-risk PR (the reviewer's level and the path-rule floor) whose latest review covered its head commit with 0 must-fix is merged by GitHub's auto-merge without the user, armed with `--match-head-commit` set to that commit and disarmed before any later push (decisions 266, 280; medium and high risk stay the user's; a break found after an auto-merge pauses it until the user decides). To fix an open PR with a worker, follow `docs/workflow.md`, "Fix an open PR (or a task's branch) with a worker". A PR made in the session, not by a worker, first goes
through the self-review in `sdlc-implementation` ("Before reporting a change done"), and a commit pushed after a PR's
last review gets a narrow re-review or is named in the PR as unreviewed (decisions 251, 289).


Each SDLC phase has a project skill in `.claude/skills/`; follow it for that phase:
- `sdlc-planning`: interview rounds, numbered decisions, spikes, milestone plan.
- `sdlc-implementation`: vertical slices, TDD (red first), smallest change per step, commit after every green step (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`). A milestone is built as one batch on one branch (rule 8): a short report per slice, a background review of each risky slice while the next is built (decision 162), one full review at the end.
- `sdlc-code-review`: review a milestone at its end (the normal review), a worktree before merging, or a slice when asked or risky; findings need evidence, fixes go back through TDD.
- `sdlc-issue-flow`: one issue from open to merged and cleaned up, stage by stage (decision 279, an experiment); the checklist an orchestrator follows.
- `sdlc-retro`: look back over the work since the last retro (numbers per task, patterns with at least two instances, last retro's experiments) and propose at most three process changes as experiments for the user to decide; records in `sdlc/retros/` (decision 196).

## Working rules

- **Show progress as `[current] of [total]` (user rule, 2026-10-01).** Whenever you mention a step that is complete or in progress, give its position and the total: "slice 4 of 13", "test case 3 of 7", "step 2 of 5". This covers milestone slices, test cases within a slice, review rounds, manual steps for the user, and any other sub-step whose total is known. If the total isn't known yet, say so rather than leaving it out.
- **Say when work can run in parallel, up to 3 sessions in total (user rule, 2026-10-01).** Whenever upcoming work has parts that don't depend on each other (independent slices, a background review next to the next slice, a research spike, docs next to code), say so and name each part as its own session, counting the current one: at most 3 at once. Say which parts must wait and why (shared files, one part's output feeding the other). Don't start extra sessions without the user's go-ahead.
- **No open questions go to a worker (user rule, 2026-10-11, issue #206).** Before `sbxm task start` or `task run`, answer every "to decide when building" point of the issue or spec with the user, one at a time with a recommendation, and write the answers into the issue or spec. A worker in a sandbox can't ask, so an open point becomes a guess that the review then argues about. Ask one more question there (decision 244): does the change have a limit, a size, or an identity (path, drive, ref) that decides behavior? If yes, the issue lists the boundary cases (at the limit, one over, one under, the largest input) and the worker writes those tests first.
- **Check external facts first, and run the real check early (decision 273, an experiment).** Before a worker starts on an issue that depends on an external tool's behavior a spike recorded, re-run the spike's key check by hand: self-updating tools change within days (`agy` and the cloud image did, #330 and #337). After the worker, before the review rounds, run the issue's real test (an ignored test or a by-hand smoke) and give its result to the review.
- **Promised work goes to GitHub at once (decision 278, an experiment).** A follow-up you promise the user and don't do in the same turn becomes a checkbox under `## Follow-ups` in its open home issue, or else an issue labelled `follow-up` with a size. An agent's memory holds only pointers and facts local to one machine.
- **Every issue gets a size, twice (user rule, 2026-10-13).** When an issue is created, by hand or from a review finding, give it a `size:S`, `size:M` or `size:L` label and a `**Size:**` line with the reason; the table is in the `sdlc-code-review` skill (section 8, "Size"). When its open points are answered under the rule above, confirm or change the size in the issue and on the label: the scope is only fixed then.

## Commands

```
cargo build
cargo run -- --help                          # run the CLI
cargo test                                   # all tests (no Docker needed)
cargo test --test cli help_prints_usage      # a single test
$env:SBXM_REAL_BASE_DIR='H:\sbxm-it'; cargo test --test real_sbx -- --ignored   # real sbx; needs `sbx login`, dir not on C: (decision 56)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
just script-test                             # Pester 5 tests for scripts/deploy-profiles.ps1 (decision 136)
just deploy-profiles                         # copy profiles/* into the profiles_dir sbxm reads
```

The `justfile` wraps these (`just check`, `just real-test`) plus representative sbxm commands (`just` lists them).

## Code layout

Crate `sbxm` (edition 2024), lib + bin: everything lives in the lib; `src/main.rs` parses args (`src/cli.rs`, clap derive) and passes `SbxBackend` to the commands. One file per subcommand in `src/commands/`. All `sbx` calls go through the `SandboxBackend` trait (`src/backend/`; `FakeBackend` in tests, `SbxBackend` shells out); all `gh` calls through `GitHubBackend` (`src/github/`, `FakeGitHub`). The areas: `src/task/` (`sbxm task`: config, `task.json`, pipeline phases, review, finish, resume, lock, cloud lifetimes), `src/run/` and `src/eval/` (`sbxm run` comparisons, judge, Jev, cosine), `src/template.rs` (prebuilt templates), `src/slot.rs` (the machine-wide sandbox slots, decision 304), `src/kit.rs` (the only module that knows the kit format), `src/config.rs`, `src/harness.rs`, `src/headless.rs`, `src/status/`. Search the code and read a module's `//!` doc comment where it has one (many say what the module owns and which decisions it follows), rather than a map here (the long map was removed by decision 287, an experiment). Tests: one file per command in `tests/`, helpers in `tests/common/`; `tests/real_sbx.rs` and `tests/real_sbx_cloud.rs` are ignored tests against the real `sbx`. Never touch the real config.

On Windows, cargo can print `error finalizing incremental compilation session directory … Access is denied`. It's a harmless filesystem-lock warning, not a lint failure.

The Bash tool here turns every `\\` in a command into `\` before bash runs it: in heredocs, single quotes and scripts alike (a lone `\` is unaffected; the PowerShell tool keeps backslashes intact). A `PreToolUse` hook (`.claude/hooks/block-double-backslash.py`, registered in `.claude/settings.json`) blocks any Bash command containing `\\`. Write files that need backslashes (JSON fixtures, Windows paths, Rust `\n` escapes inside strings) with the Write/Edit tools, or use PowerShell. Exception: the Write/Edit tools decode a JSON-style unicode escape (backslash, `u`, four hex digits) into its character, so write those with PowerShell (`[char]92`) or build them in code (`char::from(92)`).

## What `sbxm` is

A Rust CLI (crate/binary name `sbxm`) that wraps Docker Sandboxes (`sbx`) to:

1. **Milestone 1: sandbox lifecycle.** Create or reuse a sandbox for a project at `<base_dir>/<project>`, applying a shared, versioned common config (egress allowlist, secret references, instructions file, per-harness hooks/guardrails). Commands: `new`/`open`, `list`, `stop`, `rm`, `config show`, `config profiles-dir`, `doctor`.
2. **Milestone 2: comparisons.** Run 2–4 *contestants* (`harness × model × optional config profile`) headless and in parallel on the same task, each in a fresh snapshot of an optional seed directory. Capture answer, diff and transcript, then evaluate with any subset of executable checks, rubric-based LLM judge, Jev (TypeSafe System One), cosine similarity (text answers only) and human review.

## Architectural constraints (from sdlc/decisions/)

- **sbxm enforces nothing itself:** `sbx` handles egress (deny-by-default proxy), secrets (proxy-injected placeholders) and isolation. sbxm compiles config into **schemaVersion "2" mixin kits** (one shared plus one per harness pinned with `requires.agent`) and passes them to `sbx create --kit`. `sbx ls --json` doesn't expose kits or env, so sbxm keeps its own host-side state. See "Spike results" in `sdlc/decisions.md` for verified `sbx` behavior.
- **Mandatory instructions** go in each harness's always-loaded user-level file via the per-harness mixin's `files/home/`. Kit `agentInstructions` is only surfaced on demand, so it's for reference material only.
- **Timeouts must be enforced inside the sandbox** (`sbx exec … timeout --kill-after=… <limit> <cmd>`). Killing `sbx exec` on the host leaves the agent process running.
- **Base dir must not be on `C:`:** on this Windows machine `sbx` fails to mount any workspace on `C:` (not only `%TEMP%`/AppData; decision 56). Any other volume works; this machine's `base_dir` is on `I:` (an SSD, since 2026-11-05).
- **`SandboxBackend` trait:** all `sbx` interaction goes through it (shelling out to the `sbx` CLI). A `bollard` backend may be added later, and tests use a fake backend by default.
- **Harness adapters:** each harness (Claude Code, Codex, Pi, Antigravity) gets an adapter that owns its headless command, transcript/usage parsing, native config file names (`CLAUDE.md`, `AGENTS.md`, Antigravity's `.gemini/AGENTS.md`, hooks), and provider egress domains. If a configured feature is unsupported, warn loudly; never drop it silently.
- **Config:** TOML layered global (`~/.config/sbxm/config.toml`) → named profile → project `sandbox.toml`. Record a hash of the merged config with every sandbox and run. A changed hash on an existing sandbox requires `--rebuild`.
- **Security is a core goal:** egress is deny-by-default. Config only *names* secrets (values live in `sbx secrets`), and secret values must never reach results files. Seed directories are copied into a fresh git repo with a single baseline commit (no history or remotes). Project names are restricted to `[a-z0-9-]`, and reserved Windows names are rejected. `rm` deletes project directories only with `--purge` plus confirmation.
- **Evals:** rubric criteria are structured (`id`, `kind` = `pass_fail` | `scale` with levels, `weight`, `notes`) so Jev, the LLM judge and humans score the same criteria. Jev-specific questions are a separate extension point. Contestants are scored independently by default and ranked in code, with blind A/B/C labels for judges and humans.
- **Jev:** HTTP API only (no Rust SDK), called by a fixed `curl` command inside a restricted evaluator sandbox, never by a host HTTP client, so the token stays in `sbx`'s scoped secret (decision 241). 32k-token limit on state plus longest question, so it never receives transcripts, and large diffs are split per file. Docs: https://docs.typesafe.ai/llms.txt. The `typesafe@typesafe-ai` Claude Code plugin (skill `/typesafe:typesafe-ai`) is installed for reference.
- **Results:** plain files under `<base>/<project>/runs/<run-id>/<contestant>/` are the source of truth. `--repeat N` is part of the run model. Timed-out runs keep their partial output and are still evaluated.

## Environment Notes

- Primary platform is Windows (PowerShell) with Docker Desktop. Keep path handling cross-platform (`std::path`, no hard-coded separators).
- Sandbox names are `sbxm-<project>-<harness>`. sbxm metadata lives in `<base>/.sbxm/<project>/`, never inside the mounted workspace.
