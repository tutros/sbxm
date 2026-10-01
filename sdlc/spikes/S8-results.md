# Spike S8 results

Spec: `sdlc/spikes/S8.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date:
- Run by (human who met the precondition, or "no one: Q3–Q4 not run"):
- Scratch repo `<owner>/sbxm-spike-s8`:
- `sbx` version:
- Budget used: tool calls / minutes / web fetches / peak sandboxes (limits: 60 / 45 / 2 / 2)
- `sbx secret ls` at start:

## Q1. What does `sbx` do with a `github` service secret?

**Status:**

- Hosts the token is injected for, and as which header:
- Does storing it widen egress by itself:
- `GH_TOKEN` in the sandbox (length, prefix only):
- Credential helper for git:
- Sources:

## Q2. Which hosts do the GitHub operations need?

**Status:**

| Command | Exit code | Result (trimmed) | Hosts in `sbx policy log` |
|---|---|---|---|
| `git ls-remote origin HEAD` | | | |
| `git fetch origin` | | | |
| `gh issue view 1 -R tutros/sbxm --json title` | | | |
| `gh api repos/tutros/sbxm --jq .full_name` | | | |

Minimal `network.allow` for these:

## Q3. Do push, PR and issue edits work through the proxy?

**Status:**

| Step | Command | Exit code | Result (trimmed) |
|---|---|---|---|
| 1 | `gh api user --jq .login` | | |
| 2 | `git push -u origin spike-s8-branch` | | |
| 2b | (only if needed) `gh auth setup-git`, then push again | | |
| 3 | `gh pr create …` | | |
| 4 | `gh issue comment 1 …` / `gh issue edit 1 …` | | |
| 5 | grep for `ghp_` / `github_pat_` (match counts) | | |

- Git setup that made push work:
- Hosts used (`sbx policy log`):

## Q4. Do the limits hold?

**Status:**

| Limit | Command | Result (quoted) | Held? |
|---|---|---|---|
| Direct push to `main` refused | `git push origin HEAD:main` | | |
| Merge without review refused | `gh pr merge …` | | |
| Workflow file push refused | push `.github/workflows/spike-s8.yml` | | |
| No access to other repos | `gh repo view tutros/sbxm --json viewerPermission` | | |
| Other GitHub hosts blocked | `curl … https://uploads.github.com/` | | |

## Cleanup

- `sbx ls` (no `sbxm-spike-s8-*`):
- `sbx secret ls` (same as at start):
- `E:\sbxm-it\spike-s8` exists? (should be no):
- Left for the human: delete `<owner>/sbxm-spike-s8`, revoke the token, `sbx secret rm github`.

## Follow-ups

-
