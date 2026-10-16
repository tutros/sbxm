# Spike S15 results: templates with the Pi harness

Spec: `sdlc/spikes/S15.md`. Filled in by the worker; everything is observed unless marked **inferred**.

**Verdict:** works. `sbx create -t TAG` combines with the Pi kit reference: the template's image is used, the kit's
environment wins, the narrow kit applies, no real key is in the template, and Pi answers from a template sandbox with
an empty allow list, signed in with the user's Claude subscription (OAuth). Questions 1 and 5 were first Blocked
because the account was out of extra usage; the user added credits and both were rerun the same day (Rerun below).

## Conditions

- 2026-10-16, 19:11-19:30 UTC. `sbx` 0.46.0. sbxm binary `H:\sbxm-e2e\sbxm-ae857fa.exe` (main ae857fa), not the
  spec's `sbxm-8cf72f4.exe`: the user chose the newer build so slice 0 and S15 ran on the same one; nothing that S15
  uses (create, kits, harnesses) changed between them. C: free at the start: 30.7 GiB.
- Other sandboxes running during timings: none (`sbx ls` empty before each create).
- **Deviation from the spec's setup:** the install step is `apt-get update || true; apt-get install -y jq` instead of
  `apt-get update && apt-get install -y jq`. The first `sbxm new` failed with it (question 1 evidence): the Pi image
  ships an apt source for `download.docker.com`, which the profile's allow list doesn't have. The `|| true` form is
  the one sbxm's own profiles use.
- Timings are wall time between two clock reads in separate tool calls, so each includes a few seconds of tool
  overhead; question 4's create especially may include waiting. They are rougher than S11's and slice 0's (decision
  217), which wrapped the command itself.

## Questions

