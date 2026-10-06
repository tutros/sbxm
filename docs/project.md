# sbxm project context

This file is the one place for the project's goals and current state; `AGENTS.md` and the `sbxm-context` skill link here for them. The code layout, commands, constraints and the always-on working rules stay in `AGENTS.md` (loaded every session), so they are linked below, not copied. Read the files your task touches; verify anything time-bound with `gh` before trusting it.

## Goals (user-confirmed 2026-10-05)

1. **Software factory:** launch sandboxes with a profile (kits) to produce or change code from a PRD/spec, a GitHub issue or a review. The task source is pluggable. Command: `sbxm task`.
2. **Comparisons:** run 2-4 contestants (harness x model x profile) on the same task and evaluate them. Command: `sbxm run`.

Plus the lifecycle commands (`new`, `open`, `list`, `stop`, `rm`, `config`, `doctor`). What sbxm is and the constraints it follows: `AGENTS.md`, "What `sbxm` is" and "Architectural constraints".

## State (as of 2026-10-06)

- Merged: milestones 1, 2a (`sbxm run`) and 2b (`sbxm task`); decision 169 (continue work on an open PR: #102, #103, #122); `run.log` for task commands (#127, decision 170 part 1); the state machine's table and its guards (#115, PR #133).
- The state machine is designed (decisions 173-177, `sdlc/specs/task-state-machine.md` section 5, `sdlc/spikes/state-table.md`, the table in `src/task/machine.rs`). Remaining, in waves with a checkpoint: #117 (`fix_rounds` loop) with #116 (`task states`), then **pause and check a real multi-round run**, then #119 + #121 (`Repeat of:` marker, no-progress stop, `task start --spec`, `[finish] sink`), then #118 + #120 (`task resume`, `--rounds N`, draft PR), then #123 (PR risk assessment, decision 176). #107/#108 were held until #115 was done, so they are next.
- Separate tracks: dashboard #88-#92, #114, #111, #106, #126, #130, #135.
- Log of gaps found while using `task`, run logs and the agenda: `sdlc/evals-workflow-notes.md`.

## Where facts live

| Need | Read |
|---|---|
| Code layout, commands, constraints, working rules | `AGENTS.md` |
| Any design question | `sdlc/decisions.md` (numbered; add new ones, never depart silently; the next free number is after the last entry) |
| `sbxm task` behavior | `sdlc/spec-m2b.md`, `sdlc/prd-m2b.md` |
| Task state machine | `sdlc/specs/task-state-machine.md`, `sdlc/spikes/state-table.md`, `src/task/machine.rs` |
| How the commands fit together | `docs/workflow.md`; `README.md` is user-facing, keep it in step with behavior |
| Past reviews, spikes | `sdlc/reviews/`, `sdlc/spikes/` |
| Open work | `gh issue list`, `gh pr list` (repo `tutros/sbxm`; review findings are issues) |

Phase skills: `sdlc-planning`, `sdlc-implementation` (TDD, commit per green step), `sdlc-code-review`.

## Practical notes

- Build, test and lint commands, and the Bash `\\` quirk: `AGENTS.md`, "Commands" and its notes. The suite is slow and flaky under load on Windows; the gates inside a sandbox (Linux) are the reliable ones. A Windows `Permission denied` from `git` or a file lock is usually transient: retry after a few seconds.
- Run long jobs in the background so you are told when they end.

## Using sbxm on itself (the e2e setup)

- Use a release binary built from the commit being worked on, copied to a named file (`sbxm-<sha>.exe`) with its SHA-256 recorded, run with its own `SBXM_CONFIG_DIR` and a base dir off `C:` (decision 56).
- `task run --issue N` = start + gates + review + at most one fix round, then stops at `ready`. `task finish` (the user decides) pushes with the user's login. `--reviewer-harness`/`--reviewer-model` override the reviewer; the config is read once at start, so changing `sbxm-task.toml` mid-run has no effect (use `--restart` to start over). An issue with a `PR: #n` line for a merged PR is refused (decision 169).
- `task rm` shows what it deletes; clean up finished tasks, worktrees and build dirs only with the user's OK.
- antigravity (`agy`): model ids come from `agy models`; set `model` explicitly (the default is believed to be Gemini 3.1 Pro but is unverified). Needs the `google` secret.
- Typical timing of one task with a fix round (28 min): about two thirds agent work, one third sandbox setup; the `setup.install` steps (mostly `cargo install just`, ~95 s) are about 2m40s per sandbox, and the reviewer's sandbox is set up again each round.

## Working rules the user gave beyond `AGENTS.md`

The always-on rules (every change through a branch and a PR, never push to `main`, `[current] of [total]` progress, up to 3 parallel sessions) are in `AGENTS.md`, "Workflow" and "Working rules". Also:
- Keep messages short; one decision at a time with a recommendation; say unprompted when things get complex or a request looks over- or underspecified.
- Don't pre-harden: log a gap with its trigger and fix it when hit.
- Severity: the user may downgrade edge-case must-fix findings to should-fix and log them as issues instead of running more review rounds.
- Forked subagents: tell them to stop after their deliverable and not start follow-ups; verify their claims against GitHub.
- Security tooling details (antivirus, TLS interception) stay local, never in repo docs, PRs or issues.
