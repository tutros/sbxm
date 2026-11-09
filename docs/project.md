# sbxm project context

This file is the one place for the project's goals and current state; `AGENTS.md` and the `sbxm-context` skill link here for them. The short code layout, commands, constraints and the always-on working rules stay in `AGENTS.md` (loaded every session), so they are linked below, not copied. Read the files your task touches; verify anything time-bound with `gh` before trusting it.

## Goals (user-confirmed 2026-10-05)

1. **Software factory:** launch sandboxes with a profile (kits) to produce or change code from a PRD/spec, a GitHub issue or a review. The task source is pluggable. Command: `sbxm task`.
2. **Comparisons:** run 2-4 contestants (harness x model x profile) on the same task and evaluate them. Command: `sbxm run`.

Plus the lifecycle commands (`new`, `open`, `list`, `stop`, `rm`, `config`, `doctor`). What sbxm is and the constraints it follows: `AGENTS.md`, "What `sbxm` is" and "Architectural constraints".

## State (as of 2026-11-08)

- **Merged:** milestones 1 (sandbox lifecycle), 2a (`sbxm run` comparisons, with Jev slices 14a and 14b), 2b (`sbxm task`), 3 (prebuilt templates, PR #269) and 4 (cloud sandboxes, PR #322). The history is in the milestone plans (`sdlc/milestone-*.md`), the decisions (`sdlc/decisions/`) and the retros (`sdlc/retros/`).
- **How work runs now:** issues are readied with the user (open points answered, a size, boundary cases; decisions 244, 295), labelled `ready-for-worker`, and run by `sbxm task loop` (decision 245) in the user's terminal, one sandbox at a time (16 GB of RAM). The loop resolves its PRs' conflicts (decision 298), re-runs and fixes failed checks (decisions 299, 300), checks sign-ins first (#445), and arms auto-merge for low-risk PRs (decisions 266, 280, 293); medium and high risk wait for the user. Its log goes under the base dir and is posted to the open `loop-log` issue (decision 305). A retro runs about every ten merged PRs (the last: `sdlc/retros/2026-11-08-retro.md`).
- **Roadmap, in order (the user's priority: smooth unattended loops first):** the machine-wide sandbox slot (#504, slice 1 of #476), then `task loop --check` (#511, decision 307) and fixing small should-fix findings in the same task (#510, decision 306), then #476's other slices (a job queue the loop drains, a scheduled start, toasts), so long commands no longer need the user's terminal; then review and CI time (#471, #473, #395); then planning guardrails for auto-merging medium-risk PRs (#460).
- **Directions, not yet planned:** a model router fed by task records (#419), a database for sbxm's state, GitHub's merge queue (each with its trigger in #405).
- **The live backlog and the work in flight:** issue #405. Gaps found while using `sbxm task`: `sdlc/evals-workflow-notes.md`.

## Where facts live

| Need | Read |
|---|---|
| Code layout, commands, constraints, working rules | `AGENTS.md` |
| Any design question | `sdlc/decisions/` (numbered, one file each; never depart silently: a new decision is a new file in `sdlc/decisions/`, numbered one above the highest file or as its issue reserved) and `sdlc/decisions.md` (the rounds and spikes) |
| `sbxm task` behavior | `sdlc/spec-m2b.md`, `sdlc/prd-m2b.md` |
| Templates milestone | `sdlc/milestone-3.md`, decisions 199-218 and 221, `sdlc/spikes/S11.md` |
| Task state machine | `sdlc/specs/task-state-machine.md`, `sdlc/spikes/state-table.md`, `src/task/machine.rs` |
| How the commands fit together | `docs/workflow.md`; `README.md` is user-facing, keep it in step with behavior |
| Past reviews, spikes | `sdlc/reviews/`, `sdlc/spikes/` |
| Open work | `gh issue list`, `gh pr list` (repo `tutros/sbxm`; review findings are issues) |

Phase skills: `sdlc-planning`, `sdlc-implementation` (TDD, commit per green step), `sdlc-code-review`, `sdlc-issue-flow`, `sdlc-retro`.

## Practical notes

- Build, test and lint commands, and the Bash `\\` quirk: `AGENTS.md`, "Commands" and its notes. The suite is slow and flaky under load on Windows; the gates inside a sandbox (Linux) are the reliable ones. A Windows `Permission denied` from `git` or a file lock is usually transient: retry after a few seconds.
- Run long jobs in the background so you are told when they end.

## Using sbxm on itself (the e2e setup)

- Use a release binary built from the commit being worked on, copied to a named file (`sbxm-<sha>.exe`) with its SHA-256 recorded, run with its own `SBXM_CONFIG_DIR` and a base dir off `C:` (decision 56).
- `task run --issue N` = start + gates + review + up to `[worker] fix_rounds` fix rounds (default 3), then stops at `ready`. `task finish` (the user decides) pushes with the user's login. `--reviewer-harness`/`--reviewer-model` override the reviewer; the config is read once at start, so changing `sbxm-task.toml` mid-run has no effect (use `--restart` to start over). An issue with a `PR: #n` line for a merged PR is refused (decision 169).
- `task rm` shows what it deletes; clean up finished tasks, worktrees and build dirs only with the user's OK.
- antigravity (`agy`): model ids come from `agy models`. A headless run with no model set (`sbxm task` worker or reviewer, or an `sbxm run` contestant or judge without `model`) gets `gemini-3.8-flash-medium`, the user's choice of 2026-10-24 (`ANTIGRAVITY_DEFAULT_MODEL` in `src/harness.rs`, issue 314), and the task and run records and output name it; without it `agy` picked Gemini 4 Argon (Medium) itself and never said so. Interactive `new`/`open` leave the choice to `agy` (`/model`). Needs the `google` secret.
- Typical timing of one task with a fix round (28 min): about two thirds agent work, one third sandbox setup; the `setup.install` steps were about 2m40s per sandbox (mostly `cargo install just`, ~95 s) until decision 183 dropped `just`, PowerShell and Pester from the profile, which leaves about 57 s (apt 44 s, rustup 13 s; measured 2026-10-11 on a review sandbox and again in spike S11, where the whole `sbxm new` took 68 s); the reviewer's sandbox is set up again each round.

## Working rules the user gave beyond `AGENTS.md`

The always-on rules (every change through a branch and a PR, never push to `main`, `[current] of [total]` progress, up to 3 parallel sessions) are in `AGENTS.md`, "Workflow" and "Working rules". Also:
- Keep messages short; one decision at a time with a recommendation; say unprompted when things get complex or a request looks over- or underspecified.
- Don't pre-harden: log a gap with its trigger and fix it when hit.
- Severity: the user may downgrade edge-case must-fix findings to should-fix and log them as issues instead of running more review rounds.
- Forked subagents: tell them to stop after their deliverable and not start follow-ups; verify their claims against GitHub.
- Security tooling details (antivirus, TLS interception) stay local, never in repo docs, PRs or issues.
