# Milestone 1: sandbox lifecycle

Goal: `sbxm` creates, reuses, lists, stops and removes per-project Docker Sandboxes built from a shared, versioned config. Comparisons, runs and evals are milestone 2.

Constraints come from `decisions.md` (numbers in brackets refer to it). Verified `sbx` behavior is in its "Spike results" section.

## Plan-level choices

Confirmed 2026-09-24 as decisions 40–42. P3 changed: the directory is renamed to `sbxm/`; git is already initialized.

**P1. Where sbxm metadata lives (changes the paths in [5], [24], [27])**
The sandbox mounts `<base>/<project>` read-write. Anything sbxm keeps inside that directory (project `sandbox.toml`, state, later `runs/`) can be read by the agent, and **edited by the agent, which could widen the next sandbox's egress**. `sbx env` has this problem too and solves it with read-only binds.
→ Proposal: the workspace stays `<base>/<project>` (as in `idea.md`, and existing folders work unchanged). All sbxm metadata goes in `<base>/.sbxm/<project>/` (`sandbox.toml`, `state.json`, generated kits, and in M2 `runs/`). That directory is never mounted. Project names can't start with `.`, so it can't collide with a project.

**P2. Sandbox naming**
→ Proposal: `sbxm-<project>-<harness>`, so one project can have a Claude sandbox and a Codex sandbox side by side, and `list` can filter on the `sbxm-` prefix. Names stay within `sbx`'s rules (letters, digits, `-`, `.`).

**P3. Version control for this repo**
`model_compare/` isn't a git repo. → Proposal: `git init` at scaffold time.

## Crate layout

Single binary crate `sbxm`. Modules, in dependency order:

| Module | Responsibility |
|---|---|
| `project` | Name validation [33] (`^[a-z0-9][a-z0-9-]*$`, max length, reject `default` and Windows reserved names) and path resolution (workspace, `.sbxm/<project>/`, sandbox name). |
| `config` | TOML schema, loading, layering global → profile → project [27], canonical serialization, SHA-256 config hash [28]. |
| `harness` | `Harness` trait with one adapter per harness: `claude`, `codex`, `gemini` (built-in `sbx` agents) and `pi` (pinned kit ref). Each adapter owns the `sbx` agent/kit reference, the path of its mandatory-instructions file [37], and native hook/config file placement. |
| `kit` | Serde structs for the **v2** kit schema (only the fields we emit) plus a generator: merged config + harness → `common` mixin + `harness-<h>` mixin (`requires.agent`) [36][39]. The only module that knows the kit format. |
| `backend` | `SandboxBackend` trait [17]; `SbxBackend` (shells out to `sbx`, parses `--json`); `FakeBackend` for tests [35]. |
| `state` | `state.json` per project: sandbox name, harness, profile, config hash, created time, `sbx` version, kit paths. Needed because `sbx ls --json` doesn't expose kits or env. |
| `commands` | One file per subcommand. |
| `cli` | `clap` derive definitions. |

Dependencies: `clap` (derive), `serde`, `toml`, `serde_json`, a *maintained* serde YAML crate (`serde_yaml` is archived; pick one at scaffold), `sha2`, `semver`, `anyhow`, `thiserror`, `dirs`. Dev: `insta` (snapshot tests of generated `spec.yaml`), `assert_cmd`, `tempfile`.

## Config

**Global** `~/.config/sbxm/config.toml` (on all platforms; override with `SBXM_CONFIG_DIR`):
```toml
base_dir = 'E:\sbxm-projects'
profiles_dir = 'E:\sbxm-config\profiles'   # the versioned common-config repo [28]
default_profile = "default"
default_harness = "claude"
min_sbx_version = "0.43.0"

[resources]            # sbx defaults to all CPUs / 16 GiB, so be explicit
cpus = 4
memory = "8g"
```

