# Spike S6 results

Spec: `spikes/S6.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date:
- `sbx` version:
- Budget used: _ tool calls, _ minutes, _ model calls

## Q1. Skills-store mount vs. mixin `files/home/` (Claude)

**Status:**

| Sandbox | Mount at `~/.claude/skills` | Canary SKILL.md readable |
|---|---|---|
| `spike-s6-claude-ro` (default) | | |
| `spike-s6-claude-off` (`--skills off`) | | |

Evidence:

## Q2. Does the Claude kit overwrite mixin home files?

**Status:**

| File | After create | After stop + exec |
|---|---|---|
| `~/.claude/CLAUDE.md` | | |
| `~/.claude/settings.json` | | |

Evidence:

## Q3. User-level instructions file and skills dir

**Status:**

| Harness | Instructions file | Skills dir | Store mount | Source |
|---|---|---|---|---|
| Codex | | | | |
| Gemini CLI | | | | |
| Pi | | | | |

Evidence:

## Q4. Is the instructions file loaded?

**Status:**

| Harness | Result |
|---|---|
| Claude | |
| Pi | |
| Codex | Blocked: no credentials; file location from Q3 only |
| Gemini CLI | Blocked: no credentials; file location from Q3 only |

Evidence:

## Implications

For slices 14, 15, 18–20 and profile-owned skills (decision 46). Facts only; the main session decides.

## Blocked items and errors

## Follow-ups (not investigated)

## Cleanup

- `sbx ls --json` (no `spike-s6-*`):
- `sbx skills ls --json` (store still empty):
- `sbx secret ls --json` (unchanged, masked):
- Images pulled and sizes:
