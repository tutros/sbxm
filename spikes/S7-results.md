# Spike S7 results

Spec: `spikes/S7.md`. Fill in every section. Status is one of **Answered**, **Partial**, **Blocked**.

- Run date: 2026-09-27 (15:10 to 15:15 local, plus writing up)
- `sbx` version: `sbx version: v0.43.0 79805a6e3c6667520dc2da4f6bdeddae9b700969` (`sbx version`; `sbx --version` is an unknown flag)
- Pinned `sbx-kits-contrib` SHA: `869c83997680a252ed2b35671b3fd0d9adc2d487` (`git ls-remote https://github.com/docker/sbx-kits-contrib.git HEAD` → `869c83997680a252ed2b35671b3fd0d9adc2d487	HEAD`; checked out as `git rev-parse HEAD` → same SHA)
- Budget used: about 25 tool calls / about 6 minutes of commands (under 15 with the write-up) / 0 model calls / 0 web fetches / peak 2 sandboxes (limits: 90 / 75 / 1 / 2 / 2)
- `kit.allowedSources` at start (command + output): `sbx settings get kit.allowedSources` → `["docker.io/","github.com/docker/"]`. `sbx settings get --json kit.allowedSources` → `"source": "override"`, `"default": ["docker.io/"]`, `"env_var": "DOCKER_SANDBOXES_KIT_ALLOWED_SOURCES"`, `"description": "JSON array of allowed kit source prefixes (e.g. [\"docker.io/\", \"ghcr.io/docker/\"]). Use [\"*\"] to allow any remote source."`
- `sbx secret ls` at start:
  ```
  SCOPE      TYPE      NAME        SECRET
  (global)   service   anthropic   (stored)

  Note: 1 additional environment variable found. Run `sbx setup` to review and import.
  ```
- `sbx skills ls --json` at start: `{"store": "C:\\Users\\james\\AppData\\Local\\DockerSandboxes\\sandboxes\\state\\agent-skills", "skills": []}`
- `C:` free space before and after: 13.04 GB before, 12.78 GB after the Pi image pull, 12.79 GB at the end (image left in place). `E:` 20.52 GB.
- Pi version in the image: `0.87.1` (`sbx exec spike-s7-pi -- pi --version`); image `docker.io/sbx/pi-image:latest`

## Q1. What is the Pi kit, at a pinned commit?

**Status:** Answered

