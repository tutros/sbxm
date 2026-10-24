# Milestone 4, P2: the cloud capture run

`sdlc/milestone-4.md` P2 (a), before slice 1. Run 2026-10-24, about 08:00-08:06 UTC, by hand in the main session,
`sbx` v0.46.0 (`991967dc`), Windows host, `--cloud`. One default-shape `shell` sandbox, `sbxm-p2-capture`, with
`--ttl 1h --on-timeout delete`, alive for about 4 minutes, and at 08:15 a second, `sbxm-p2-shape` (medium shape,
`--ttl 1h30m`), alive for under a minute. Everything is observed unless marked **inferred**. The raw
output is in `src/backend/fixtures/sbx-cloud-*`, unchanged.

## Fixtures

| File | Command |
|---|---|
| `sbx-cloud-ls-empty.json` | `sbx --cloud ls --json`, no sandboxes |
| `sbx-cloud-ls.json` | `sbx --cloud ls --json`, one running sandbox (`name`, `id`, `agent`, `status`, `cpus`, `memory_mib`, `image`, `created_at`, `expires_at`) |
| `sbx-cloud-policy-ls.json` | `sbx --cloud policy ls <name> --json` (`allowNetworks`, `denyNetworks`, `mode`) |
| `sbx-cloud-policy-ls-global.json` | `sbx --cloud policy ls --json`, the account default |
| `sbx-cloud-secret-ls.json` | `sbx --cloud secret ls --json`: two global OAuth logins and one sandbox-scoped custom secret |
| `sbx-cloud-secret-set-custom.txt` | `sbx --cloud secret set-custom --host H --env E --value V --sandbox <name>` |
| `sbx-cloud-create.txt` | `sbx --cloud create --name <name> --ttl 1h --on-timeout delete --cpus 2 --memory 4g --allow-network H shell` |
| `sbx-cloud-create-skills-refused.txt` | the same with `--skills off` |
| `sbx-cloud-create-shape-refused.txt` | `sbx --cloud create ... --cpus 3 --memory 5g shell`, not a billable shape |
| `sbx-cloud-cp-not-found.txt` | `sbx --cloud cp <name>:<missing path> <host file>` |
| `sbx-cloud-cp-symlink-refused.txt` | `sbx --cloud cp <name>:<a symlink> <host file>` |
| `sbx-cloud-rm-not-found.txt` | `sbx --cloud rm <name> -f` for a sandbox already removed |
| `sbx-cloud-secret-rm-not-found.txt` | `sbx --cloud secret rm <service> --sandbox <name> -f` for a secret already removed |

## The open points P2 named

1. **Name or id:** `exec`, `cp` (both directions), `policy ls` and `rm` all accept the sandbox's name as well as its
   `sbx_...` id. `create` prints the id, and `cp`'s output names the id even when given the name.
2. **`exec -w`:** accepted with `--cloud` (`-w /tmp` printed `/tmp`). The default working directory is
   `/home/agent/workspace`, as user `agent`.
3. **`--skills`:** refused with `--cloud`, before anything is created: "--skills is not supported with --cloud; the
   skills store is backed by a host-side directory". sbxm must not pass it to a cloud create.
4. **`--ttl`:** the help says a Go-style duration ("e.g. 30m, 2h, 1h30m; units are case-insensitive").
   `--ttl 1h30m` was accepted: `ls` showed `expires_at` 90 minutes after `created_at` (less the 5 s of the create).
   `--on-timeout restart` needs a TTL of at least 1h.
5. **Shape:** `--cpus` and `--memory` pick the shape, and only five are accepted. `--cpus 3 --memory 5g` was refused
   before anything was created, naming them: micro (1 vCPU / 2048 MiB), small (2 / 4096), medium (4 / 8192), large
   (8 / 16384), xl (16 / 32768) (`sbx-cloud-create-shape-refused.txt`). `--cpus 4 --memory 8g` gave `cpus: 4`,
   `memory_mib: 8192` in `ls`; `--cpus 2 --memory 4g` gave the default, small. Without the flags a sandbox is small.
   (S14's "8 vCPU, 15 GiB" large shape was its in-sandbox reading; the shape is 16384 MiB.)
6. **Does `rm` remove the sandbox's scoped secrets?** **No.** After `rm`, `secret ls` still listed the custom secret
   scoped to the removed sandbox's name. Locally it goes with the sandbox (S1). sbxm must remove every scoped secret
   itself after the sandbox, on every path (slice 4); an already-removed secret answers `sbxcloud: not found`, exit 1.

## Also observed

- The scoped secret set before `create` reached the sandbox as its placeholder (`P2_FAKE=sbx-cs-...`), as in S14 Q2.
- `--allow-network postman-echo.com` with no kit: `policy ls` listed only that host, with `mode: deny-all`, not the
  `shell` agent's built-in provider list S17 saw when a kit was given. Why the two differ is **not known**; the
  strict check (slice 7) must read whatever list is there either way.
- Egress: the allowed host answered 200; `example.com` failed with `curl` exit 7 (connection refused), one of S14's
  three refusal shapes.
- `cp` out refuses a symlink in the cloud ("download does not read through a symlink", exit 1), and copies a folder
  whole ("Downloaded 1 file(s)"). sbxm's host-side check that a staged file is one regular file under its cap
  (slice 1) is still needed, for the folder case.
- `rm` took 3 s and did not need a retry this time.

## Not captured (moved to slice 9's real test, decision 267)

- **The not-logged-in message:** capturing it needs `sbx logout`, which signs the user out of both OAuth logins in
  the cloud. Left for slice 9's real test or a moment the user chooses.
- **`rm`'s "temporarily unavailable" error:** it can't be triggered on demand. S14 recorded its words ("temporarily
  unavailable; retry the delete"); slice 3's fixture uses that text until a real one turns up.

Spend: not measured; a small sandbox for about 4 minutes and a medium one for under a minute (**inferred**: cents).
