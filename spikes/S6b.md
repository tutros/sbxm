# Spike S6b: Claude hooks route, non-empty skills store, Codex/Gemini mixin home files

**Status:** approved by the user 2026-09-25, including the temporary skills-store canary (Q1) and leaving Pi out.
Run in a git worktree. **Output:** fill in `spikes/S6b-results.md`
(template provided). Nothing else.

## Why

Slice 14 (mandatory instructions via the `harness-claude` mixin) and slice 15 (`harness.claude.home_files`, hooks)
need three facts S6 left open (`spikes/S6-results.md`, "Follow-ups"):
1. The Claude kit **replaces** a mixin's `~/.claude/settings.json` (S6 Q2), so hooks need another route.
2. S6 only tested an **empty** skills store, where `--skills readonly` mounts nothing. Whether a non-empty store is
   mounted over `~/.claude/skills` and hides mixin skills is unknown (decision 46 depends on it).
3. S6 wrote Codex/Gemini files with `sbx exec`, not through a mixin. Whether a mixin's `files/home/` instructions and
   config files survive the Codex and Gemini kits is unknown (slices 18–19).

Read first: `spikes/S6.md`, `spikes/S6-results.md`, and decisions 37, 46, 49, 54, 56 in `decisions.md`.

**Not in scope: Pi.** Its kit needs `kit.allowedSources` changed (decision 48), and nothing before slice 20 needs it.
It moves to a later spike.

## Environment facts (verified 2026-09-25 unless marked)

- `sbx` v0.43.0 on Windows 10, logged in. `sbx secret ls --json` holds only a global `anthropic` service secret, so
  only Claude can make model calls.
- **Workspaces on `C:` fail** with `ERROR: failed to run sandbox container` (decision 56). Use only
  `E:\sbxm-it\spike-s6b\` for workspaces and kits.
- The skills store was empty on 2026-09-24 (S6). Check again before Q1 (see Q1).
- Claude, Codex and Gemini CLI images are already pulled (S6). Docker data lives on `C:` (about 11 GB free in S6).
  `docker` is not on PATH.
- The Bash tool collapses every `\\` in a command to `\`, and a project hook blocks such commands. Write files that
  contain backslashes with the file-write tool, or use PowerShell. Bash calls that `cd` outside the worktree are
  refused; use absolute paths.
- Mixin format: `spec.yaml` with `schemaVersion: "2"`, `kind: mixin`, `name`, optional `requires.agent: <agent>`, plus
  a `files/home/` tree copied into the agent's home. Validate with `sbx kit validate --json <dir>`, pass with
  `sbx create --kit <dir>`. Spec (pinned): `https://github.com/docker/sbx-kits-contrib/blob/d058fedc156325f87612d9bcd9bd313ab74ba100/spec/SPEC-v2.md`.
