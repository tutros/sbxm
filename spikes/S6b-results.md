# Spike S6b results

Spec: `spikes/S6b.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date:
- `sbx` version:
- Budget used: tool calls, minutes, model calls (limits: 100 / 75 / 4). Peak sandboxes at once:
- Skills store before the run (`sbx skills ls --json`):

## Q1. Does a non-empty skills store hide mixin skills?

**Status:**

| Sandbox | Store mounted at | Store canary (LIME-4) readable | Mixin canary (PEAR-8) readable |
|---|---|---|---|
| `spike-s6b-claude` (`~/.claude/skills`) | | | |
| `spike-s6b-codex` (`~/.codex/skills`) | | | n/a |
| `spike-s6b-codex` (`~/.agents/skills`) | | | n/a |

Commands used to add and remove the canary:

Evidence:

## Q2. A working route for Claude hooks

**Status:**

| Route | File in place after create | Marker after 1st call | Survives restart | Marker after restarted call | Agent can modify it |
|---|---|---|---|---|---|
| A. Managed settings | | | | | |
| B. Merge in `setup.install` | | | | | n/a |
| C. `settings.local.json` | | | | | n/a |

Install order printed by `sbx create`:

Evidence:

## Q3. Do the Codex and Gemini kits keep a mixin's home files?

**Status:**

| File | After create | After restart |
|---|---|---|
| `~/.codex/AGENTS.md` | | |
| `~/.codex/config.toml` | | |
| `~/.gemini/GEMINI.md` | | |
| `~/.gemini/settings.json` | | |

PLUM-9 in `codex debug prompt-input`:

Evidence:

## Implications

For slices 14, 15, 18–19 and decision 46. Facts only; the main session decides.

## Blocked items and errors

## Follow-ups (not investigated)

## Cleanup

- Canary removed from the store (command + output):
- `sbx ls --json`:
- `sbx skills ls --json`:
- `sbx secret ls` (table):
