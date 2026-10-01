# Working on issues in sbxm sandboxes

`scripts/issue-workers.ps1` hands open GitHub issues to parallel Claude Code agents, one sbxm sandbox per issue
(decisions 80–82). The host picks the issues, so no two workers get the same one. It skips questions, issues whose
**Depends on** issues are still open, and issues **Related** to one that already has a worker.

Each worker is a clone at `<base_dir>\sbxm-issue-<n>` on branch `issue-<n>`. The sandbox has no GitHub access: the
agent reads the issue from `.sbxm-issue/issue.md`, commits locally, and writes `.sbxm-issue/result.md` with evidence
for each acceptance criterion. You push and open the PR from the host.

## The whole workflow

1. **Edit code** on a branch in the host session, following `sdlc-implementation`: one slice, test first, a commit
   after every green step (fmt, clippy, tests).
2. **Review** at the end of a slice or milestone with `sdlc-code-review`. The findings, with evidence, go to `reviews/`.
   Then open a PR and run `./scripts/issue-workers.ps1 review -Pr <n>`: the independent reviewer comments on the PR
   (decisions 86, 87). Fix what it finds on the branch and run it again, then merge.
3. **Open issues:** each finding becomes a GitHub issue labeled `must-fix`, `should-fix` or `question`, with acceptance
   criteria and **Depends on** / **Related** links (decision 79).
4. **Answer questions:** workers skip `question` issues until you've answered them.
5. **Work the issues:** `start` gives each picked issue its own clone and sandbox; the agent works test first, commits
   locally and writes `result.md`. Watch with `status`.
6. **Review the work:** `review` runs the host checks, then an independent reviewer (Codex by default) writes `review.md`; must-fix
   findings get one fix round and a second review (decision 84).
7. **Read** `result.md` and `review.md`. Small should-fix items can be fixed by hand on the branch.
8. **Open the PR:** `finish` pushes the branch and opens a PR with `Fixes #<n>`, the result and the review.
9. **Merge:** you merge, which closes the issue. Issues that depended on it can now be picked in step 5.
10. **Clean up:** `remove` deletes the sandbox and the clone.

## One-time setup

1. Copy the profile into your sbxm profiles folder (`profiles_dir` in `config.toml`); rerun this after the profile
   changes:

   ```
   just deploy-profiles
   ```

   Expected: `<profiles_dir>\sbxm-dev\profile.toml` exists. It installs Rust 1.93.0, a C toolchain, PowerShell 7.6.6,
   Pester 5.5.0 and `just` 1.58.0, allows crates.io, Microsoft's package host and the PowerShell Gallery, and needs the `anthropic` secret (`sbx secret ls` shows it). The Codex reviewer also needs the
   `openai` secret; `review` checks for it before running anything.

2. Check `gh` is logged in: `gh auth status`.

## Per batch (PowerShell 7, from the repo root)

1. See which issues would be picked: `./scripts/issue-workers.ps1 start -Workers 2 -DryRun`
2. Start them: `./scripts/issue-workers.ps1 start -Workers 2` (or pick them: `-Issue 1,2`).
   Each sandbox takes about a minute to create; the agents then run in the background, up to `-TimeLimit` (2h).
3. Check progress: `./scripts/issue-workers.ps1 status`
   Expected per issue: `#1: agent running|finished, <k> commit(s), result.md written|no result.md, reviewed|not reviewed`.
   The agent's output is in `<base_dir>\sbxm-issue-<n>\.sbxm-issue\agent.log`.
4. When the agent has finished, review it (decision 84): `./scripts/issue-workers.ps1 review -Issue 1`
   Expected: `cargo fmt --check`, `cargo clippy` and `cargo test` lines, `review round 1`, then
   `#1: <k> must-fix finding(s)`; with k > 0 a `fix round` and `review round 2`; last line `#1: review done; …`.
   A failing host check stops it with the log path (`.sbxm-issue\gates.log`). The reviewer sandbox is removed at the end.
5. Read `result.md`, `review.md` and the commits. To take over interactively: `sbxm open sbxm-issue-<n> --harness claude`.
6. Push and open the PR (its body is `Fixes #<n>`, `result.md` and `review.md`): `./scripts/issue-workers.ps1 finish -Issue 1`
7. After merging: `./scripts/issue-workers.ps1 remove -Issue 1` (asks you to confirm the paths it deletes).

`-BaseDir` (default `E:\sbxm-projects`) must match `base_dir` in sbxm's `config.toml`.