| # | Question | Status | Result |
|---|---|---|---|
| 1 | Baseline | Answered | `sbxm new s15-base --profile s15-pi --harness pi` created `sbxm-s15-base-pi` in about 19.6 s (image already pulled by a first failed try; the Pi kit's npm step 0.9 s, the jq step 8.8 s). `pi -p` first got `400 ... You're out of extra usage`; in the rerun it answered `MEDLAR-15`. |
| 2 | Save | Answered | `sbx template save` (stopped sandbox) took about 5.5 s; `flavor` `pi`, size 734,437,048 bytes. The baseline sandbox was then removed. |
| 3 | Create from the template with the same kits | Answered | Starts, about 24.9 s. Both markers (`~/s15-marker`, `/tmp/s15-marker`) came back, so the template's image was used, not the Pi kit's; `SBXM_CONFIG_HASH` is the kit's (`b58444cd514f`). Setup steps ran again: the Pi kit's npm proxy step (0.9 s) and the jq step (3.6 s). Pi has no `/var/log/sbx-kit-startup.log`. |
| 4 | Narrow kit | Answered | Starts (about 93.8 s between clock reads, see Conditions; the create output shows only the Pi kit's step, 1.0 s). `jq` present. `archive.ubuntu.com`, `example.com` and `github.com` answered 403, each in `sbx policy log` as "No matching allow rule (default deny)". The Pi kit's own rules allow only `api.anthropic.com`, `platform.claude.com:443` and `registry.npmjs.org` (`sbx policy ls --wide`), so unlike `codex` (decision 217) a Pi sandbox can be held to the profile's list plus the provider. |
| 5 | Pi works from the template | Answered | First Blocked with question 1's error (twice in all, stop rule). In the rerun, from a new template (`pi-2`) with the narrow kit (no installs, `allow: []`), `pi -p` answered `MEDLAR-15`: the provider sign-in and `AGENTS.md` both work from a template. |
| 6 | Secrets in the template | Answered | 0 matches for the Anthropic key pattern in the environment, process command lines and filesystem (without `/proc`, `/sys`, `/dev`, `/run` and the workspace). `~/.pi/agent/auth.json`: `anthropic.access` and `anthropic.refresh` of length 26 (placeholders, as S11 found for `claude`), `expires`, `type`. The template carried question 1's Pi session file (`~/.pi/agent/sessions/--tmp--/2026-10-16T19-17-33-*.jsonl`, written before the save), as slice 0 observed for `codex`. |
| 7 | Disk | Partial | Recorded: C: free at the start, just before and just after the save (0.07 GiB used by the save), after cleanup, and the template's reported size (see Disk). **Not recorded:** C: free before and after each of the three creates (questions 1, 3 and 4), and what the template added on disk beyond the save's own reading. |

## Evidence

1. First try, with `apt-get update && ...`:
   ```
   ✗ apt-get update && apt-get install -y jq (kit=sbxm-common, user=0, exit 100)
   E: Failed to fetch https://download.docker.com/linux/ubuntu/dists/resolute/InRelease  403  Forbidden
   error: failed to apply kit to sandbox
   ```
   It left no sandbox (`sbx ls` empty). Second try: `✓ ... (kit=pi, user=1000, 879ms)`,
   `✓ apt-get update || true; apt-get install -y jq (kit=sbxm-common, user=0, 8.8s)`, `✓ Created sandbox
   sbxm-s15-base-pi`. `sbxm new` also warned: `skills.store is "readonly", but pi sandboxes don't support it`.
   `sbx exec sbxm-s15-base-pi bash -lc "cd /tmp && timeout 120 pi -p '<question>' < /dev/null"`:
   `400 {"type":"error","error":{"type":"invalid_request_error","message":"You're out of extra usage. ..."}}`.
2. Markers written to `~` and `/tmp`; `~/.pi/agent` held `AGENTS.md` (41 bytes), `auth.json`, `models-store.json`,
   `sessions/`. `sbx stop`, then `sbx template save sbxm-s15-base-pi sbxm-spike-s15:pi-1`: `Save complete`.
   `sbx template ls --json`: `"repository": "docker.io/library/sbxm-spike-s15", "tag": "pi-1", "flavor": "pi",
   "size": 734437048`. `sbx rm --force sbxm-s15-base-pi`.
3. `sbx create --name sbxm-spike-s15-same -t sbxm-spike-s15:pi-1 --pull never --cpus 4 -m 8g --skills readonly
   --kit <copied common> --kit <copied harness-pi> docker.io/sbx/pi-kit:20260924-d058fedc... <ws\same>`: `image
   sbxm-spike-s15:pi-1`, `✓ image ready`, both install steps ran, `✓ Created`. In it: both markers `s15`,
   `SBXM_CONFIG_HASH=b58444cd514f`, `/usr/bin/jq`. `sbx rm --force sbxm-spike-s15-same`.
4. Narrow `common` kit: `allow: []`, no `setup.install`. Create as in 3 with it: `run 1 install command(s)` (the Pi
   kit's), `✓ Created`. Probes: `archive.ubuntu.com 403`, `example.com 403`, `github.com 403`,
   `registry.npmjs.org 200`. `sbx policy ls sbxm-spike-s15-narrow --wide`: one kit rule, allow tcp
   `api.anthropic.com`, `platform.claude.com:443`, `registry.npmjs.org`.
5. `pi -p` in `sbxm-spike-s15-narrow`: the same `400 ... out of extra usage` (request id differs).
6. A script printing counts, names and lengths only (S11's method); output as in the table.

### Rerun of questions 1 and 5 (same day, after the user added usage credits)

The user had stored the `anthropic` secret in `sbx` as OAuth (`sbx secret ls`: `anthropic (oauth configured)`), so
Pi in a sandbox uses the user's Claude subscription, not an API key; the earlier `400` was the subscription's extra
usage running out.

1. `sbxm rm s15-base --harness pi` (sbxm still held a record of the sandbox removed with `sbx rm`), then `sbxm new
   s15-base --profile s15-pi --harness pi`: created (Pi kit step 1.0 s, jq step 6.0 s). `pi -p '<question>'`:
   `MEDLAR-15`. In it: `~/.pi/agent/auth.json` `anthropic.type` is `oauth`, `access` and `refresh` 26 characters
   each (placeholders); `ANTHROPIC_API_KEY` is set, 13 characters (a placeholder, as slice 0 found for `codex`).
2. `sbx stop`, `sbx template save sbxm-s15-base-pi sbxm-spike-s15:pi-2`, `sbx rm --force sbxm-s15-base-pi`; `sbx
   create --name sbxm-spike-s15-narrow -t sbxm-spike-s15:pi-2 --pull never ...` with the narrow `common` kit as in
   question 4: only the Pi kit's step ran (1.1 s). `pi -p '<question>'`: `MEDLAR-15`. Then `sbx rm --force
   sbxm-spike-s15-narrow` and `sbx template rm --force sbxm-spike-s15:pi-2`.

Pi calls in all: 4 of the spec's 6.

## Timings

| What | Runs | Result |
|---|---|---|
| `sbxm new` from scratch (image cached) | 1 | about 19.6 s (jq step 8.8 s) |
| `sbx template save` | 1 | about 5.5 s |
| `sbx create -t` with the original kits | 1 | about 24.9 s (steps 0.9 s + 3.6 s) |
| `sbx create -t` with the narrow kit | 1 | about 93.8 s between clock reads; not trusted (Conditions) |

Unlike S11 (`claude`) and slice 0 (`codex`), these single, rough runs show no speed-up from the template for Pi; a
fair comparison needs repeated runs with the command itself timed (Follow-ups).

## Disk

| Item | Size |
|---|---|
| C: free at the start / before the save / after the save | 30.7 / 30.25 / 30.18 GiB |
| Template reported size | 734 MB |
| C: free after cleanup | 30.42 GiB (the Pi base image `docker.io/sbx/pi-image:latest`, pulled by the first create, stays) |

## Follow-ups

- Time `sbx create -t` for Pi with the command itself timed, three runs each, to see whether a template saves time
  for Pi at all (the Pi image is small and its kit step takes about 1 s).
- `sbxm new` warns that Pi sandboxes don't support `skills.store = "readonly"`; templates don't change that.

## Cleanup

- Before the spike: `sbx ls` empty; `sbx template ls`: `claude-code-docker`, `codex-docker`, `antigravity-image`;
  `sbx secret ls` names: `google`, `anthropic`, `openai`. C: 30.7 GiB free.
- After cleanup: `sbx ls` empty; no `sbxm-spike-s15` tag; secret names the same. `sbx template ls` also lists
  `docker.io/sbx/pi-image:latest`, the Pi base image `sbx` pulled for the first create (outside the spike's prefix,
  so left in place). C: 30.42 GiB free.
- `H:\sbxm-spike-s15\` (config, kits, workspaces; no sandbox state) is **not removed yet**: the removal was
  refused by the assistant's tool as a protected path, so it is left for the user.
