# PRD: milestone 2b, `sbxm task`

Status: draft for the PRD/spec experiment [140]. Written 2026-10-01 after interview rounds 1-6 (decisions 139-156).
The spec (`spec-m2b.md`) holds the exact behavior; this file says what and why. Decisions are cited by number.

## Problem

The workflow that carries a GitHub issue to a reviewed pull request lives in `scripts/issue-workers.ps1` (453 lines of
PowerShell plus a second script for filing review findings). It works, but it is frozen to bug fixes [88]: it is
Claude-only for workers, tied to `main` and to the `-claude` sandbox name, hard-wired to cargo gates, has no durable
record of a task, and can't be tested with sbxm's fakes. Every improvement we want has to wait for it to move into
sbxm [88][92], and the next goal, a software factory that covers each SDLC stage [139], needs a place to attach each
stage to a task.

## Users

- **The maintainer** (a developer on Windows with Docker Sandboxes) who hands issues to agents, reads their results,
  and merges.
- **Later (M3, not this milestone):** another project that consumes sbxm through its CLI [119][120 of the M3 draft].
  M2b avoids Rust- and sbxm-specific assumptions where it is cheap.

## Goals

1. **Parity.** Every action of the script has an `sbxm task` equivalent: pick and start issue workers, show status,
   gates plus an independent review with one fix round, review of any open PR, hand-off as a PR, cleanup [139].
2. **Three recorded wants** (PR #51): worker and reviewer harness choice (claude, codex, antigravity); a base-branch
   option; a gates-only check [139][143].
3. **A durable task record** (`task.json`) that every lifecycle stage can attach to, with a stage field and a hooks
   section for stages not built yet [139][146].
4. **Findings to issues**, folded in from `file-review-issues.ps1`: dry run by default, `--create` to publish
   [134][149].
5. **Onboarding a repo** is two commands and an edit: `task init` writes `sbxm-task.toml`; `task gates --dry-run`
   shows what would run [155].
6. **Deterministic gates:** the project's gates run on the host as a pipeline step, independent of what an agent
   runs, and are configurable per project [150][151][152].
7. **Testable without a network:** sandboxes behind `SandboxBackend`, GitHub behind `GitHubBackend` (via the `gh`
   CLI), both with fakes [142].
8. **Safe defaults kept:** workers have no GitHub access, the reviewer is a different harness than the worker, sbxm
   never merges [141][148].

## Non-goals (M2b)

- CI triggers [83], automatic filing of findings, automatic merging, GitHub access for sandboxes (S8 leaves M2b)
  [141][149].
- Batching several tasks into one sandbox and pre-built sandbox templates (templates first, later) [147].
- Planning, release and operate-and-learn stages: hooks only, nothing built [139].
- Tasks without a GitHub issue or PR, automatic resume of an interrupted task, an automatic fix round on gate
  failure, a catalogue of per-language gates [146][151][152][153].
- Cosine and Jev evaluators (deferred out of M2a) [138].

## Commands (summary; flags and output in the spec)

`sbxm task init`, `start`, `status`, `review` (`--issue` or `--pr`), `gates` (`--dry-run`), `file-findings`,
`finish`, `rm` [143]. Candidate, to confirm: `task run`, a thin composition of start and review that stops before
`finish`, which needs a human.

## Success criteria

| # | Criterion | How it is checked |
|---|---|---|
| S1 | Each script action has an `sbxm task` equivalent, listed in a parity table in the spec | Table reviewed; each row has a test or a manual check |
| S2 | A repo with only `sbxm-task.toml` and a profile can go init, start, review, finish on `FakeBackend` and `FakeGitHub` | Automated tests, no network |
| S3 | Same flow on real `sbx` and real GitHub for a scratch issue, with a Claude worker and a Codex reviewer, then a Codex worker and a Claude reviewer | Manual end-to-end checklist (like slice 13) |
| S4 | Antigravity works as worker and as reviewer | End-to-end checklist |
| S5 | A non-Rust repo with no gates list is refused with a clear error; with a list, its commands run on the host before the reviewer | Automated tests |
| S6 | A task killed mid-run is shown as interrupted; `start` refuses without `--restart` | Automated tests with a fake clock and pid probe |
| S7 | Selection rules produce the same picks and skip reasons as the script for a fixture of real issues | Golden test against captured `gh issue list` output |
| S8 | `task.json` carries `stage` and `hooks`, and the sandbox name is a field, not derived | Schema test in the spec |
| S9 | The old script is marked deprecated once S1-S4 pass; deleting it is a separate decision | Decision recorded at the end of M2b |

## Constraints (from existing decisions)

Timeouts are enforced inside the sandbox; the base dir is off `C:`; config files reject unknown keys; destructive
commands (`rm`) confirm; sandboxes never get GitHub access; secrets stay in `sbx`; every change goes through a PR and
an independent review before the user merges [79][86]; the old script stays frozen until retired [88].

## Risks

- **Scope creep toward a full factory.** Mitigation: [139]'s hooks-only rule and this non-goals list; warn when a
  slice grows.
- **Behavior drift from the script.** Mitigation: S7 golden tests, the parity table, and the script staying frozen.
- **`gh` output changes.** Mitigation: parsers tested against captured output, like `sbx`'s [142].
- **Setup cost (~2.5 min per `sbxm-dev` sandbox).** Accepted for M2b; templates are the planned fix [147].

## Open items to confirm

1. Include `task run` (start then review, stopping before `finish`)?
2. The task id for a PR review (`pr-<n>`) when the same PR also has an issue task: separate tasks, or linked?