**Profile** `<profiles_dir>/<name>/profile.toml`, plus the files it references:
```toml
description = "Default profile"

[network]
allow = ["github.com", "*.githubusercontent.com", "registry.npmjs.org"]
deny  = []

[env]
EXAMPLE_FLAG = "1"

[secrets]
services = ["anthropic", "github"]   # must exist in `sbx secret ls --json` [30]

[instructions]
mandatory = "instructions/mandatory.md"  # always loaded, per-harness user-level file [37]
reference = "instructions/reference.md"  # kit agentInstructions, read on demand

[[setup.install]]
command = "echo hello"
user = "1000"
description = "example"

[harness.claude]
home_files = "harness/claude/home"   # copied to kit files/home/ (e.g. .claude/settings hooks)
[harness.codex]
[harness.gemini]
[harness.pi]
kit = "git+https://github.com/docker/sbx-kits-contrib.git#ref=<40-hex-sha>&dir=pi"

[skills]
store = "readonly"   # sbx skills store mount: "readonly" or "off"; "readwrite" is rejected [46]
```

**Project** `<base>/.sbxm/<project>/sandbox.toml` (optional): same keys as a profile, merged on top. Lists (`network.allow`, `secrets.services`, `setup.install`) are *appended*; scalars and maps override.

**Hash:** SHA-256 over the canonical JSON of the merged config, plus the *contents* of referenced files, plus sbxm's version. It's stored in `state.json` and also injected into the sandbox as `SBXM_CONFIG_HASH` / `SBXM_PROFILE` env vars (verified to work in S2), so every sandbox records where it came from.

## Kit generation

Output: `<base>/.sbxm/<project>/kits/<hash-prefix>/{common,harness-<h>}/spec.yaml` (+ `files/home/`). Deterministic: the same hash gives byte-identical output.

| Config | `common` mixin (any harness) | `harness-<h>` mixin (`requires.agent: <h>`) |
|---|---|---|
| `network.allow/deny` | `permissions.network` | none |
| `env` + hash vars | `environment.variables` | none |
| `setup.install` | `setup.install` | none |
| `instructions.reference` | `agentInstructions.content` | none |
| `instructions.mandatory` | none | `files/home/<adapter's user-level file>` |
| `harness.<h>.home_files` | none | copied into `files/home/` |

`skills.store` isn't a kit field: it becomes `sbx create --skills <value>` [46].

Unsupported feature for a harness (e.g. hooks configured, adapter has no hook location) → **loud warning** [11], never silently dropped.

Each generated kit goes through `sbx kit validate --json` before `sbx create`. A validation failure aborts the command.

## Commands

| Command | Behavior |
|---|---|
| `sbxm config init` | Writes a starter global config and `default` profile. Refuses to overwrite. |
| `sbxm new <project> [--harness h] [--profile p] [--seed dir]` | Validate name → create `<base>/<project>` if missing (copy `--seed` contents if given; existing dir is reused as-is and `--seed` is then an error) → merge config → check secrets → generate + validate kits → `sbx create --name … --cpus … -m … --kit common --kit harness-<h> <agent> <workspace>` → write state. Doesn't attach. |
| `sbxm open <project> [--harness h] [--rebuild]` | [31]: running → attach (`sbx run --name`); stopped → attach (`sbx run` restarts it); missing → `new` then attach. If the current hash ≠ the stored one → warn and refuse unless `--rebuild`. `--rebuild` = `sbx rm -f` + create. The workspace is kept, **but harness session history (kit volumes) is lost**, and the command says so. |
| `sbxm list [--json]` | `sbx ls --json` filtered to `sbxm-*`, joined with state: project, harness, profile, status, hash drift (✓/changed), orphans (state without a sandbox, or a sandbox without state). |
| `sbxm stop <project> [--harness h]` | `sbx stop`. |
| `sbxm rm <project> [--harness h] [--purge]` | `sbx rm -f` + delete state/kits. `--purge` also deletes `<base>/<project>` and `.sbxm/<project>`, after an interactive confirmation that shows the path. Refuses `--purge` when not attached to a terminal unless `--yes` is also given [32]. |
| `sbxm config show [project] [--profile p] [--harness h] [--kits]` | Prints the merged TOML and hash. `--kits` prints generated `spec.yaml`s without creating anything. |
| `sbxm doctor` | Checks: `sbx` on PATH and ≥ `min_sbx_version`; daemon reachable (`sbx ls --json` succeeds); base dir exists, is writable and **not under `%TEMP%`/AppData** (S1); free disk space; config and profiles parse; referenced files exist; required secrets exist; generated kits pass `sbx kit validate`. Exit code is non-zero on any failure. |

