# Spike S6 results

Spec: `sdlc/spikes/S6.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date: 2026-09-24 (19:41 to 19:53 local)
- `sbx` version: `sbx version: v0.43.0 79805a6e3c6667520dc2da4f6bdeddae9b700969`
- Budget used: 43 tool calls, 12 minutes, 1 model call (limits: 80 / 60 / 3). Peak sandboxes at once: 2.

## Q1. Skills-store mount vs. mixin `files/home/` (Claude)

**Status:** Answered (for an **empty** store, which is the only state this spike was allowed to test; see gap below)

| Sandbox | Mount at `~/.claude/skills` | Canary SKILL.md readable |
|---|---|---|
| `spike-s6-claude-ro` (default) | **No mount.** `findmnt -T` resolves to the root overlay; nothing in `/proc/self/mountinfo` mentions skills | Yes, full content |
| `spike-s6-claude-off` (`--skills off`) | No mount (identical mount table to `-ro`) | Yes, full content |

Mixin used (valid per `sbx kit validate --json`: `"valid": true, "warnings": []`): `kind: mixin`, `name: spike-s6-canary`,
`requires.agent: claude`, plus `files/home/.claude/skills/spike-canary/SKILL.md`, `files/home/.claude/CLAUDE.md` and
`files/home/.claude/settings.json`. Create: `sbx create --name spike-s6-claude-ro --cpus 2 -m 2g --kit E:\sbxm-it\spike-s6\kits\canary claude E:\sbxm-it\spike-s6\ws-claude-ro`
(and the same with `--skills off` for `spike-s6-claude-off`).

Evidence (both sandboxes printed the same, apart from the workspace path):

```
== findmnt skills          (findmnt -T /home/agent/.claude/skills -o TARGET,SOURCE,FSTYPE)
TARGET SOURCE  FSTYPE
/      overlay overlay
== mountinfo lines mentioning home/skill/agent
117 106 254:64 / /home/agent/.claude/projects rw,relatime - ext4 /dev/vde rw
118 106 254:80 / /home/agent/.claude/sessions rw,relatime - ext4 /dev/vdf rw
119 106 254:96 / /home/agent/.claude/todos rw,relatime - ext4 /dev/vdg rw
120 106 254:112 / /home/agent/.claude/shell-snapshots rw,relatime - ext4 /dev/vdh rw
121 106 254:128 / /home/agent/.claude/statsig rw,relatime - ext4 /dev/vdi rw
== all non-overlay/proc/sys mounts
/ overlay overlay
/run tmpfs tmpfs
/run/secrets tmpfs tmpfs
/var/lib/docker /dev/vdd ext4
/home/agent/.claude/projects /dev/vde ext4
... sessions, todos, shell-snapshots, statsig (ext4) ...
/etc/resolv.conf bind-...[/resolv.conf] virtiofs
/etc/hosts bind-...[/hosts] virtiofs
/e/sbxm-it/spike-s6/ws-claude-ro host[/e/sbxm-it/spike-s6/ws-claude-ro] virtiofs
== ls skills
/home/agent/.claude/skills/:
drwxr-xr-x 3 agent agent 4096 Sep 25 02:42 .
drwxr-xr-x 2 agent agent 4096 Sep 25 02:42 spike-canary
/home/agent/.claude/skills/spike-canary:
-rw-r--r-- 1 agent agent  156 Sep 25 02:42 SKILL.md
== cat SKILL
---
name: spike-canary
description: Spike S6 canary skill. Use when asked about the spike canary skill.
---

