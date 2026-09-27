# Spike S7 results

Spec: `spikes/S7.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date:
- `sbx` version:
- Pinned `sbx-kits-contrib` SHA:
- Budget used: tool calls / minutes / model calls / web fetches / peak sandboxes (limits: 90 / 75 / 1 / 2 / 2)
- `kit.allowedSources` at start (command + output):
- `sbx secret ls` at start:
- `sbx skills ls --json` at start:
- `C:` free space before and after:
- Pi version in the image:

## Q1. What is the Pi kit, at a pinned commit?

**Status:**

- `kind` / `name` / agent name for `requires.agent`:
- Network allow list:
- Env and expected secret services:
- Install and startup steps (quoted, with user):
- Files written to home:

`spec.yaml` (verbatim):

```yaml
```

## Q2. The exact `kit.allowedSources` refusal

**Status:**

- Command, exit code:
- Output (verbatim):
- Anything created (`sbx ls --json`, directories):
- Matching rule and its source:

## Q3. Does an sbxm-style mixin compose with the Pi kit?

**Status:**

- `sbx kit validate --json` result:
- `sbx create` exit code and output (verbatim, trimmed only of progress bars):
- `sbx ls --json` entry:

## Q4. Pi's always-loaded instructions file

**Status:**

| Check | Result | Evidence |
|---|---|---|
| Path (with source) | | |
| Canary present after create | | |
| Canary present after restart | | |
| QUINCE-7 in rendered prompt or model answer | | |

## Q5. Pi skills dir, skills store and config files a mixin must not ship

**Status:**

- Skills dirs (with source):
- Store mount announced / actually mounted:

| Config file | After create | After restart | Evidence |
|---|---|---|---|
| | kept / replaced / merged | | |

## Q6. Headless command and provider (record only)

**Status:**

- Headless flag:
- Model selection:
- Usage/cost output:
- Provider domains in the kit:

## Blocked items and errors

## Follow-ups

## Cleanup

- `sbx ls --json` (no `spike-s7-*`):
- `kit.allowedSources`, `sbx secret ls`, `sbx skills ls --json` unchanged:
- `E:\sbxm-it\spike-s7` gone:
