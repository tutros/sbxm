# Spike S6b results

Spec: `sdlc/spikes/S6b.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date: 2026-09-25 (about 19:49 to 19:56 local)
- `sbx` version: `sbx version: v0.43.0 79805a6e3c6667520dc2da4f6bdeddae9b700969`
- Budget used: about 27 tool calls, about 7 minutes, 2 model calls (limits: 100 / 75 / 4). 1 web fetch (pinned SPEC-v2). Peak sandboxes at once: 3.
- Skills store before the run (`sbx skills ls --json`):
  ```
  { "store": "C:\\Users\\james\\AppData\\Local\\DockerSandboxes\\sandboxes\\state\\agent-skills", "skills": [] }
  ```
- Claude Code in the image: `2.1.280 (Claude Code)`. C: free: 9.77 GB before, 9.76 GB after (no full image pulls; the
  Gemini pull fetched one layer).

## Q1. Does a non-empty skills store hide mixin skills?

**Status:** Blocked (the canary could not be put into the store; see below). Only empty-store evidence was collected,
which repeats S6.

| Sandbox | Store mounted at | Store canary (LIME-4) readable | Mixin canary (PEAR-8) readable |
|---|---|---|---|
| `spike-s6b-claude` (`~/.claude/skills`) | Not tested with a non-empty store. Empty store: `sbx create` announces `→ /home/agent/.claude/skills · 0 folders · (ro)`, but nothing is mounted (`findmnt -T` → `/ overlay`) | n/a (never added) | Yes (empty store only) |
| `spike-s6b-codex` (`~/.codex/skills`) | Not tested. Empty store: no mount; dir absent after create, created by Codex itself (`.system/`) after `codex debug prompt-input` | n/a | n/a |
| `spike-s6b-codex` (`~/.agents/skills`) | Not tested. Empty store: `sbx create` announces `→ /home/agent/.agents/skills · 0 folders · (ro)`, but nothing is mounted and `~/.agents/skills` doesn't exist (`~/.agents/` is an empty dir) | n/a | n/a |

Commands used to add and remove the canary: none succeeded; nothing was ever added, so nothing needed removing.

- `sbx skills --help` lists `add` (Git repository only), `import` (from fixed host agent dirs), `ls`, `rm`
  (`sbx skills rm <skill>... [-f]`), `update`. A remove command exists, so the precondition held.
- No command adds a skill from a local directory:
  ```
  PS> sbx skills add 'E:\sbxm-it\spike-s6b\store-canary' --skill spike-s6b-store-canary
  ERROR: repository must be a remote Git URL or GitHub owner/repository: unknown protocol
  PS> sbx skills add 'file:///E:/sbxm-it/spike-s6b/store-canary' --skill spike-s6b-store-canary   # after a throwaway `git init` + commit in that spike dir
  ERROR: repository must be a remote Git URL or GitHub owner/repository: file: invalid protocol
  ```
- `sbx skills import` resolves `~` from the process environment. With `USERPROFILE`/`HOME` pointed at a fake home
  under the spike dir (`E:\sbxm-it\spike-s6b\fakehome\.agents\skills\spike-s6b-store-canary\SKILL.md`), a dry run
  showed only the canary:
  ```
  PS> sbx skills import --dry-run
  Would import skill "spike-s6b-store-canary"
  No skills directory found at E:\sbxm-it\spike-s6b\fakehome\.claude\skills; nothing to import.
  ... (.config/opencode, .copilot, .cursor, .factory: same) ...
  Dry run: 1 skill(s) would be imported into C:\Users\james\AppData\Local\DockerSandboxes\sandboxes\state\agent-skills
  ```
  The real import was **denied by the agent's permission classifier** ("Modify Shared Resources"). I didn't retry or
  look for another route. The store was never changed.

Evidence (empty store, `spike-s6b-claude`, default `--skills`, right after create):

```
== findmnt            (findmnt -T /home/agent/.claude/skills -o TARGET,SOURCE,FSTYPE)
TARGET SOURCE  FSTYPE
/      overlay overlay
== mountinfo skills   (grep -i skill /proc/self/mountinfo: no lines)
== ls skills
drwxr-xr-x 2 agent agent 4096 Sep 26 02:52 spike-s6b-mixin-canary
== mixin canary
SPIKE-S6B MIXIN CANARY: the word is PEAR-8
```

Codex (`spike-s6b-codex`): after create, `findmnt -T` printed nothing and `ls` said `No such file or directory` for
both `~/.codex/skills` and `~/.agents/skills`, and `grep -i -e skill -e agents /proc/self/mountinfo` had no lines.
After restart, `~/.codex/skills` held only `.system` (Codex's built-in skills), `findmnt -T` → `/ overlay`, and `~/.agents/` was an empty dir.

## Q2. A working route for Claude hooks

**Status:** Answered

| Route | File in place after create | Marker after 1st call | Survives restart | Marker after restarted call | Agent can modify it |
|---|---|---|---|---|---|
| A. Managed settings (`setup.install`, user `0`) | Yes, `root:root 0644`, dir `root:root 0755` | **Yes** | Yes (file unchanged) | **Yes** | Not directly (`test -w` fails), but **`sudo -n true` exits 0**, so the agent can change or delete it with sudo |
| B. Merge in `setup.install` (user `agent`, `jq`) | Yes, kit keys kept plus `hooks` | **Yes** | Yes (file unchanged) | **Yes** | n/a (agent owns `~/.claude/settings.json`) |
| C. `~/.claude/settings.local.json` (`files/home/`) | Yes, kit didn't touch it | **No** | Yes (file unchanged) | **No** | n/a |

Route A via `files/`: not possible. SPEC-v2 (pinned): "Only `files/home/` and `files/workspace/` are recognized
targets; any other subdirectory is ignored with a warning." and "Relative paths only. Absolute paths and `..`
traversal are rejected". So route A used a `setup.install` step with `user: "0"`. (SPEC-v2 also has a `setup.files`
list with absolute `path`s. It wasn't tried; see Follow-ups.)

Install order printed by `sbx create` (the agent kit's steps first, then the mixin's in `--kit` order, matching SPEC-v2's
"all three lists concatenate in `--kit` order"):

```
→ copy 2 home file(s)
→ run 5 install command(s)
  ✓ set -e ws="${WORKSPACE_DIR:-/}" esc=$(printf '%s' "$ws" | s… (kit=claude, user=0, 20ms)
  ✓ set -e mkdir -p /home/agent/.claude HELPER='' if [ "${SBX_C… (kit=claude, user=agent, 24ms)
  ✓ set -e [ -n "$MCP_GATEWAY_URL" ] || exit 0 export PATH="$HO… (kit=claude, user=agent, 1.2s)
  ✓ set -e mkdir -p /etc/claude-code cat > /etc/claude-code/man… (kit=spike-s6b-claude, user=0, 25ms)
  ✓ set -e f=/home/agent/.claude/settings.json echo "S6B route … (kit=spike-s6b-claude, user=agent, 39ms)
→ register 3 startup command(s), run on every container start
  + sh -c chown -R agent:agent /home/agent/.claude/projects /ho… (kit=claude, user=0)
  + sh -c { command -v apt-get && apt-get update -qq -y || true… (kit=claude, user=root)
  + sh -c set -e [ -n "$MCP_GATEWAY_URL" ] || exit 0 export PAT… (kit=claude, user=agent)
```

`user: "agent"` passed `sbx kit validate --json` (`"valid": true, "warnings": []`) and ran as `user=agent`.

Evidence:

Mixin `E:\sbxm-it\spike-s6b\kits\claude\spec.yaml` (abridged; hook JSON identical in all three routes except the marker):

```yaml
schemaVersion: "2"
kind: mixin
name: spike-s6b-claude
requires: { agent: claude }
setup:
  install:
    - user: "0"        # route A
      command: |
        set -e
        mkdir -p /etc/claude-code
        cat > /etc/claude-code/managed-settings.json <<'EOF'
        {"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch /tmp/s6b-route-a"}]}]}}
        EOF
        chmod 644 /etc/claude-code/managed-settings.json
    - user: "agent"    # route B
      command: |
        set -e
        f=/home/agent/.claude/settings.json
        [ -f "$f" ] || echo '{}' > "$f"
        hook='{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch /tmp/s6b-route-b"}]}]}}'
        jq --argjson h "$hook" '. * $h' "$f" > "$f.tmp" && mv "$f.tmp" "$f"   # (node fallback unused; jq is /usr/bin/jq)
