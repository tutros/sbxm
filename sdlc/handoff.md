# Handoff: where the project stands and how to continue (2026-10-03)

For a fresh session, local or in the cloud. Read `AGENTS.md` first (it is loaded automatically), then this file, then the
files it points to. This file carries what the maintainer's local assistant memory knows that the repo did not. It has
no personal paths or secrets on purpose.

## Where things stand

- **Milestone 1 (sandboxes), M2a (comparisons) and M2b (`sbxm task`) are merged.** Cosine similarity (issue #63, PR #69)
  is merged; decision 166 (it loads its model from local files) is merged but **not yet confirmed by the maintainer**.
- **Jev (TypeSafe's classifier) is not built.** Spike S12 (2026-10-03) checked its API against a real token and found
  the docs hold, with five corrections. Status and the path: `sdlc/jev.md`, `sdlc/spikes/S12.md`, the corrected issue draft
  `sdlc/specs/jev-evaluator-14a.md` (unfiled; its proposed decision is numbered 168 because 167 is taken).
- **Direction (the maintainer, 2026-10-03):** a multi-plane software factory. The model router and an evaluation
  service live **outside** this app and are called with a task and criteria; several projects at once; cloud when local
  resources fall short. Drafts: `sdlc/specs/jev-questions.md` (which questions to ask Jev, what "best" and "accepted"
  mean, how cost is tracked) and `sdlc/specs/factory-architecture.md` (planes, contracts, what the base needs). **Both are
  on open PRs (#76, #77) when this was written: if they are not on `main` yet, they are on the branches
  `docs-jev-questions` and `docs-factory-architecture`.** They are proposals, not decisions.
- **Principle: optimize just in time.** Log a gap with its evidence and a trigger; do not build for problems nobody is
  hitting. The gap log is `sdlc/evals-workflow-notes.md` (G1 to G27, a run log, and the next-session agenda).

## What to do next (in this order)

1. Merge the open docs PRs (the maintainer does this).
2. **Interview (planning skill) on continuing work on an open PR with `sbxm task` (G16, G17, G25).** It is the one gap that
   blocks the loop. Output: a numbered decision, then issues, then work through the workflow.
3. **Scratch test of the Jev question sets on issues #58 to #61** (a throwaway script like spike S12, no sbxm code): see what
   the answers look like on real tasks before any contract is written.
4. Interview on templates and a strict, no-update sandbox mode (spike S11, PR #67, is unfinished).
5. The maintainer's answers to the open questions at the end of `sdlc/specs/jev-questions.md` (the tolerance for "best
   value", the cost unit, benchmark inputs) and `sdlc/specs/factory-architecture.md` (decision 36 for services, where
   services run, the cloud target).

## Rules the maintainer has set (follow them)

- Every change, docs included, goes to a branch and a pull request; the independent reviewer runs on the PR before the
  maintainer merges; never push to `main`.
- Show progress as "[current] of [total]" whenever the total is known; say when work can run in parallel (at most 3
  sessions in total); keep messages short, one decision at a time; say unprompted when things get too complex.
- **Tokens and secrets:** a secret stays out of the conversation. For the Jev token the maintainer gives only the path of
  a helper that prints it; it is passed to `sbx secret set-custom --command`, never `--value`; nobody reads the token file.
- **Know the binary.** Do not run a long job with an executable whose build you cannot name. Build a clean commit with
  `cargo build --release --locked`, record the commit, compiler and SHA-256 next to it, and start every log with them.
- **Disk.** Keep over 10 GB free before starting a sandbox; one sandbox at a time unless over 25 GB is free; never delete on
  the system drive without asking. Where sandbox disks live (they cannot be moved yet; a junction failed for the `claude` agent): `docs/workflow.md`, "Practical notes".
- Deploy profiles only from an up-to-date `main` (`just deploy-profiles` replaces whole profiles).
- A gate that exits 101 with no failing test is probably a full disk, not a code failure.
- Review findings are GitHub issues; fix them test first. Workers must not install software themselves: put what a build
  needs in the profile's `setup.install`.

## Continuing in a cloud session (Claude Code on the web)

What a cloud session has: the repo cloned from GitHub (so only what is pushed), the instruction files, and nothing else.
It has no local drives, no Docker Sandboxes (`sbx`), no stored `sbx` secrets and no assistant memory. So it can do
repo-only work (docs, specs, code and tests that do not need `sbx`, pull requests) but it **cannot run `sbxm` against
sandboxes**; that stays local. The maintainer decided (2026-10-03) not to subscribe to Docker's Agentic Platform for now, so `sbx`
cloud sandboxes and spike S13 are shelved. If that changes, S13 would check kits, egress rules, secrets, getting files in and out, and the cloud sandbox's
expiry against the 2-hour worker limit). Start one from the terminal with `claude --cloud "<task>"` on a
pushed branch, or from the Desktop app's "Continue in" menu. Details:
https://code.claude.com/docs/en/claude-code-on-the-web

## Where things are

`AGENTS.md` (code layout and rules) · `README.md` (the command reference) · `docs/workflow.md` (how the commands fit
together) · `sdlc/decisions.md` (numbered decisions; read before designing) · `sdlc/evals-workflow-notes.md` (gaps, run log,
agenda) · `sdlc/jev.md`, `sdlc/spikes/`, `sdlc/specs/` (Jev, spikes, drafts) · `sdlc/reviews/` (review records).