SPIKE-S6 SKILL CANARY: the skill word is FIG-3
```

**Gap:** the store was empty (`sbx skills ls --json` → `"skills": []`), and the spec forbids `sbx skills add|import`.
With an empty store, `sbx` 0.43 mounts **nothing** at `~/.claude/skills` in `readonly` mode, so the mixin's file
wins. Whether a *non-empty* store is mounted over `~/.claude/skills` (and would then hide the mixin's skills) is
**not tested**.

## Q2. Does the Claude kit overwrite mixin home files?

**Status:** Answered

| File | After create | After stop + exec |
|---|---|---|
| `~/.claude/CLAUDE.md` | Present with canary (`SPIKE-S6 CANARY: the canary word is PLUM-7`, 43 bytes) | Present with canary, unchanged (same mtime) |
| `~/.claude/settings.json` | **Replaced** by the Claude kit's settings (no `spikeCanary` key, not merged) | Still the kit's version, no canary |

Evidence (`spike-s6-claude-ro`, right after create):

```
== CLAUDE.md
-rw-r--r-- 1 agent agent 43 Sep 25 02:42 /home/agent/.claude/CLAUDE.md
SPIKE-S6 CANARY: the canary word is PLUM-7
== settings.json
-rw-r--r-- 1 agent agent 235 Sep 25 02:42 /home/agent/.claude/settings.json
{
  "themeId": 1,
  "alwaysThinkingEnabled": true,
  "apiKeyHelper": "echo proxy-managed",
  "permissions": { "defaultMode": "bypassPermissions" },
  "bypassPermissionsModeAccepted": true,
  "skipDangerousModePermissionPrompt": true
}
```

After `sbx stop spike-s6-claude-ro` (`Sandbox 'spike-s6-claude-ro' stopped; state preserved.`, `sbx ls` status
`stopped`), then `sbx exec spike-s6-claude-ro sh -c '…'`:

```
== CLAUDE.md
-rw-r--r-- 1 agent agent 43 Sep 25 02:42 /home/agent/.claude/CLAUDE.md
SPIKE-S6 CANARY: the canary word is PLUM-7
== settings.json
-rw-r--r-- 1 agent agent 235 Sep 25 02:42 /home/agent/.claude/settings.json
{ …identical kit content as above, no "spikeCanary"… }
== skill
SPIKE-S6 SKILL CANARY: the skill word is FIG-3
== uptime
1.66 1.52          (container really restarted)
```

Likely writer (not proven): the Claude kit's install step shown during create,
`$ set -e mkdir -p /home/agent/.claude HELPER='' if [ "${SBX_C… (kit=claude, user=agent)`. Neither
`/var/log/sbx-kit-startup.log` nor `/etc`, `/opt`, `/usr/local/bin` contained a match for `settings`/`apiKeyHelper`.
Either way, the observable result is that a mixin's `settings.json` is dropped, while `CLAUDE.md` and the skills tree survive.

## Q3. User-level instructions file and skills dir

**Status:** Partial (Codex and Gemini CLI Answered; Pi Blocked)

| Harness | Instructions file | Skills dir | Store mount | Source |
|---|---|---|---|---|
| Codex (codex-cli 0.149.1) | `~/.codex/AGENTS.md` (`$CODEX_HOME/AGENTS.md`; also `AGENTS.override.md` exists) — **confirmed loaded into the prompt** | **Both** `~/.codex/skills/` and `~/.agents/skills/` (both listed in the prompt) | None with an empty store; `sbx` pre-creates an empty `~/.agents/` dir | Strings in the installed binary + `codex debug prompt-input` (local render, no model call) |
| Gemini CLI (0.60.0) | `~/.gemini/GEMINI.md` | **Both** `~/.gemini/skills/` and `~/.agents/skills/` (`.agents` wins on name clash) | None with an empty store; no `~/.agents/` dir pre-created | Docs shipped in the package + `gemini skills list` |
| Pi | Blocked | Blocked | Blocked | Kit could not be installed (see Blocked items) |

Evidence, Codex (`spike-s6-codex`, `sbx create --name spike-s6-codex --cpus 2 -m 2g codex E:\sbxm-it\spike-s6\ws-codex`):

- `which codex` → `/usr/local/share/npm-global/bin/codex`; native binary at
  `/usr/local/share/npm-global/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex`.
- `ls -la ~`: `.agents` (empty dir, created at sandbox create time), `.codex` (`config.toml` from the kit: `approval_policy = "never"`, `sandbox_mode = "danger-full-access"`, mcp-gateway), no `.gemini`/`.pi`.
- Binary strings (`grep -ao`):
  - `codex-home/src/instructions/mod.rs … Failed to read global AGENTS.md instructions from`
  - `AGENTS.override.mdAGENTS.md … core/src/agents_md.rs`
  - `create discoverable skills in `$CODEX_HOME/skills`, or `~/.codex/skills` when `CODEX_HOME` is unset.`
  - `Installs into `$CODEX_HOME/skills/<skill-name>` (defaults to `~/.codex/skills`).`
  - `return os.environ.get("CODEX_HOME", os.path.expanduser("~/.codex"))`
- Direct proof: wrote `~/.codex/AGENTS.md` (PLUM-7 canary), `~/.codex/skills/spike-canary/SKILL.md` (FIG-3) and
  `~/.agents/skills/spike-canary-agents/SKILL.md` (KIWI-9), then `codex debug prompt-input "hi"` (renders the
  model-visible prompt locally; exit 0). Output excerpts:
  ```
  "text": "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nSPIKE-S6 CANARY: the canary word is PLUM-7\n</INSTRUCTIONS>"
  - spike-canary: SPIKE-S6 codex-home skill canary FIG-3 (file: /home/agent/.codex/skills/spike-canary/SKILL.md)
  - spike-canary-agents: SPIKE-S6 dot-agents skill canary KIWI-9 (file: /home/agent/.agents/skills/spike-canary-agents/SKILL.md)
  ```
  Codex also ships built-in system skills under `~/.codex/skills/.system/` (e.g. `skill-installer`).
- `findmnt` (non-system mounts): only `/`, `/run`, `/run/secrets`, `/var/lib/docker`, resolv.conf, hosts, workspace. No skills mount.

Evidence, Gemini CLI (`spike-s6-gemini`, created the same way with `gemini`):

- `which gemini` → `…/@google/gemini-cli/bundle/gemini.js`, `gemini --version` → `0.60.0`. `~/.gemini/` holds
  `projects.json` and a kit-written `settings.json` (`tools.sandbox=false`, `folderTrust.enabled=false`, mcp-gateway).
  No `~/.agents/`.
- Bundle: `var DEFAULT_CONTEXT_FILENAME = "GEMINI.md";`, `const userProfileFile = path9.join(homedir(), GEMINI_DIR, fileName);`,
  `static getUserSkillsDir()`, `static getUserAgentSkillsDir()`, `var AGENTS_DIR_NAME = ".age…`.
- Shipped docs, `bundle/docs/cli/gemini-md.md:20`: ``- **Location:** `~/.gemini/GEMINI.md` (in your user home directory).``
- Shipped docs, `bundle/docs/cli/skills.md:44`: ``3.  **User skills**: Located in `~/.gemini/skills/` or the `~/.agents/skills/` ``;
  line 53: ``.agents/skills/` alias takes precedence over the `.gemini/skills/` directory.``
- Direct proof: planted canary skills in both dirs, then `gemini skills list` (no model call):
  ```
  Discovered Agent Skills:
  spike-canary [Enabled]
    Description: SPIKE-S6 gemini-home skill canary FIG-3
    Location:    /home/agent/.gemini/skills/spike-canary/SKILL.md
  spike-canary-agents [Enabled]
    Description: SPIKE-S6 dot-agents skill canary KIWI-9
    Location:    /home/agent/.agents/skills/spike-canary-agents/SKILL.md
  ```
- `findmnt`: same as Codex, no skills mount.
- Headless flag seen in `gemini --help`: `-p, --prompt  Run in non-interactive (headless) mode`.

Evidence, Pi: see Blocked items. The 2 allowed web fetches (official repo) were used and were inconclusive:
`raw.githubusercontent.com/badlogic/pi-mono/main/packages/coding-agent/README.md` and
`raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/README.md` are both the same 70-line stub
README (the project moved to `github.com/earendil-works/pi`, npm `@earendil-works/pi-coding-agent`, site `pi.dev`), with
no mention of `AGENTS.md` paths, skills dirs or `~/.pi`. Line 17 only says Pi supports "prompt templates, skills,
extensions, and themes" and "print, JSON, or RPC mode".

## Q4. Is the instructions file loaded?

**Status:** Partial (Claude Answered; Pi Blocked; Codex/Gemini Blocked by spec)

| Harness | Result |
|---|---|
| Claude | **Loaded.** Reply `PLUM-7` |
| Pi | Blocked: Pi sandbox could not be created (kit source not allowed, see Blocked items) |
| Codex | Blocked: no credentials; file location from Q3 only (note: Q3's local `codex debug prompt-input` render shows `~/.codex/AGENTS.md` content in the model-visible prompt, without a model call) |
| Gemini CLI | Blocked: no credentials; file location from Q3 only |

Evidence (model call 1 of 3, the only one made):

```
PS> sbx exec spike-s6-claude-ro claude -p "What is the canary word? Reply with the word only." --max-turns 1 --max-budget-usd 0.10
PLUM-7
exit=0
```

This ran after the stop/restart in Q2, so the mixin's `~/.claude/CLAUDE.md` is loaded on a restarted sandbox too.

## Implications

For slices 14, 15, 18–20 and profile-owned skills (decision 46). Facts only; the main session decides.

- Decision 37 holds for Claude: a per-harness mixin's `files/home/.claude/CLAUDE.md` survives create and restart and is loaded by headless `claude -p`.
- A mixin **cannot** ship `~/.claude/settings.json` through `files/home/`: the Claude kit replaces it (no merge). Hooks or settings for Claude must use another route (e.g. a `setup` step that merges into the kit's file, or a different settings scope). Not investigated.
- Codex: mandatory instructions go in `~/.codex/AGENTS.md`; skills in `~/.codex/skills/` or `~/.agents/skills/`. The Codex kit writes `~/.codex/config.toml`, so the same overwrite risk as Claude's `settings.json` probably applies to that file (not tested).
- Gemini CLI: mandatory instructions go in `~/.gemini/GEMINI.md`; skills in `~/.gemini/skills/` or `~/.agents/skills/`. The kit writes `~/.gemini/settings.json` (same untested overwrite risk).
- `~/.agents/skills/` is read by both Codex and Gemini CLI, so it could serve as a shared skills target for those two. The `sbx` Codex setup pre-creates `~/.agents/`, which suggests (unverified) that is where the store gets mounted for Codex.
- With an **empty** store, `--skills readonly` mounts nothing, so mixin skills in `files/home/.claude/skills/` are visible in both `readonly` and `off`. With a non-empty store the mixin's skills may be hidden. That's untested, so `--skills off` for sandboxes with profile-owned skills (decision 46) is still the safe choice.
- Pi paths remain unverified.

## Blocked items and errors

- **Pi sandbox (Q3 Pi, Q4 Pi):** `sbx create --name spike-s6-pi --cpus 2 -m 2g 'git+https://github.com/docker/sbx-kits-contrib.git#ref=d058fedc156325f87612d9bcd9bd313ab74ba100&dir=pi' 'E:\sbxm-it\spike-s6\ws-pi'` failed (exit 1):
  ```
  To allow this kit's publisher and install it, run:
    sbx settings set kit.allowedSources '["docker.io/","github.com/docker/"]'
  Then re-run your original command.
  ```
  Not attempted: changing `sbx settings` (a host configuration change outside what the spec permits), or cloning the
  pinned kit to a local directory and passing it with `--kit` (that would bypass the publisher allowlist). **Question for
  the main session:** allow `kit.allowedSources` to include `github.com/docker/`, or approve using a local clone of the
  pinned commit? Note: sbxm's own Pi support has the same problem. On a default install, `sbx create` with the remote
  Pi kit fails until the user sets `kit.allowedSources`, so `doctor` may need to check this.
- **Pi docs:** both allowed web fetches returned a stub README (details under Q3). Pi's docs now live under `earendil-works/pi` / `pi.dev`, not fetched (budget).
- Tool errors, no effect on results: two Bash calls were refused by the agent harness's worktree guard (the commands `cd`'d outside
  the worktree). The kit files were written with the file-write tool instead.
- One extra web fetch, outside the Q3 per-harness budget: the kit spec `SPEC-v2.md` at commit `d058fedc…` (named as a reference in the spec), to get the mixin/`files/home` format right.

## Follow-ups (not investigated)

- Does a **non-empty** skills store get mounted at `~/.claude/skills` (or `~/.agents/skills` for Codex) and hide mixin files? Needs `sbx skills import` into the store, which this spike forbade.
- Where exactly does the Claude kit write `settings.json`, and is there a supported way to add hooks (merge in `setup.install`, `~/.claude/settings.local.json`, or managed settings)?
- Do the Codex and Gemini kits also overwrite `~/.codex/config.toml` / `~/.gemini/settings.json` shipped by a mixin?
- Pi: instructions file, skills dir, headless mode (`pi --help`), once the kit source is allowed.
- `codex debug prompt-input` could be used in M2 to check instructions and skills injection for Codex without credentials or cost.

## Cleanup

Each sandbox was removed with `sbx rm -f` once its question was done (`Sandbox 'spike-s6-claude-off' removed`,
`'spike-s6-claude-ro' removed`, `'spike-s6-codex' removed`, `'spike-s6-gemini' removed`). `spike-s6-pi` was never
created. `E:\sbxm-it\spike-s6` was deleted (`Remove-Item -Recurse -Force`; `Test-Path` → `False`).

- `sbx ls --json` (no `spike-s6-*`):
  ```
  { "sandboxes": [] }
  ```
- `sbx skills ls --json` (store still empty):
  ```
  { "store": "C:\\Users\\james\\AppData\\Local\\DockerSandboxes\\sandboxes\\state\\agent-skills", "skills": [] }
  ```
- `sbx secret ls --json` (unchanged, masked; identical to the pre-run output):
  ```
  { "secrets": [ { "scope": "global", "type": "service", "name": "anthropic", "secret": "(stored)" } ],
    "custom_secrets": [], "shadowed_services": [], "env_only_count": 1 }
  ```
- Images pulled and sizes: `docker` isn't on PATH in either shell, and `sbx --help` has no image-listing command, so
  exact images and sizes are unknown. Pulled: the Codex and Gemini CLI agent images (the Claude image was already
  present; Pi was never pulled). Free space on C: (the drive holding `…\AppData\Local\DockerSandboxes`) was 14.3 GB before the Codex create,
  11.4 GB after it, 11.2 GB after Gemini, and 11.3 GB at the end. That's about 3 GB for both, left in place.