```
plus `files/home/.claude/settings.local.json` (route C) and `files/home/.claude/skills/spike-s6b-mixin-canary/SKILL.md`.

Right after create (`sbx create --name spike-s6b-claude --cpus 2 -m 2g --kit E:\sbxm-it\spike-s6b\kits\claude claude E:\sbxm-it\spike-s6b\ws-claude`):

```
== /home/agent/.claude/settings.json
-rw-r--r-- 1 agent agent 435 Sep 26 02:52 /home/agent/.claude/settings.json
{
  "themeId": 1,
  "alwaysThinkingEnabled": true,
  "apiKeyHelper": "echo proxy-managed",
  "permissions": { "defaultMode": "bypassPermissions" },
  "bypassPermissionsModeAccepted": true,
  "skipDangerousModePermissionPrompt": true,
  "hooks": { "SessionStart": [ { "hooks": [ { "type": "command", "command": "touch /tmp/s6b-route-b" } ] } ] }
}
== /home/agent/.claude/settings.local.json
-rw-r--r-- 1 agent agent 94 Sep 26 02:52 /home/agent/.claude/settings.local.json
{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch /tmp/s6b-route-c"}]}]}}
== /etc/claude-code/managed-settings.json
-rw-r--r-- 1 root root 95 Sep 26 02:52 /etc/claude-code/managed-settings.json
{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch /tmp/s6b-route-a"}]}]}}
== sudo
sudo exit=0
uid=1000(agent) gid=1000(agent) groups=1000(agent),27(sudo),1001(docker)
```

Model call 1 of 4 (no markers existed before it):

```
PS> sbx exec spike-s6b-claude claude -p "Reply with OK only." --max-turns 1 --max-budget-usd 0.10
OK
exit=0
-rw-r--r-- 1 agent agent 0 2026-09-26 02:52:51.582619300 +0000 /tmp/s6b-route-a
-rw-r--r-- 1 agent agent 0 2026-09-26 02:52:51.578619300 +0000 /tmp/s6b-route-b
```

`sbx stop spike-s6b-claude` → `Sandbox 'spike-s6b-claude' stopped; state preserved.`, `sbx ls` status `stopped`; then
`sbx exec` → `Sandbox spike-s6b-claude started successfully`, `/proc/uptime` `2.43` (really restarted). The old
markers `/tmp/s6b-route-a` and `-b` were still there (`/tmp` persists across stop/start). All three files were
byte-identical to the post-create output above. Markers removed with `rm -f /tmp/s6b-route-*` (then `ls` → `No such file or directory`).

Model call 2 of 4 (after restart):

```
OK
exit=0
-rw-r--r-- 1 agent agent 0 2026-09-26 02:53:17.344874800 +0000 /tmp/s6b-route-a
-rw-r--r-- 1 agent agent 0 2026-09-26 02:53:17.352874800 +0000 /tmp/s6b-route-b
== route A perms
-rw-r--r-- 1 root root 95 Sep 26 02:52 /etc/claude-code/managed-settings.json
drwxr-xr-x 2 root root 4096 Sep 26 02:52 /etc/claude-code
agent cannot write directly (no sudo)      # test -w
sudo -n true exit=0
```

## Q3. Do the Codex and Gemini kits keep a mixin's home files?

**Status:** Answered

| File | After create | After restart |
|---|---|---|
| `~/.codex/AGENTS.md` | Present with canary (`SPIKE-S6B CANARY: the word is PLUM-9`, 37 bytes) | Present with canary, unchanged |
| `~/.codex/config.toml` | **Replaced** by the kit's config (no `# spike-s6b canary` line) | Still the kit's version |
| `~/.gemini/GEMINI.md` | Present with canary (37 bytes) | Present with canary, unchanged |
| `~/.gemini/settings.json` | **Merged**: `spikeCanary: true` kept, kit added `mcpServers.mcp-gateway` | Still merged (mtime changed 02:52 → 02:53, so it's rewritten on each start; content the same) |

PLUM-9 in `codex debug prompt-input`: **Yes** (after create and after restart; 1 match each).

Evidence:

Mixins: `kits\codex\` and `kits\gemini\`, `spec.yaml` with only `schemaVersion: "2"`, `kind: mixin`, `name`,
`requires.agent: codex|gemini`, plus the two `files/home/` files each. Both `sbx kit validate --json` → `"valid": true, "warnings": []`.
Created with `sbx create --name spike-s6b-codex --cpus 2 -m 2g --kit E:\sbxm-it\spike-s6b\kits\codex codex E:\sbxm-it\spike-s6b\ws-codex`
(and the same for `gemini`). Create output: Codex `→ copy 2 home file(s)`, `→ run 1 install command(s)`
(`set -e mkdir -p /home/agent/.codex /home/agent/.agents MODE… (kit=codex, user=agent)`), 2 startup commands
(the second `set -e [ -n "$MCP_GATEWAY_URL" ] || exit 0 cfg="$HOME… (kit=codex, user=agent)`). Gemini
`→ copy 2 home file(s)`, no install commands, 3 startup commands, the third `set -e [ -n "$MCP_GATEWAY_URL" ] || exit 0 frag=/tmp/… (kit=gemini, user=agent)`.
Codex also warned `no OpenAI credentials available. codex will start logged-out.`

Codex, right after create (identical after restart apart from uptime `1.60`):

```
== /home/agent/.codex/AGENTS.md
-rw-r--r-- 1 agent agent 37 Sep 26 02:52 /home/agent/.codex/AGENTS.md
SPIKE-S6B CANARY: the word is PLUM-9
== /home/agent/.codex/config.toml
-rw-r--r-- 1 agent agent 371 Sep 26 02:53 /home/agent/.codex/config.toml
# Codex configuration for Docker sandbox
# This configuration enables "yolo mode" - no approvals, full access

approval_policy = "never"
sandbox_mode = "danger-full-access"
mcp_oauth_credentials_store = "file"

[mcp_servers.mcp-gateway]
type = "http"
url = "http://mcp-gateway.docker.internal/mcp"
[mcp_servers.mcp-gateway.headers]
Authorization = "Bearer proxy-managed"
```

`codex debug prompt-input "hi"` (no model call), written to `/tmp/pi.txt`, exit 0, 7960 bytes, `grep -c PLUM-9` → `1`:

```
AGENTS.md instructions\n\n<INSTRUCTIONS>\nSPIKE-S6B CANARY: the word is PLUM-9\n</INSTRUCTIONS>"
```

After restart: `codex debug prompt-input "hi" | grep -c PLUM-9` → `1`. (A first attempt piping straight into
`grep -o ".\{60\}PLUM-9.\{20\}"` returned exit 1. The likely cause is that the trailing `.\{20\}` needs more characters than follow
`PLUM-9` on that line (unverified). The saved-file rerun above is the valid check.)

Gemini, right after create:

```
== /home/agent/.gemini/GEMINI.md
-rw-r--r-- 1 agent agent 37 Sep 26 02:52 /home/agent/.gemini/GEMINI.md
SPIKE-S6B CANARY: the word is PLUM-9
== /home/agent/.gemini/settings.json
-rw-rw-r-- 1 agent agent 210 Sep 26 02:52 /home/agent/.gemini/settings.json
{
  "spikeCanary": true,
  "mcpServers": {
    "mcp-gateway": {
      "httpUrl": "http://mcp-gateway.docker.internal/mcp",
      "headers": { "Authorization": "Bearer proxy-managed" }
    }
  }
}
```

After `sbx stop` + `sbx exec` (uptime `2.24`): `GEMINI.md` identical; `settings.json` identical content, mtime `02:53`.
Note: S6 saw the kit's own `settings.json` holding `tools.sandbox=false` and `folderTrust.enabled=false`. With a mixin
file present, those keys are **absent**, so the mixin's file replaces the kit's base file and only the MCP fragment is merged in on top.

## Implications

For slices 14, 15, 18–19 and decision 46. Facts only; the main session decides.

- Claude hooks: route B works. It's a mixin `setup.install` step (`user: "agent"`) that `jq`-merges into
  `~/.claude/settings.json`. It runs after the Claude kit's install steps, keeps the kit's keys, fires in `claude -p`,
  and survives stop/start (install runs once; nothing rewrites the file on start). `jq` is at `/usr/bin/jq` in the
  Claude image.
- Route A also works: managed settings at `/etc/claude-code/managed-settings.json`, written by a root install step,
  because `files/` can't target `/etc`. It's root-owned, but the agent has passwordless sudo (`sudo -n true` → 0), so
  it is **not tamper-proof** against the agent.
- Route C doesn't work: Claude Code 2.1.280 ignores `~/.claude/settings.local.json` for hooks, although the kit leaves
  the file in place.
- Mixin install steps can be `user: "0"` or `user: "agent"`. They run once at create, after all agent-kit install
  steps, in `--kit` order.
- `/tmp` persists across `sbx stop`/start.
- Codex: a mixin's `~/.codex/AGENTS.md` survives create and restart and is in the rendered prompt. A mixin's
  `~/.codex/config.toml` is **replaced** by the kit, the same problem as Claude's `settings.json`. Codex config would
  need a merge route (e.g. a `setup.install` step; untested for Codex).
- Gemini: a mixin's `~/.gemini/GEMINI.md` survives. A mixin's `~/.gemini/settings.json` survives, and the kit merges
  its MCP gateway entry into it on every start. The kit's own defaults (`tools.sandbox=false`,
  `folderTrust.enabled=false`) are then absent, so a mixin-shipped `settings.json` must carry those itself if they matter.
- Decision 46 (non-empty store hiding mixin skills): **still unverified**. `sbx create` always announces a store mount
  at `~/.claude/skills` (Claude) or `~/.agents/skills` (Codex), even with 0 folders. With an empty store nothing is
  mounted. No local-directory route exists to put a test skill in the store (`add` takes only remote Git URLs), and
  import from a fake home was refused by the permission classifier. `--skills off` for sandboxes with profile-owned
  skills is still the safe choice.

## Blocked items and errors

- **Q1 canary:** `sbx skills add` rejects local paths (`unknown protocol`) and `file://` URLs (`file: invalid
  protocol`). `sbx skills import` with `USERPROFILE`/`HOME` pointed at a fake home in the spike dir passed
  `--dry-run` (only the canary), but the real import was denied by the Claude Code auto-mode permission classifier
  (`[Modify Shared Resources]`). Not retried and not worked around. To unblock, the user could approve that import, or
  push the canary to a throwaway remote Git repo for `sbx skills add <url> --skill spike-s6b-store-canary`, then remove it with `sbx skills rm -f spike-s6b-store-canary`.
- A throwaway `git init` + commit was run inside `E:\sbxm-it\spike-s6b\store-canary` (outside the repo, deleted in
  cleanup) for the `file://` attempt. No git command touched the worktree.
- Tool error, no effect on results: the first `codex debug prompt-input | grep -o` pattern returned exit 1. The pattern
  was too strict (see Q3); the rerun was conclusive.

## Follow-ups (not investigated)

- Non-empty skills store vs. mixin skills (Q1): still open; needs an approved way to add one skill.
- SPEC-v2 `setup.files` (absolute `path`, `content`, `mode`, `onlyIfMissing`) might place managed settings without a
  shell step. Who owns the written file, and does it run before or after agent-kit steps?
- Route A tamper resistance: the agent has passwordless sudo in the Claude image. Can a kit or `sbx` option drop sudo?
- Codex `config.toml`: does a `setup.install` merge (like route B) survive, or does the Codex startup step
  (`cfg="$HOME…`) rewrite the file on each start?
- Gemini: the startup step rewrites `settings.json` on every start (mtime changed). Does it always preserve the other keys?
- Does a Claude `SessionStart` hook also fire on interactive `sbx run` sessions? Only `claude -p` was tested.

## Cleanup

- Canary removed from the store (command + output): not needed; the canary was never added (import denied). Store
  checked before cleanup: `{ "store": "…agent-skills", "skills": [] }`.
- Sandboxes: `sbx rm -f spike-s6b-claude` / `-codex` / `-gemini` → `Sandbox 'spike-s6b-claude' removed`,
  `'spike-s6b-codex' removed`, `'spike-s6b-gemini' removed`. `Remove-Item -Recurse -Force E:\sbxm-it\spike-s6b` → `Test-Path` `False`.
- `sbx ls --json`:
  ```
  { "sandboxes": [] }
  ```
- `sbx skills ls --json`:
  ```
  { "store": "C:\\Users\\james\\AppData\\Local\\DockerSandboxes\\sandboxes\\state\\agent-skills", "skills": [] }
  ```
- `sbx secret ls` (table, identical to the pre-run output):
  ```
  SCOPE      TYPE      NAME        SECRET
  (global)   service   anthropic   (stored)

  Note: 1 additional environment variable found. Run `sbx setup` to review and import.
  ```
