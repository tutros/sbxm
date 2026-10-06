---
name: sbxm-context
description: Project context for sbxm - what it is, the two goals, where each kind of fact lives, how to run and test it, how `sbxm task` and `sbxm run` are used day to day, the current build plan, and the working rules. Use at the start of any session on this repo, before planning, implementing or reviewing, or when you need to know where something is documented.
---

# sbxm project context

A map, not a copy: `AGENTS.md` (read through `CLAUDE.md`) holds the code layout and constraints, and the files below hold the rest. Read the ones your task touches; don't rely on this file for facts that change.

## What it is

A Rust CLI over Docker Sandboxes (`sbx`). Goals (user-confirmed 2026-10-05):
1. **Software factory:** launch sandboxes with a profile (kits) to produce or change code from a PRD/spec, a GitHub issue or a review. The task source is pluggable. Command: `sbxm task`.
2. **Comparisons:** run 2-4 contestants (harness x model x profile) on the same task and evaluate them. Command: `sbxm run`.
Plus the lifecycle commands from milestone 1 (`new`, `open`, `list`, `stop`, `rm`, `config`, `doctor`). sbxm enforces nothing itself: `sbx` does egress, secrets and isolation.

## Where facts live

| Need | Read |
|---|---|
| Code layout, constraints, commands | `AGENTS.md` |
| Any design question | `sdlc/decisions.md` (numbered; add new ones, never depart silently; next free number is in the latest handoff or at the file's end) |
| `sbxm task` behavior | `sdlc/spec-m2b.md`, `sdlc/prd-m2b.md` |
| Task state machine (decisions 173-177) | `sdlc/specs/task-state-machine.md` section 5, `sdlc/spikes/state-table.md`, `src/task/machine.rs` |
| Gaps found while using `task`, run logs, next-session agenda | `sdlc/evals-workflow-notes.md` |
| How the commands fit together, practical notes | `docs/workflow.md`, `README.md` (user-facing; keep in step with behavior) |
| Past code reviews | `sdlc/reviews/` |
| Spikes | `sdlc/spikes/` |
| Open work | `gh issue list`, `gh pr list` (the repo is `tutros/sbxm`; review findings are issues) |

## Build plan in flight (verify with `gh` before trusting)

State machine issues #115-#121 are built in waves with a checkpoint: wave 1 #115; wave 2 #117 (fix_rounds loop) with #116 (`task states`); **pause and check a real multi-round run**; wave 3 #119 + #121; wave 4 #118 + #120; then #123 (PR risk assessment, decision 176); #107/#108 wait for #115. Separate tracks: dashboard #88-#92, #114, #111, #106, #126, #130.

## Running and testing

```
cargo build
cargo test                                   # no Docker needed, but slow and flaky under load on Windows
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```
- The gates inside a sandbox (Linux) are the reliable ones; a Windows `Permission denied` from `git` or file locks is usually transient, so retry after a few seconds.
- Base dir must not be on `C:` (decision 56). Never touch the real config in tests.
- Long jobs: run them in the background so you are told when they end.
- Bash tool quirk: it collapses `\\`; a hook blocks such commands. Use PowerShell or the Write/Edit tools for backslashes (details in `AGENTS.md`).

## Using sbxm on itself (the e2e setup)

- Use a release binary built from the commit being worked on, copied to a named file (`sbxm-<sha>.exe`) with its SHA-256 recorded, and run it with its own `SBXM_CONFIG_DIR` and base dir off `C:`.
- `task run --issue N` = start + gates + review + at most one fix round, then stops at `ready`. `task finish` (the user decides) pushes with the user's login. `--reviewer-harness`/`--reviewer-model` override the reviewer; the config is read once at start, so changing `sbxm-task.toml` mid-run has no effect (use `--restart` to start over).
- `task rm` shows what it deletes; clean up finished tasks, worktrees and build dirs only with the user's OK.
- antigravity (`agy`): model ids come from `agy models`; set `model` explicitly (the default is believed to be Gemini 3.1 Pro but is unverified). Needs the `google` secret.

## Working rules (from the user)

- Every change, docs and plans included, goes to a branch and a PR after an initial code review; an independent reviewer runs on the PR; **the user merges; never push to `main`**.
- Follow the phase skills: `sdlc-planning`, `sdlc-implementation` (TDD, commit per green step), `sdlc-code-review`.
- Show progress as `[current] of [total]` whenever the total is known.
- Say when work can run in parallel (max 3 sessions including this one); don't start extra sessions unasked.
- Keep messages short; one decision at a time with a recommendation; say unprompted when things get complex or a request looks over- or underspecified.
- Don't pre-harden: log a gap with its trigger and fix it when hit.
- Severity: the user may downgrade edge-case must-fix findings to should-fix and log them as issues instead of running more review rounds.
- Forked subagents: tell them to stop after their deliverable and not start follow-ups; verify their claims against GitHub.
- Security tooling details (antivirus, TLS interception) stay local, never in repo docs, PRs or issues.