- `kind` / `name` / agent name for `requires.agent`: `kind: sandbox` (a full agent kit with its own image, not a mixin), `name: pi`. The agent name is `pi`: a mixin with `requires.agent: pi` composed with it (Q3), `sbx create` printed `agent      pi`, and `sbx ls --json` shows `"agent": "pi"`. The Dockerfile sets `LABEL com.docker.sandboxes.flavor="pi"` and comments that sbx reports the agent from this label.
- Network allow list: `api.anthropic.com`, `registry.npmjs.org`, `platform.claude.com:443`.
- Env and expected secret services: one credential, service `anthropic`. `apiKey` → env `ANTHROPIC_API_KEY`, `proxyManaged: true`, injected as `x-api-key` on `api.anthropic.com`. `oauth` block (for a host whose `anthropic` credential is an OAuth login): token endpoint `platform.claude.com/v1/oauth/token`, sentinels `sk-ant-oat01-proxy-managed` / `sk-ant-ort01-proxy-managed`, written as a `credentialFile` to `~/.pi/agent/auth.json`. The kit declares no `env` of its own; create printed `→ set 1 environment variable(s)`, and the sandbox had `ANTHROPIC_API_KEY` and `SBX_CRED_ANTHROPIC_MODE=none`.
- Install and startup steps (quoted, with user): one install step, `user: "1000"`:
  `if [ -n "${HTTP_PROXY:-}" ]; then npm config set proxy="$HTTP_PROXY" https-proxy="${HTTPS_PROXY:-$HTTP_PROXY}"; fi`
  (writes `~/.npmrc`). No startup steps. Entrypoint `pi`. Pi is baked into the image (Dockerfile: `npm install -g "@earendil-works/pi-coding-agent@${version}"` as `agent`, `version` from npm's `latest` dist-tag at build time; nightly rebuild, so the Pi version floats even with a pinned kit SHA; README "Pinning a kit revision").
- Files written to home: the kit has no `files/` tree (files under `pi/`: `spec.yaml`, `Dockerfile`, `README.md`, `README.image.md`, `testdata/tck.yaml` containing `promptArgs: ["-p"]`). It writes to home only through (a) the install step → `~/.npmrc` and (b) the OAuth `credentialFile` → `~/.pi/agent/auth.json` (README: "the engine writes it at sandbox start", only when the host credential is an OAuth login). `agentInstructions: filename: AGENTS.md` is written outside home: it landed as `/e/sbxm-it/spike-s7/AGENTS.md`, the workspace's parent directory inside the container (Q4).

`spec.yaml` (verbatim, `pi/spec.yaml` at `869c83997680a252ed2b35671b3fd0d9adc2d487`; the file has CRLF line endings):

```yaml
schemaVersion: "2"
kind: sandbox
name: pi
displayName: Pi
description: Minimal terminal coding agent with extensible tools, skills, and TUI
sandbox:
  # Pre-baked image: pi at the latest upstream release, on the shell-docker
  # template. Built and published by build-and-publish-kits.yml, the shared
  # kit-image pipeline (see ../PUBLISHING.md) -- no bespoke workflow needed,
  # same as openclaw/kiro/copilot. Creating a sandbox no longer waits on an
  # npm install; the pipeline's nightly rebuild keeps pi current (rolling
  # updates, no deliberate version bump -- see the Dockerfile).
  image: docker.io/sbx/pi-image:latest
  entrypoint:
    - pi
agentInstructions:
  filename: AGENTS.md
permissions:
  network:
    allow:
      # A credential's inject domains are not allowed implicitly, and e2e runs
      # every kit under deny-all -- see README, "How auth works".
      - api.anthropic.com
      # pi's runtime package installs (extensions, skills, prompt templates,
      # themes) and the packages named in settings, fetched on startup.
      - registry.npmjs.org
      # The oauth block's token-refresh endpoint.
      - platform.claude.com:443
credentials:
  - service: anthropic
    apiKey:
      name: ANTHROPIC_API_KEY
      proxyManaged: true
      inject:
        # pi only sends x-api-key to api.anthropic.com; no other host needs
        # the key, so no other host gets it.
        - domain: api.anthropic.com
          header: x-api-key
          format: '%s'
    # For a host whose stored anthropic credential is an OAuth login -- signed
    # in from a `claude` sandbox. Without this block such a host has no usable
    # credential here: the apiKey sentinel reaches Anthropic unswapped and
    # every model call is a 401. An API key still wins when the host has one.
    oauth:
      tokenEndpoint:
        host: platform.claude.com
        path: /v1/oauth/token
      # Where the bearer is used, so the access-token sentinel is swapped on
      # inference calls and not only on refresh. pi sends a value containing
      # `sk-ant-oat` as `Authorization: Bearer`, so the sentinel below is what
      # selects that format -- see README, "How auth works".
      resourceHosts:
        - api.anthropic.com
      sentinels:
        accessToken: sk-ant-oat01-proxy-managed
        refreshToken: sk-ant-ort01-proxy-managed
      credentialFile:
        # pi's native auth store, which pi prefers over the environment, so
        # nothing in the kit has to translate it.
        path: ~/.pi/agent/auth.json
        structure:
          anthropic:
            type: oauth
            access: "{{.AccessToken}}"
            refresh: "{{.RefreshToken}}"
            # A standalone {{.ExpiresAt}} (SPEC-v2 §5.4.2, Unix ms) materializes
            # as a JSON number in practice -- the type pi's store wants. Observed
            # behavior, not something the spec text promises.
            expires: "{{.ExpiresAt}}"
setup:
  # Config only -- pi itself is baked into the image, so nothing is installed
  # here. This is the re-export pattern of SPEC-v2 §9.5, and it is idempotent,
  # so re-running it on recreate is fine.
  #
  # Guarded on HTTP_PROXY being non-empty: npm rejects an empty value for a
  # url-typed config key, so an unset or empty variable would fail the step and
  # take sandbox creation down with it -- a proxy-less runtime is something
  # every sibling kit tolerates, and this one should too. HTTPS_PROXY is
  # preferred for https-proxy when the runtime provides a dedicated one, with
  # HTTP_PROXY as the fallback. Both keys go in a single `npm config set` so
  # sandbox creation pays for one Node startup rather than two.
  install:
    - command: 'if [ -n "${HTTP_PROXY:-}" ]; then npm config set proxy="$HTTP_PROXY" https-proxy="${HTTPS_PROXY:-$HTTP_PROXY}"; fi'
      user: "1000"
      description: Point npm at the sandbox proxy in ~/.npmrc so pi's runtime npm use (`pi install npm:...`, `pi update`, startup package fetches) works in exec contexts that do not inherit the proxy environment variables
```

Also from `pi/README.md` at the same SHA: the kit is published as an OCI artifact, `sbx run --kit "docker.io/sbx/pi-kit:latest" pi`, with immutable tags `docker.io/sbx/pi-kit:<YYYYMMDD>-<sha>` ("Pinning a kit revision"). See Follow-ups: that source is inside the default `kit.allowedSources`.

## Q2. The exact `kit.allowedSources` refusal

**Status:** Answered

- Command, exit code: `sbx create --name spike-s7-refused --cpus 2 -m 2g 'git+https://gitlab.com/spike-s7/none.git#ref=0000000000000000000000000000000000000000&dir=x' 'E:\sbxm-it\spike-s7\ws-refused'` → exit 1. stdout empty; everything below is on stderr.
- Output (verbatim):
  ```
  ERROR: resolve kits: kit "git+https://gitlab.com/spike-s7/none.git#ref=0000000000000000000000000000000000000000&dir=x": kit "git+https://gitlab.com/spike-s7/none.git#ref=0000000000000000000000000000000000000000&dir=x" cannot be installed — its source is not in your allowlist.

  Your current kit.allowedSources:
    - docker.io/
    - github.com/docker/

  To allow this kit's publisher and install it, run:
    sbx settings set kit.allowedSources '["docker.io/","github.com/docker/","gitlab.com/spike-s7/"]'

  Then re-run your original command.

  To allow any remote source (not recommended), run:
    sbx settings set kit.allowedSources '["*"]'
  ```
- Anything created (`sbx ls --json`, directories): nothing. `sbx ls --json` → `{"sandboxes": []}`; `Test-Path E:\sbxm-it\spike-s7\ws-refused` → `False`. The refusal happens during kit resolution, before the workspace check.
- Matching rule and its source: string prefix of the source `host/path`. Source: `sbx settings get --json kit.allowedSources` description "JSON array of allowed kit source prefixes (e.g. [\"docker.io/\", \"ghcr.io/docker/\"]). Use [\"*\"] to allow any remote source." The suggested fix appends `<host>/<owner>/` (`gitlab.com/spike-s7/` for `git+https://gitlab.com/spike-s7/none.git`), which is how S6's `github.com/docker/` entry was formed. `sbx settings set --help`, `sbx create --help` and `sbx kit --help` don't mention `allowedSources`. Override env var: `DOCKER_SANDBOXES_KIT_ALLOWED_SOURCES`. Local directories are governed separately by `kit.allowLocalKits` (default `true`, `sbx settings list`), which is why sbxm's own mixin dirs pass.

## Q3. Does an sbxm-style mixin compose with the Pi kit?

**Status:** Answered

- `sbx kit validate --json` result: mixin `E:\sbxm-it\spike-s7\kits\pi\` with `spec.yaml`
  ```yaml
  schemaVersion: "2"
  kind: mixin
  name: spike-s7-harness-pi
  requires:
    agent: pi
  ```
  and `files/home/.pi/agent/AGENTS.md` = `SPIKE-S7 CANARY: the word is QUINCE-7` → `{"reference": "E:\\sbxm-it\\spike-s7\\kits\\pi", "kind": "directory", "valid": true, "warnings": []}`, exit 0. `C:` free before create: 13.03 GB.
- `sbx create` exit code and output (verbatim, trimmed only of progress bars): first attempt exited 1 with `The selected workspace does not exist. Would you like to create it? (y/N):` / `ERROR: user cancelled operation` (the workspace dir must exist first; sbxm already creates it). After `New-Item -ItemType Directory E:\sbxm-it\spike-s7\ws-pi`:
  `sbx create --name spike-s7-pi --cpus 2 -m 2g --skills readonly --kit E:\sbxm-it\spike-s7\kits\pi 'git+https://github.com/docker/sbx-kits-contrib.git#ref=869c83997680a252ed2b35671b3fd0d9adc2d487&dir=pi' E:\sbxm-it\spike-s7\ws-pi` → exit 0 in 15 s (including the image pull):
  ```
  ── RESOLVE SETUP
     resolving configuration…
       sandbox    spike-s7-pi
       agent      pi
       kit        git+https://github.com/docker/sbx-kits-contrib.git#ref=869c83997680a252ed2b35671b3fd0d9adc2d487&dir=pi
                  E:\sbxm-it\spike-s7\kits\pi
       workspace  E:\sbxm-it\spike-s7\ws-pi → /e/sbxm-it/spike-s7/ws-pi (rw)
       image      docker.io/sbx/pi-image:latest
       cpu        2
       memory     2g
     ✓ configuration resolved
  ── PREPARE IMAGE
     → pull docker.io/sbx/pi-image:latest
       (12 layers downloaded)
     ✓ image ready
  WARN: credential not sent: no binding authorizes this service
  ── CONFIGURE AGENT
     → set 1 environment variable(s)
     → copy 1 home file(s)
     → run 1 install command(s)
       $ if [ -n "${HTTP_PROXY:-}" ]; then npm config set proxy="$HT… (kit=pi, user=1000)
       ✓ if [ -n "${HTTP_PROXY:-}" ]; then npm config set proxy="$HT… (kit=pi, user=1000, 1.4s)
  Note: no binding authorizes anthropic — the credential was not injected. Create a binding (re-run interactively, or edit ~/.config/sbx/credentials.yaml) to use it.
  ── CREATE SANDBOX
     ✓ Created sandbox spike-s7-pi
  To connect to this sandbox, run:
    sbx run --name spike-s7-pi
  ```
  No skills mount line, no startup commands.
- `sbx ls --json` entry:
  ```json
  {"name": "spike-s7-pi", "id": "302fa506-2c17-4801-b95f-197803b3a8ef", "agent": "pi", "status": "running",
   "last_used_at": "2026-09-27T22:12:00.5095857Z", "workspaces": ["E:\\sbxm-it\\spike-s7\\ws-pi"]}
  ```

## Q4. Pi's always-loaded instructions file

**Status:** Answered

| Check | Result | Evidence |
|---|---|---|
| Path (with source) | `~/.pi/agent/AGENTS.md` (agent dir `~/.pi/agent`, overridable with `PI_CODING_AGENT_DIR`) | Installed `docs/configuration.md`: "`<agent-dir>/AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, or `CLAUDE.MD` \| User instructions applied across working directories." and "User-level configuration lives in the agent directory, which defaults to `~/.pi/agent`"; `pi --help`: "PI_CODING_AGENT_DIR - Config directory (default: ~/.pi/agent)". Confirmed by the rendered prompt below. |
| Canary present after create | Yes | `cat ~/.pi/agent/AGENTS.md` → `SPIKE-S7 CANARY: the word is QUINCE-7` (38 bytes, `agent:agent`, 644) |
| Canary present after restart | Yes | `sbx stop spike-s7-pi` → `Sandbox 'spike-s7-pi' stopped; state preserved.`, `sbx ls` status `stopped`; then `sbx exec` → `Sandbox spike-s7-pi started successfully`, `uptime` `up 0 min`, same 38-byte file, same content |
| QUINCE-7 in rendered prompt or model answer | Yes, in the rendered prompt, after create and after restart (no model call) | See below |

Rendering without a model call: Pi has no CLI prompt dump, but its SDK exposes it (installed `docs/sdk.md`: "`session.systemPrompt` is read-only and returns the current effective system prompt"). Script `render.mjs` in the workspace imported `createAgentSession` from the global install, created a session, printed parts of `session.systemPrompt`, and disposed it without calling `prompt()`. Run as `sbx exec spike-s7-pi -- bash -lc 'cd /e/sbxm-it/spike-s7/ws-pi && PI_OFFLINE=1 timeout --kill-after=10 60 node render.mjs'` → exit 0:

```
PROMPT_LENGTH=19263
QUINCE_MATCHES=1
CONTEXT_AROUND_CANARY:
...
<project_context>
Project-specific instructions and guidelines:

<project_instructions path="/home/agent/.pi/agent/AGENTS.md">
SPIKE-S7 CANARY: the word is QUINCE-7

</project_instructions>
...
MODEL=anthropic/claude-opus-4-8
PATHS=["/home/agent/.pi/agent/AGENTS.md","/e/sbxm-it/spike-s7/AGENTS.md"]
SKILLS_SECTION=false
```

Second context file: sbx writes the kit's `agentInstructions` (`filename: AGENTS.md`) to the **workspace's parent directory inside the container**, `/e/sbxm-it/spike-s7/AGENTS.md` (16,372 bytes, sections "Environment Persistence", "Network access", "Docker network access", ...; not present on the host: `Test-Path E:\sbxm-it\spike-s7\AGENTS.md` → `False`). Pi walks parent directories for context files (`docs/configuration.md`: "Pi loads them from the agent directory, the working directory, and its parent directories"), so for Pi, `agentInstructions` is **always loaded**, not on demand as decision 37 assumes for Claude.

Model call: not made (0 of 1). It wasn't needed, since the prompt rendered without one. It also would have failed: create printed `no binding authorizes anthropic — the credential was not injected`, and `SBX_CRED_ANTHROPIC_MODE=none`, although `pi auth check --provider anthropic --credentials --json` reports `{"status":"ready","provider":"anthropic","authType":"api_key","credentials":"proxy-managed"}` (it only sees the unswapped sentinel; the kit README says that case gives "an opaque `401` on the first model call").

## Q5. Pi skills dir, skills store and config files a mixin must not ship

**Status:** Answered

- Skills dirs (with source): user `~/.pi/agent/skills/` (installed `docs/configuration.md`: "`<agent-dir>/skills/` \| User skills and supporting files.") and `~/.agents/skills/` (installed `docs/skills.md`: "Pi also supports the Agent Skills locations `~/.agents/skills/` and `.agents/skills/`."). Project: `.pi/skills/` (needs project trust) and `.agents/skills/` from the working directory up to the repo root. Neither user dir exists in a fresh sandbox (`ls: cannot access '/home/agent/.pi/agent/skills'` / `'/home/agent/.agents/skills'`).
- Store mount announced / actually mounted: **No / No.** Neither create (both with `--skills readonly`) printed a skills line. `mount | grep -ciE skill` → `0`; `findmnt -T /home/agent/.pi/agent` → only the root overlay; the rendered prompt has no `<available_skills>` section. Caveat: the store is empty (`"skills": []`), and S6b saw that an empty store mounts nothing for Claude either, so this doesn't prove the store would skip Pi when it has skills. It's consistent with decision 46.

Second mixin `E:\sbxm-it\spike-s7\kits\pi-config\` (`name: spike-s7-pi-config`, `requires.agent: pi`, validate → `"valid": true, "warnings": []`) shipping three files; `sbx create --name spike-s7-pi-config --cpus 2 -m 2g --skills readonly --kit E:\sbxm-it\spike-s7\kits\pi-config '<pi kit ref at 869c8399…>' E:\sbxm-it\spike-s7\ws-pi-config` → exit 0, `→ copy 3 home file(s)`, same install step and credential note as Q3. Files `cat`ed before running `pi` at all, then after `sbx stop` + `sbx exec` (`up 0 min`):

| Config file | After create | After restart | Evidence |
|---|---|---|---|
| `~/.npmrc` (kit install step) | **merged** (key kept, comment dropped) | same | Shipped `# spike-s7 canary` + `spike-s7-canary=true`. After: 113 bytes, `spike-s7-canary=true` / `proxy=http://gateway.docker.internal:3128/` / `https-proxy=http://gateway.docker.internal:3128/`. npm rewrote the file and dropped the comment line. Unchanged after restart (same mtime 22:14:28). |
| `~/.pi/agent/auth.json` (kit OAuth `credentialFile`) | **kept** (host credential is not OAuth, so the engine didn't write it) | same | `{"spike-s7-canary": {"type": "api_key", "key": "SPIKE-S7-NOT-A-KEY"}}` (70 bytes), mode 644 (Pi's own is 600). `pi auth check` still `ready`/`proxy-managed` because the canary isn't an `anthropic` entry. Untested: a host with an OAuth `anthropic` login, where the engine writes this file at every start. |
| `~/.pi/agent/settings.json` (not written by the kit; Pi's main user config) | **kept** | same | `{"spikeCanary": true}` (22 bytes), same mtime after restart |

In a sandbox without the mixin, Pi itself creates `~/.pi/agent/auth.json` and `~/.pi/agent/models-store.json` (2 bytes each) on first run.

## Q6. Headless command and provider (record only)

**Status:** Answered

- Headless flag: `-p`/`--print` ("Non-interactive mode: process prompt and exit", `pi --help`); `pi -p -- "<prompt>"` for prompts starting with a dash. The kit's `testdata/tck.yaml` is `promptArgs: ["-p"]`. `--mode json` gives JSON event output, `--mode rpc` an RPC mode. `--no-session` avoids saving a session; sessions are otherwise saved under `~/.pi/agent/sessions/` (created by the SDK render).
- Model selection: `--model <pattern>` ("supports "provider/id" and optional ":<thinking>""), `--provider <name>` ("default: google"), `--thinking <level>`, `--models` for cycling. With only `ANTHROPIC_API_KEY` set, the effective model was `anthropic/claude-opus-4-8` (SDK render, `session.model`).
- Usage/cost output: yes, in JSON mode. Installed `docs/json.md`: `{"type":"message_update","usage":{"input":100,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":101,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},...}`; "The top-level `usage` is the latest cumulative provider-reported usage for the assistant response." Not observed live (no model call).
- Provider domains in the kit: `api.anthropic.com` (inference), `platform.claude.com:443` (OAuth refresh), plus `registry.npmjs.org` (Pi packages). Only the `anthropic` credential is declared, though Pi supports many providers via env vars (`OPENAI_API_KEY`, `GEMINI_API_KEY`, ...; `pi --help`).

## Blocked items and errors

- Nothing is Blocked.
- First `sbx create` for Q3 exited 1: `The selected workspace does not exist. Would you like to create it? (y/N):` / `ERROR: user cancelled operation`. Fixed by creating the workspace dir first (a different step on retry, not a repeat).
- Credentials: both creates printed `WARN: credential not sent: no binding authorizes this service` and `Note: no binding authorizes anthropic — the credential was not injected. Create a binding (re-run interactively, or edit ~/.config/sbx/credentials.yaml) to use it.` The global `anthropic` service secret is stored but not injected into a Pi sandbox without a binding. Not investigated (sbx credentials config is off limits); it blocks any real Pi model call.
- Tool errors, no effect: PowerShell's `Set-Content -NoNewline` was rejected once (`A parameter cannot be found that matches parameter name 'NoNewline'`); the file was edited with the file tool instead.

## Follow-ups

- **The Pi kit is published at `docker.io/sbx/pi-kit`** (`pi/README.md` "Usage" and "Pinning a kit revision", with immutable `<YYYYMMDD>-<sha>` tags). `docker.io/` is in the default `kit.allowedSources`, so using that OCI ref could make decision 48's allowlist check unnecessary. Untested: whether sbx accepts that ref as the agent argument and how its tags map to repo SHAs.
- **Credential binding:** why `anthropic` isn't injected into the Pi sandbox ("no binding authorizes anthropic", `~/.config/sbx/credentials.yaml`), whether Claude sandboxes use a different path, and what the user must do before `sbxm new --harness pi` can make model calls. sbxm's secret check (`secret_services`) may need to account for bindings.
- **`agentInstructions` is always loaded for Pi:** sbx writes it as `AGENTS.md` in the workspace's parent directory inside the container, and Pi reads parent-directory context files. That affects decision 37 for Pi (sbxm's common reference instructions would become mandatory context), and possibly Codex/others that also walk parents.
- **Config files and decision 61 for Pi:** `.pi/agent/settings.json` and `.npmrc` survive (npm merges `.npmrc` but drops comments); `.pi/agent/auth.json` survives but overrides the environment in Pi's resolution order (README: "`--api-key` > `auth.json` > env > `models.json`") and is rewritten by the engine for OAuth hosts, so a mixin shipping it looks like a candidate for refusal. Not tested with an OAuth host credential.
- Pi floats with the image (`pi-image:latest`, rebuilt nightly): pinning the kit SHA doesn't pin Pi's version (0.87.1 today). The Pi image is on `docker.io/sbx/`, not checked against any image allowlist.
- Pi's default `--provider` is `google`; with an Anthropic key the effective model was `anthropic/claude-opus-4-8`. sbxm's Pi adapter should pass `--model` explicitly.

## Cleanup

`sbx rm -f spike-s7-pi` → `Sandbox 'spike-s7-pi' removed`; `sbx rm -f spike-s7-pi-config` → `Sandbox 'spike-s7-pi-config' removed`. `Remove-Item -Recurse -Force E:\sbxm-it\spike-s7`. `spike-s7-refused` was never created.

- `sbx ls --json` (no `spike-s7-*`): `{"sandboxes": []}`
- `kit.allowedSources`, `sbx secret ls`, `sbx skills ls --json` unchanged: `sbx settings get kit.allowedSources` → `["docker.io/","github.com/docker/"]`; `sbx secret ls` → `(global)   service   anthropic   (stored)` plus the same `1 additional environment variable` note; `sbx skills ls --json` → same store path, `"skills": []`. All identical to the start.
- `E:\sbxm-it\spike-s7` gone: `Test-Path E:\sbxm-it\spike-s7` → `False`
- Pi image `docker.io/sbx/pi-image:latest` left in place; `C:` free after: 12.79 GB.
