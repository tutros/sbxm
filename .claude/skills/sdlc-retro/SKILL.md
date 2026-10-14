---
name: sdlc-retro
description: How this project looks back on its own work - pick a window (since the last retro), gather what happened with numbers (tasks, review rounds, findings, time, hand fixes, gaps), find the patterns behind the friction, check whether the last retro's experiments worked, and propose at most three process changes as experiments for the user to decide. Use when the user asks for a retro, a look back or a reflection, at the end of a milestone, or when the last retro is more than about ten merged PRs old.
---

# Retro phase

A retro looks at how the work went, not at what was built. It turns what happened since the last retro into a few
process changes, each tried as an experiment first (decision 140's way: the M2b PRD/spec experiment and the issue 206
experiment were both run like this). A retro changes nothing by itself: every change it proposes is the user's to
decide, one at a time, and an adopted one goes through the normal branch and PR.

## 1. When, and the window

- When the user asks, at the end of a milestone, or when about ten PRs have merged since the last retro (say so
  then; don't start one unasked).
- The window starts where the last retro's window ended (the `Window:` line of the newest file in `sdlc/retros/`)
  and runs to today. Say the window and how many PRs and tasks it covers before gathering anything.
- **The first retro**, with no earlier file: ask the user where its window starts, with a recommendation (the day
  after the last look back on record, such as `sdlc/issue-206-experiment-report.md`, or the last ten merged PRs),
  before gathering anything.
- Every retro file records its window on a `Window: <start> to <end>` line, so the next one can start from it.

## 2. Gather the facts (numbers, with where each came from)

Read only; nothing is written in this step. For every task and PR in the window:

| What | Where |
|------|-------|
| Merged PRs, their issues and sizes | `gh pr list --state merged --search "merged:>=<date>"`, the issues' `size:` labels |
| Review rounds and must-fix counts per round, `Repeat of:` findings, stop reasons | each task's `run.log` and `review-<n>.md` under `<base>/.sbxm/tasks/` (or `task-archive/`), PR review comments |
| Wall time per command (each `run.log` header to that command's last line) and per task (first header to `ready`) | `run.log` timestamps |
| Time each agent ran (worker, each fix round, each reviewer) | `duration_s` of each run in `task.json` |
| Sandbox setup and gate time | not recorded: write unknown; if a retro needs it, propose it as a sized code issue |
| Hand fixes after a task finished, and why | commits on the PR after `review.posted`, the session handoffs in memory |
| CI failures and their real cause (read the log, don't guess) | `gh run list`, `gh run view --log-failed` |
| Gaps logged, and gaps fixed | `sdlc/evals-workflow-notes.md` (G-numbers added or closed in the window) |
| Decisions made, and decisions changed | `sdlc/decisions.md` |
| Lessons and corrections from the user | the handoff and feedback memories in the window |

Put the numbers in one table per task: issue, size, rounds, must-fix per round, wall time, agent time, hand fixes,
CI result.
A fact you couldn't find is written as unknown, not estimated.

## 3. Find the patterns

Look for what repeats, not single events. Questions that found real problems before:

- **Review rounds:** which tasks took more than three rounds, and why? Were the round-1 must-fix findings of one
  kind (a missed rule, a platform difference, an open question the worker guessed)? Would a prompt, a skill rule
  or an answered open point have caught them?
- **Platform:** did anything pass in the Linux sandbox and fail on Windows (G46)? Does the CI catch it now?
- **Size:** did each issue's `size:` match the time and rounds it took? If not, which row of the size table was wrong?
- **Interventions:** what did the user have to do by hand (approvals, `!` commands, merges, restarts), and could
  any of it be removed safely?
- **Waiting:** where did work stand still (approvals, disk, flaky tests, a session restart) and for how long?
- **Last retro's experiments:** for each one, did its measure move? Say keep, change or drop, with the numbers.

Every pattern needs at least two instances from the window, with their task or PR numbers. One instance is an
anecdote: log it in the gaps file with a trigger instead.

## 4. Propose changes as experiments

At most three, ranked by the time or rework they would save. For each:

- **The problem**, with its instances.
- **The change**: the exact text of the prompt, skill, rule or command it touches.
- **The measure**: what number should move (rounds per task, round-1 must-fix count, minutes per task, approvals
  per task), and its value in this window.
- **The trial**: how many tasks it runs for (usually three) before it is kept or dropped.
- **The cost**, including any added process. Say so when a change adds more process than it removes
  (the project's "flag over-specifying" rule).

A change to the sbxm code itself is a GitHub issue with a size, not an experiment.

## 5. Write it down and decide

1. Write `sdlc/retros/<date>-retro.md` (create the folder for the first one): the `Window:` line, the per-task table, the patterns, last retro's experiments and
   their outcome, and the proposals. Keep security-tool details out of it (it goes to GitHub).
2. Give the user a short summary: the three proposals in one line each, nothing else.
3. Ask about each proposal on its own, with a recommendation (step 1 of N, 2 of N...).
4. Record each answer in the retro file. An adopted experiment gets a numbered decision in `sdlc/decisions.md`;
   a code change gets an issue.
5. Everything goes through a branch and a PR with the independent review, like any other docs change.

The next retro starts by reading this file's experiments (section 3, last bullet).
