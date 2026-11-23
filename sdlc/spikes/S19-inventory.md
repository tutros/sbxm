# S19 inventory: sbxm's `sbx` calls against sbx 0.48-0.54

Gathered 2026-11-22 by reading the code at main `af7edaa` against the `sbx` release notes for 0.47.0-0.54.0 (https://github.com/docker/sbx-releases/releases). Nothing was run against `sbx` 0.54; spike S19 [336] settles the MAYBE items. Planning round 1 of milestone 6 (issue #740, decisions 334-343).

## AFFECTED (most severe first)

1. **v2 mixins with v3 built-in agents (0.51).** `src/kit.rs` writes `schemaVersion "2"` mixins (`sbxm-common`, `sbxm-harness-<agent>`), passed as `--kit` to the built-in `claude` and `codex` agents (`src/harness.rs`, `agent_arg`). 0.51 resolves built-in agents to published v3 kits (`docker.io/sbx/<name>:latest`) and refuses to combine v1/v2 kits with v3 ones: every create is expected to fail, local and cloud. Whether the pinned `pi-kit` and `antigravity-kit` tags are v2 or v3 is unknown.
2. **`sbx ls --json` returns a bare array (0.48).** `src/backend/sbx.rs` parses `{"sandboxes": [...]}` (fields `name`, `agent`, `status`, `cpus`, `memory_mib`, `expires_at`, `created_at`); fixtures `src/backend/fixtures/sbx-ls.json`, `sbx-cloud-ls.json`, `sbx-cloud-ls-empty.json`; `tests/real_sbx.rs` reads `ls["sandboxes"]` in two places. Breaks list, status, doctor, finish, cloud lifetimes and the cloud sign-in check.
3. **`sbx template ls --json` returns a bare array (0.48).** `src/backend/sbx.rs` parses `{"images": [...]}` (`id`, `repository`, `tag`, `flavor`, `created_at`, `size`); fixture `sbx-template-ls.json`. Breaks every template command, doctor and the stale-lock check.
4. **Jev's `shell` agent becomes a Debian 13 v3 kit (0.51).** `src/eval/jev.rs` creates its evaluator from agent `shell` plus its own kits: the same kit-version clash as item 1, if those kits are v2.
5. **Sandboxes and templates made on 0.46 must be recreated (0.51).** The template hash doesn't change on an `sbx` upgrade, so nothing in sbxm notices.
6. **Cloud TTL over 24 hours is refused (0.54).** `src/task/lifetime.rs` (`worker_ttl`: time limits plus fix rounds plus 30 minutes) has no cap; passed by `sbx create --ttl`.
7. **A fixture names a retired image.** `sbx-cloud-ls.json` names `docker/sandbox-templates:shell-docker`, no longer published (0.51). Fixture only: nothing parses `image`.

## MAYBE (for S19)

- The JSON shapes of `secret ls --json` (local and cloud), `policy ls --json` (local `rules[]`; cloud camelCase `allowNetworks`/`denyNetworks`), `skills ls --json` and `kit validate --json` (0.48 moved many outputs to bare arrays and snake_case; these aren't listed).
- `secret set-custom --env` (0.48 renamed `--env` to `--env-name` for `secret ls` and `secret rm`; `set-custom` isn't listed), local and cloud.
- `SandboxInfo.agent` and the template `flavor` values once agents are v3 kit references.
- The stderr texts sbxm matches on: "Not authenticated to Docker", "temporarily unavailable", "sbxcloud: not found", "no custom secret found", `error: path .. not found`, and the `rm` not-found text.
- `create` and `run` with stdin inherited and no terminal (the loop's child tasks): 0.48's terminal rules, and 0.54's kit permission re-confirmation with nobody at the terminal.
- `sbx run --name` (`sbxm open`): 0.53 keeps the image's entrypoint arguments.
- `tests/real_sbx_cloud.rs` and the docs ask for `sbx logout` by hand, which needs a confirmation from 0.53.

## OK

`version --json`; `stop`; `exec` (no `--detach` or `--privileged`); `template save`; `template rm --force`; `--cloud policy deny network`; `scripts/loop-trigger.ps1`'s plain `sbx ls`. sbxm doesn't call `ports`, `logout`, `diagnose`, `mcp ls`, `run --detached`, `create -q`, `--yes` or `--format`, and pipes no `y` into `sbx`.

## Version floors today

`config.min_sbx_version` defaults to 0.43.0 (`src/config.rs`, `src/commands/config_init.rs`, checked in `src/commands/doctor.rs`); `template::MIN_SBX_VERSION` 0.46.0 (`src/template.rs`); the cloud minimum 0.46.0 (`src/task/pipeline.rs`); Jev's `MIN_SBX_VERSION` 0.46.0 (`src/eval/jev.rs`); the fake backend reports 0.43.0. No maximum or tested-up-to check.