- S6's Claude kit `settings.json` content (for comparison): `themeId`, `alwaysThinkingEnabled`, `apiKeyHelper:
  "echo proxy-managed"`, `permissions.defaultMode: bypassPermissions`, `bypassPermissionsModeAccepted`,
  `skipDangerousModePermissionPrompt`.
- Hypotheses (unverified): Claude Code reads managed settings from `/etc/claude-code/managed-settings.json` on Linux,
  with hooks allowed there; mixin `setup.install` commands run after the agent kit's install commands.

## Questions

Each question ends with exactly one status: **Answered** (evidence proves it), **Partial** (some evidence, gap stated),
or **Blocked** (reason stated). Evidence means command + relevant output, or a quote + its source (file path in the
image, or URL). A belief without evidence is not an answer.

### Q1. Does a non-empty skills store hide mixin skills?
This is the only question allowed to change shared state: **one** canary skill named `spike-s6b-store-canary` is added
to the `sbx` skills store and removed again.
- Precondition: `sbx skills ls --json` shows an empty store. If it doesn't, don't touch the store: Blocked
  ("store not empty at start").
- Method:
  1. Read `sbx skills --help` and its subcommands' `--help` to find how to add a skill from a local directory and how
     to remove it. If no remove command exists: Blocked, before adding anything.
  2. Write the canary skill to `E:\sbxm-it\spike-s6b\store-canary\SKILL.md` (frontmatter `name:
     spike-s6b-store-canary`, a one-line description, body `SPIKE-S6B STORE CANARY: the word is LIME-4`), and add it
     to the store. Confirm with `sbx skills ls --json`.
  3. Create `spike-s6b-claude` (`claude`, default `--skills`, `--kit` the Q2 mixin, which also places
     `files/home/.claude/skills/spike-s6b-mixin-canary/SKILL.md`, body `SPIKE-S6B MIXIN CANARY: the word is
     PEAR-8`). In it: `findmnt -T /home/agent/.claude/skills`, `ls -la /home/agent/.claude/skills/`, and `cat` both
     SKILL.md files.
  4. Create `spike-s6b-codex` (`codex`, default `--skills`, `--kit` the Q3 codex mixin): `findmnt -T` on
     `/home/agent/.codex/skills` and `/home/agent/.agents/skills`, and `ls -la` of both.
  5. **Remove the canary from the store right after step 4** (don't wait for cleanup) and confirm with
     `sbx skills ls --json`.
- Answered when: for Claude and Codex, you have whether the store is mounted (where), and whether the mixin canary
  is still readable.

### Q2. A working route for Claude hooks
Find at least one route by which a mixin's hook reaches Claude Code despite the kit replacing `settings.json`.
Each hook writes a marker file, so no tool use is needed.
- Candidates, all in **one** mixin (`E:\sbxm-it\spike-s6b\kits\claude\`, `requires.agent: claude`), each with its own
  marker:
  - **A. Managed settings:** a file at `/etc/claude-code/managed-settings.json` with a `SessionStart` hook running
    `touch /tmp/s6b-route-a`. First check in SPEC-v2 whether a mixin can place files outside home. If it can't, try a
    `setup.install` step (user `0`) that writes the file.
  - **B. Merge in `setup.install`:** a `setup.install` step (user `agent`) that merges a `SessionStart` hook running
    `touch /tmp/s6b-route-b` into `~/.claude/settings.json` with `jq` (or `node`/`python3` if `jq` is missing) and
    leaves the kit's keys in place.
  - **C. Claude home file:** `files/home/.claude/settings.local.json` with a `SessionStart` hook running
    `touch /tmp/s6b-route-c`. Also record whether the kit replaces or removes it.
- Method:
  1. Validate the mixin, create `spike-s6b-claude` (same sandbox as Q1). Record the install order printed by
     `sbx create` (which `kit=` each install step belongs to).
  2. Right after create, `cat` `~/.claude/settings.json`, `~/.claude/settings.local.json` and
     `/etc/claude-code/managed-settings.json`.
  3. Model call: `sbx exec spike-s6b-claude claude -p "Reply with OK only." --max-turns 1 --max-budget-usd 0.10`,
     then `ls -la /tmp/s6b-route-*`.
  4. `sbx stop`, then `sbx exec … ls -la /tmp/s6b-route-* ; cat` the three files again (does a restart undo a route?),
     `rm -f /tmp/s6b-route-*`, and repeat the model call once to see which markers come back.
  5. For route A only: can the `agent` user modify or delete the managed settings file (`ls -l`, `sudo -n true`)?
     Record it. Don't try to escalate beyond `sudo -n true`.
- Answered when: for each route, you have evidence of whether its marker appeared after the first call and after the
  restarted call. If a route is impossible (e.g. the mixin can't place the file), say why.

### Q3. Do the Codex and Gemini kits keep a mixin's home files?
- Method: two mixins, `kits\codex\` (`requires.agent: codex`) and `kits\gemini\` (`requires.agent: gemini`):
  - Codex: `files/home/.codex/AGENTS.md` (`SPIKE-S6B CANARY: the word is PLUM-9`) and `files/home/.codex/config.toml`
    containing only `# spike-s6b canary`.
  - Gemini: `files/home/.gemini/GEMINI.md` (same canary) and `files/home/.gemini/settings.json` containing only
    `{"spikeCanary": true}`.
  - Create `spike-s6b-codex` (same sandbox as Q1) and `spike-s6b-gemini`. In each, `cat` both files right after
    create, then after `sbx stop` + `sbx exec`. Also run `codex debug prompt-input "hi"` (no model call) and look for
    PLUM-9 in the rendered prompt.
- Answered when: for each file, after create and after restart: present with canary, replaced or merged (show
  content), or absent. Plus whether PLUM-9 shows in Codex's rendered prompt.

## Budget (hard limits)

- At most **100 tool calls** and **75 minutes** wall time. At most **4 model calls** in total, Claude only, each with
  `--max-turns 1 --max-budget-usd 0.10`. No Codex or Gemini model calls.
- At most **3 sandboxes** at once, all named `spike-s6b-*`, each `--cpus 2 -m 2g`.
- No new image pulls should be needed. If one is and `C:` has under **8 GB** free: skip that harness, status Blocked.
- When a limit is reached: stop, clean up, write results with what you have.

## Stop rules

- The same error twice in a row for the same step: record it (command + error) as Blocked and move on.
- Never run `sbx login`, `sbx logout`, `sbx secret set|rm`, `sbx reset`, `sbx prune`, `sbx settings set`, `sbx policy`
  changes, or any interactive command (`sbx run`, attaching to an agent TUI).
- The skills store: only adding and removing `spike-s6b-store-canary` as Q1 describes. Never `import` from a host
  harness directory, never touch other skills.
- Never modify files outside `E:\sbxm-it\spike-s6b\` and `spikes/S6b-results.md`. No changes to `src/`, `tests/`,
  `decisions.md`, `milestone-1.md`, `AGENTS.md`, `CLAUDE.md`, `.claude/`. No git commands that change state.
- Web: at most **2 fetches**, only the pinned SPEC-v2 above and Claude Code's official settings/hooks docs
  (`https://docs.anthropic.com/en/docs/claude-code/settings` or `/hooks`).
- Don't widen scope: questions not listed here go under "Follow-ups" in the results, uninvestigated.
- Don't delete Docker images, volumes or any sandbox not named `spike-s6b-*`.

## Cleanup (always, including after failure or hitting a limit)

1. If the canary is still in the skills store, remove it **first**.
2. `sbx rm -f` every sandbox whose name starts with `spike-s6b-` (check with `sbx ls --json`).
3. Remove `E:\sbxm-it\spike-s6b\`.
4. Confirm and paste into the results' Cleanup section: `sbx ls --json` has no `spike-s6b-*`; `sbx skills ls --json`
   shows an empty store (same as before the run); `sbx secret ls` (the table, not `--json`) is unchanged.

## Exit criteria

The spike is done when every question has a status with evidence or a reason, cleanup is confirmed (store empty
again), and `spikes/S6b-results.md` is complete. Final message: one line per question (`Q1: Answered, …`), budget
used, cleanup confirmed yes/no, store restored yes/no.