Backend note: never use `sbx exec` for status or inspection. It **starts stopped sandboxes** (S1).

## Build order: vertical slices

Follow the `sdlc-implementation` skill. Each slice is one user-observable behavior built end to end (CLI → domain → backend), test-first, in the smallest steps, with a commit after every green step. Modules from the crate layout table **grow slice by slice**; nothing is built ahead of the slice that needs it.

Every slice is tested against `FakeBackend`. "Real `sbx`" means an `#[ignore]` test or manual check, run when the slice is done, on a base dir outside `%TEMP%`/AppData.

Between slices 3 and 10b, sandboxes are created without sbxm kits. That's safe: `sbx`'s global deny-all egress still applies (S3).

| # | After this slice… | Test focus | Real `sbx` |
|---|---|---|---|
| 0 | **Scaffold.** `sbxm --version` and `sbxm --help` work. `rustup component add clippy rustfmt`, `cargo init --name sbxm` in the renamed `sbxm/` dir, `.gitignore`. First commit includes the planning docs (repo already initialized [42]). Update CLAUDE.md with the real commands. | `assert_cmd` on `--version`. | none |
| 1 | `sbxm config init` writes a starter global config and `default` profile to `SBXM_CONFIG_DIR`, and refuses to overwrite. | Files created with expected keys; second run errors and leaves files unchanged. | none |
| 2 | `sbxm new <name>` rejects invalid names with a clear error and non-zero exit, and touches nothing. | One case per rule [33]: `..`, `/`, `\`, `con`, `nul`, `com1`, `default`, leading `.`/`-`, uppercase, too long. No dirs created, no backend calls. | none |
| 3 | `sbxm new demo` creates `<base>/demo` (or reuses it), calls `sbx create` for `sbxm-demo-claude` with workspace and `--cpus`/`-m`, and writes `.sbxm/demo/state.json` [40][41]. | Backend call recorded with exact args; existing dir reused and its contents kept; missing base dir → clear error; state contents. | Sandbox appears in `sbx ls`. |
| 4 | `sbxm new demo --seed <dir>` copies the seed into a new project; `--seed` on an existing project is an error. | Files copied; error path creates nothing. | none |
| 5 | `sbxm list` shows sbxm sandboxes with project, harness and status, and flags orphans. | Parse the captured `sbx ls --json` shape; non-`sbxm-` sandboxes hidden; orphans both ways. | Matches `sbx ls`. |
| 6 | `sbxm stop demo` stops the sandbox. | Backend call; unknown project → clear error. | Status becomes stopped. |
| 7 | `sbxm rm demo` removes the sandbox and its state, and keeps the workspace. | Backend `rm -f` call; state gone; workspace untouched. | Gone from `sbx ls`. |
| 8 | `sbxm rm demo --purge` also deletes the workspace and `.sbxm/demo` after confirmation. | Confirm yes/no; non-TTY without `--yes` refuses; prompt shows the path [32]. | none |
| 9 | `sbxm open demo` attaches if the sandbox exists (running or stopped) and creates then attaches if it doesn't [31]. | Backend call sequence for each of the three states. | Manual attach works. |
| 10a | `sbxm new demo [--profile p]` loads `<profiles_dir>/<p>/profile.toml` (default `default_profile`); a missing or invalid profile is a clear error and nothing is created. | Profile parsed; missing/invalid profile errors with no dirs and no backend calls. | none |
| 10b | The profile's `network` reaches the sandbox through a generated `common` mixin, validated before create. | `insta` snapshot of `spec.yaml`; `kit_validate` called before `create`; validation failure aborts with no sandbox created. | `sbx kit validate` passes; allowed host 200, other 403. |
| 10c | The profile's `env` reaches the sandbox through the same `common` mixin. | Snapshot with `environment.variables`. | Env var visible in the sandbox. |
| 10d | The profile's `skills.store` becomes `--skills` on `sbx create`; `readwrite` is rejected with a clear error [46]. | Backend call args for `readonly`/`off`/default; `readwrite` error with no sandbox created. | Store skills visible with `readonly`, absent with `off`. |
| 11 | The config hash is stored in state and injected as `SBXM_CONFIG_HASH`/`SBXM_PROFILE`. After a profile change, `open` refuses; `open --rebuild` recreates, warning that session history is lost; `list` shows drift [28][31]. | Hash stable across runs; changes when a referenced file changes; refuse/rebuild/drift paths. | Env vars present in sandbox; workspace survives rebuild. |
| 12 | `.sbxm/demo/sandbox.toml` overrides the profile: lists append, scalars and maps override [27]. | Merge semantics; reflected in kit snapshot and hash. | none |
| 13 | `new`/`open` fail clearly when a profile's `secrets.services` entry isn't stored in `sbx` [30]. | Parse the captured `sbx secret ls --json` shape; message names the missing secret; no sandbox created. | Missing vs. present `anthropic`. |
| 14 | *(Run spike S6b first.)* Instructions: `reference` → `agentInstructions` in the common mixin; `mandatory` → `harness-claude` mixin (`requires.agent: claude`) at `files/home/.claude/CLAUDE.md` [37]. | Snapshots of both kits; both `--kit` args in order. | Mandatory file present in the sandbox and loaded by Claude. |
| 15 | `setup.install` and `harness.claude.home_files` reach the sandbox; configuring a feature the harness adapter can't support warns loudly [11]. | Snapshots; warning text. | Install ran; home file present. Hooks route per S6b: the Claude kit replaces a mixin's `settings.json` [49]. |
| 16 | `sbxm config show [project] [--kits]` prints the merged config, hash and generated kits without creating anything. | Output snapshot; zero backend create calls. | none |
| 17 | `sbxm doctor` reports every check in the Commands table and exits non-zero on any failure. | One test per check, pass and fail (incl. base dir under AppData, `sbx` too old, Pi needed but `github.com/docker/` not in `kit.allowedSources` [48]). | Clean on a correct setup. |
| 18 | `sbxm new demo --harness codex` works, including mandatory instructions. | Adapter snapshot. | Mandatory file at `~/.codex/AGENTS.md` [49]; confirm with `codex debug prompt-input` (no credentials needed). |
| 19 | Same for `--harness gemini`. | Adapter snapshot. | Mandatory file at `~/.gemini/GEMINI.md` [49]; loading unverified without Google credentials. |
| 20 | Same for `--harness pi`, using the Pi kit ref pinned to a commit SHA. If `kit.allowedSources` doesn't allow the kit, fail before creating anything with a message saying how to allow it [48]. | Adapter snapshot; pinned-ref validation; allowlist error text. | Needs `github.com/docker/` allowed; Pi paths from S6b. |
| 21 | **End-to-end check (manual, real `sbx`)**, see below. | none | All items pass. |

**End-to-end check (slice 21)**, on a base dir outside AppData:
    - `config init` → `doctor` is clean.
    - `new demo` → sandbox exists, `SBXM_CONFIG_HASH` set, mandatory instructions file present, allowed host returns 200 and a non-listed host 403 (`sbx policy log`).
    - `open demo` attaches. Edit the profile → `open demo` refuses → `open demo --rebuild` recreates it, and workspace files survive.
    - `list` shows drift and status. `stop`, `rm`, `rm --purge` behave as specified.
    - Repeat `new` with `--harness codex`, `gemini`, `pi`.

## Out of scope for M1

Comparisons, runs, `--repeat`, evals, Jev, headless `exec` (all M2); `sbx env` [38]; `bollard` [17]; profile-owned skills (`skills.dir`, after spike S6) [46]; cloud sandboxes; config-variant profiles in comparisons (the profile mechanism built here is what M2 will reference [10]).

## Risks

- **v2 kit format and `sbx kit` are experimental** [39]. Mitigation: one module, snapshot tests, `sbx kit validate` gate, and a `min_sbx_version` check.
- **`files/home/` collides with files the harness kit writes at install.** Verified in S6 [49]: the Claude kit replaces `~/.claude/settings.json`, while `~/.claude/CLAUDE.md` and skills survive. Hooks need another route, found in S6b before slice 14.
- **Rebuild loses harness session history.** It's shown to the user; `sbx kit add` (which keeps volumes) could be investigated later.
