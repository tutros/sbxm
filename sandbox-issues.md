# Working on issues in sbxm sandboxes

`scripts/issue-workers.ps1` hands open GitHub issues to parallel Claude Code agents, one sbxm sandbox per issue
(decisions 80–82). The host picks the issues, so no two workers get the same one. It skips questions, issues whose
**Depends on** issues are still open, and issues **Related** to one that already has a worker.

Each worker is a clone at `<base_dir>\sbxm-issue-<n>` on branch `issue-<n>`. The sandbox has no GitHub access: the
agent reads the issue from `.sbxm-issue/issue.md`, commits locally, and writes `.sbxm-issue/result.md` with evidence
for each acceptance criterion. You push and open the PR from the host.

## One-time setup

1. Copy the profile into your sbxm profiles folder (`profiles_dir` in `config.toml`):

   ```
   Copy-Item -Recurse profiles\sbxm-dev (Join-Path $HOME '.config\sbxm\profiles\')
   ```

   Expected: `<profiles_dir>\sbxm-dev\profile.toml` exists. It installs Rust 1.93.0 and a C toolchain, allows
   crates.io, and needs the `anthropic` secret (`sbx secret ls` shows it).

2. Check `gh` is logged in: `gh auth status`.

## Per batch (PowerShell 7, from the repo root)

1. See which issues would be picked: `./scripts/issue-workers.ps1 start -Workers 2 -DryRun`
2. Start them: `./scripts/issue-workers.ps1 start -Workers 2` (or pick them: `-Issue 1,2`).
   Each sandbox takes about a minute to create; the agents then run in the background, up to `-TimeLimit` (2h).
3. Check progress: `./scripts/issue-workers.ps1 status`
   Expected per issue: `#1: agent running|finished, <k> commit(s), result.md written|no result.md`.
   The agent's output is in `<base_dir>\sbxm-issue-<n>\.sbxm-issue\agent.log`.
4. Read `result.md` and the commits. To take over interactively: `sbxm open sbxm-issue-<n>`.
5. Push and open the PR (its body is `Fixes #<n>` plus `result.md`): `./scripts/issue-workers.ps1 finish -Issue 1`
6. After merging: `./scripts/issue-workers.ps1 remove -Issue 1` (asks you to confirm the paths it deletes).

`-BaseDir` (default `E:\sbxm-projects`) must match `base_dir` in sbxm's `config.toml`.
