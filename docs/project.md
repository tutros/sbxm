# sbxm project context

This file is the one place for the project's goals and current state; `AGENTS.md` and the `sbxm-context` skill link here for them. The code layout, commands, constraints and the always-on working rules stay in `AGENTS.md` (loaded every session), so they are linked below, not copied. Read the files your task touches; verify anything time-bound with `gh` before trusting it.

## Goals (user-confirmed 2026-10-05)

1. **Software factory:** launch sandboxes with a profile (kits) to produce or change code from a PRD/spec, a GitHub issue or a review. The task source is pluggable. Command: `sbxm task`.
2. **Comparisons:** run 2-4 contestants (harness x model x profile) on the same task and evaluate them. Command: `sbxm run`.

Plus the lifecycle commands (`new`, `open`, `list`, `stop`, `rm`, `config`, `doctor`). What sbxm is and the constraints it follows: `AGENTS.md`, "What `sbxm` is" and "Architectural constraints".

## State (as of 2026-10-11)

- Merged: milestones 1, 2a (`sbxm run`) and 2b (`sbxm task`); decision 169 (continue work on an open PR: #102, #103, #122); `run.log` for task commands (#127, decision 170 part 1); the state machine's table and its guards (#115, PR #133), `task states` (#116, PR #136), the `fix_rounds` loop (#117, PR #137), the `Repeat of:` marker and the no-progress stop (#119, PR #145; its run was the multi-round checkpoint); the spec task source (#121, split into #146-#149: PRs #150, #151, #155, #156), so `task start --spec` and `task review --spec` work; the `local` sink (#143, PR #157, decision 178): `task finish --spec` keeps the branch in the task's `repo.git` and `task rm --spec` protects an unfetched result; the `push` sink (#144, PR #160) pushes it to `origin` as a new branch, no PR; reviewers judge the change only, with an "Outside this change" section (#152 part 1, PR #154).
- Both seen in a real run (issue 120, 2026-10-08): a gate failure feeding a fix round, and `stopped = rounds-exhausted` followed by `task resume --rounds 1`.
- The state machine is designed (decisions 173-177, `sdlc/specs/task-state-machine.md` section 5, `sdlc/spikes/state-table.md`, the table in `src/task/machine.rs`). #118 (`task resume`, `--rounds N`) is merged (PR #163, 2026-10-08); its review findings M-1 and S-1 (round-number exhaustion) are merged as #161 (PR #167) and #162 (PR #170), both 2026-10-09; their missing regression tests are merged too: #171 (PR #174) and #169 (PR #176, both 2026-10-09; #169 also made `run_reviewer` build its earlier-reviews context from the saved `review-<n>.md` files instead of trying every round number, which took hours at `u32::MAX`). #120 (`task finish` opens a draft PR when must-fix findings are left, listing them from `task.json`) is merged (PR #165, 2026-10-09), and so is #138 (CRLF test, PR #168). #123 (PR risk assessment: a `Risk:` level and reasons from the reviewer, raised to a path-rule minimum, at the top of every PR description and PR review comment; decisions 176, 179) is merged (PR #178, 2026-10-09); its deferred finding is #179 (the unknown-risk warning is lost when a PR comment post fails). #107/#108 (the worker prompt and the spec describe a continued PR's branch truthfully, a new `{{branch_context}}` prompt placeholder) are merged as one spec task (PR #181, 2026-10-10). #140 (gates failing right after the worker hand over to the review: `task start` names `task review` as the next step and `task run` goes on to it; decision 180, PR #183) is merged (PR #184, 2026-10-10); its deferred finding, #186, is fixed: a gate command that could not be executed at all is told apart from one that ran and failed (in memory only, `task.json` is unchanged), gets no fix round, and stops `task run` after the start phase. #185 (a test fixture passed a 70,000-character commit message on the command line, which Windows refuses) is fixed. #130 (a `run.log` append that succeeds after a failed one still starts with its invocation's header) is merged (PR #190, 2026-10-10). #126 is fixed by PR #191: git's "Permission denied" retry waits about 6.4 s in total instead of 0.75 s and a failure that ran out says how often it tried, and `sbxm run` exits 1 when any contestant could not be run (decision 181). #106 (review severity: a message that loses detail is should-fix, a rule added to the `sdlc-code-review` skill) is resolved by PR #192. #111 is answered and closed: spike S13 (`sdlc/spikes/S13-sandbox-flakes.md`, PR #193) ran the full suite 25 times in a Linux sandbox, twice, without a failure. That is consistent with the Windows file lock being the main cause of #104's flaky tests, but it does not confirm it: one of the three (`run_results`) had failed inside a sandbox, which these runs did not reproduce, and a flake rarer than about 1 in 8 runs is not ruled out. Remaining, in the order the user agreed on 2026-10-10 after a timing check of the saved run logs (run time and fewer manual approvals first): #194 (`task run --spec`: built, in review as PR #208, not merged yet), #209 (the profile's rustup step hides a failed download: the sandbox is created without Rust), #206 (fewer review findings: a worker self-review, fix rounds that fix the kind of problem and not one instance, no open questions sent to a worker; tried first as prompt overrides on the next three tasks), #198 (the in-task review counts as the PR review when the PR head is unchanged; changes decision 86, to be numbered when built), #90 then #88 (the dashboard's Tasks section and `task logs`, moved up because they replace the read-only checks done by hand), #195 (re-review a PR in place, gap G34), #203 (allow rules for read-only host commands; more hand work moved to sandboxes), #201 (`task run` over a list of tasks), #202 (cleanup after merge in one command) and #199 (a spike: does a fresh reviewer sandbox per round matter). Spike S11 is finished (#200, `sdlc/spikes/S11.md`): templates work, and the feature is its own milestone, starting with a planning interview (#211). #197 is done: the trimmed profile is deployed and its setup time measured. Also from S11: #210 (a free-space check of the sandbox drive, gap G8) and #212 (a drift test that fails in a checkout without a git repository). Then #207 (deferred from issue 194's review: a restart can lose the old task when an input changes during the confirmation; fix when it happens or when `--restart` next changes), #104 (waiting for one of its tests to fail again), #96-#100, #58-#60 and the rest of the logging and dashboard work (#89, #91, #92). #114 is skipped until `task logs` (#88) needs the `sbx` lines (the user's choice, 2026-10-10). #152 stays open for part 2 (a separate improvement-review mode). #153 (source capabilities instead of kind lists) is deferred until a 4th task source, or a change needed in more than one place.
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
- `task run --issue N` = start + gates + review + up to `[worker] fix_rounds` fix rounds (default 3), then stops at `ready`. `task finish` (the user decides) pushes with the user's login. `--reviewer-harness`/`--reviewer-model` override the reviewer; the config is read once at start, so changing `sbxm-task.toml` mid-run has no effect (use `--restart` to start over). An issue with a `PR: #n` line for a merged PR is refused (decision 169).
- `task rm` shows what it deletes; clean up finished tasks, worktrees and build dirs only with the user's OK.
- antigravity (`agy`): model ids come from `agy models`; set `model` explicitly (the default is believed to be Gemini 3.1 Pro but is unverified). Needs the `google` secret.
- Typical timing of one task with a fix round (28 min): about two thirds agent work, one third sandbox setup; the `setup.install` steps were about 2m40s per sandbox (mostly `cargo install just`, ~95 s) until decision 183 dropped `just`, PowerShell and Pester from the profile, which leaves about 57 s (apt 44 s, rustup 13 s; measured 2026-10-11 on a review sandbox and again in spike S11, where the whole `sbxm new` took 68 s); the reviewer's sandbox is set up again each round.

## Working rules the user gave beyond `AGENTS.md`

The always-on rules (every change through a branch and a PR, never push to `main`, `[current] of [total]` progress, up to 3 parallel sessions) are in `AGENTS.md`, "Workflow" and "Working rules". Also:
- Keep messages short; one decision at a time with a recommendation; say unprompted when things get complex or a request looks over- or underspecified.
- Don't pre-harden: log a gap with its trigger and fix it when hit.
- Severity: the user may downgrade edge-case must-fix findings to should-fix and log them as issues instead of running more review rounds.
- Forked subagents: tell them to stop after their deliverable and not start follow-ups; verify their claims against GitHub.
- Security tooling details (antivirus, TLS interception) stay local, never in repo docs, PRs or issues.
