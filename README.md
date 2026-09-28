# sbxm

`sbxm` creates and manages per-project Docker Sandboxes (`sbx`) from one
shared, versioned config. Each project gets a folder on your machine, and each coding agent (Claude Code, Codex,
Gemini CLI or Pi) gets its own sandbox for it, built with your network allowlist, environment, secrets, instructions
and setup steps.

sbxm enforces nothing itself: `sbx` does the isolation, the deny-by-default egress proxy and secret injection. sbxm
turns your config into `sbx` kits, passes them to `sbx create`, and remembers what each sandbox was built from, so it
can tell you when the config has changed since.

## Requirements

- [Docker Desktop](https://www.docker.com/products/docker-desktop/) with `sbx` **0.43.0 or newer**, logged in
  (`sbx login`).
- The secrets your agents need, stored in `sbx` (sbxm only names them): for example `sbx secret set anthropic`.
- [Rust](https://rustup.rs/) to build sbxm. [`just`](https://github.com/casey/just) is optional, for the recipes in
  the `justfile`.
- A base folder for projects that `sbx` can mount. Not under `%TEMP%` or `AppData`. On the machine sbxm was built on,
  workspaces on `C:` failed to mount, so projects live on `E:`.

sbxm is developed on Windows 10 with PowerShell; paths are handled portably, but other platforms are untested.

## Install

```
cargo install --path .
```

or `just install`. Without installing, run it from this folder as `cargo run -- <command>` (the `justfile` recipes do
that).

## Quick start

1. Write a starter config and a `default` profile:
   ```
   sbxm config init
   ```
   This creates `~/.config/sbxm/config.toml` and `~/.config/sbxm/profiles/default/profile.toml`
   (`%USERPROFILE%\.config\sbxm` on Windows; set `SBXM_CONFIG_DIR` to use another folder). It never overwrites.
2. Open `config.toml` and set `base_dir` to the folder your projects should live in, e.g. `'E:\sbxm-projects'`.
   Create that folder.
3. Check everything:
   ```
   sbxm doctor
   ```
   Every line should start with `ok`. Each `FAIL` line says what's wrong and how to fix it.
4. Create a project and its Claude Code sandbox, then attach to it:
   ```
   sbxm new demo
   sbxm open demo
   ```
   The workspace is `<base_dir>\demo`, mounted read-write into the sandbox.

## Commands

| Command | What it does |
|---|---|
| `sbxm config init` | Writes a starter `config.toml` and `default` profile. Refuses to overwrite either. |
| `sbxm new <project> [--harness h] [--profile p] [--seed dir]` | Creates `<base_dir>/<project>` if missing (or reuses it), builds the kits from the profile and the project's `sandbox.toml`, checks them with `sbx kit validate`, and creates the sandbox. Doesn't attach. Refuses a harness that already has a sandbox for the project, before writing anything; open that one with `sbxm open <project> [--harness h]` (or `--rebuild` it). `--seed` copies a folder into a *new* project. |
| `sbxm open <project> [--harness h] [--rebuild]` | Attaches to the sandbox, starting it if it's stopped and creating it if it's missing. Refuses if the config changed since the sandbox was built; `--rebuild` recreates it. |
| `sbxm list [--json]` | Lists sbxm's sandboxes with project, harness, status and whether their config is `current` or `changed`. Flags orphans (a sandbox sbxm has no record of, or a record without a sandbox) and says how to fix each. |
| `sbxm stop <project> [--harness h]` | Stops the sandbox. |
| `sbxm rm <project> [--harness h]` | Removes the sandbox and sbxm's record of it. The workspace is kept. |
| `sbxm rm <project> --purge [--yes]` | Removes **every** sandbox of the project, then deletes the workspace and its metadata, after you confirm the exact paths. Without a terminal it needs `--yes`. Can't be combined with `--harness`. |
| `sbxm config show [project] [--profile p] [--harness h] [--kits]` | Prints the merged config exactly as it's hashed, the hash, and with `--kits` the generated kits. Creates nothing. |
| `sbxm doctor` | Checks `sbx` (on `PATH`, new enough, daemon answering), the config, every profile, every project with each of its sandboxes (secrets stored, kits valid), and the base dir (exists, writable, not a temp folder, at least 10 GiB free). Exits non-zero if anything fails. |

`--harness` defaults to `claude` everywhere. `sbxm <command> --help` shows every option.

### The `justfile`

`just` lists the recipes: `just new demo codex`, `just open demo codex`, `just rebuild demo`, `just show demo pi`,
`just demo` (one project with all four harnesses), `just purge demo`, plus `just check` and `just real-test` for
development.

## Projects, sandboxes and where things live

- **Project names** use lowercase letters, digits and `-`, at most 40 characters, and can't be `default` or a
  reserved Windows name such as `con` or `com1`.
- **Workspace:** `<base_dir>/<project>`, shared by all of the project's sandboxes.
- **Sandboxes** are named `sbxm-<project>-<harness>`, so a project can have one per harness side by side.
- **sbxm's metadata** lives in `<base_dir>/.sbxm/<project>/`: `state.json` (which sandbox was built from which
  profile and config hash), the generated kits, and your optional `sandbox.toml`. This folder is never mounted, so an
  agent can't read or change its own config.

## Harnesses

| `--harness` | Agent | Mandatory instructions file | Notes |
|---|---|---|---|
| `claude` (default) | Claude Code | `~/.claude/CLAUDE.md` | Supports `harness.claude.home_files` and `harness.claude.managed_settings`. |
| `codex` | Codex | `~/.codex/AGENTS.md` | |
| `gemini` | Gemini CLI | `~/.gemini/GEMINI.md` | `sbx`'s skills store doesn't serve Gemini: sbxm warns unless `skills.store = "off"`. |
| `pi` | Pi | `~/.pi/agent/AGENTS.md` | Uses the Pi kit from Docker Hub, pinned to a fixed tag. See below. |

Settings a harness can't use are never dropped silently: `sbxm new`/`open` print a `warning:` line for each, e.g.
`harness.claude.managed_settings` on a Codex sandbox.

**Pi, first time:** the Pi kit asks `sbx` for your `anthropic` credential, and `sbx` wants your approval once. Run
your first `sbxm new … --harness pi` in a terminal and approve when asked (`This kit wants to use these credentials:
… [A]pprove all · [R]eview each · [N]o`). `sbx` saves the approval (on Windows in `%APPDATA%\sbx\credentials.yaml`).
Without it, the sandbox starts but every model call fails with `401`.

**Pi and reference instructions:** Pi reads instruction files from every parent folder, so `instructions.reference`
is always in Pi's context instead of on demand; sbxm warns about it.

## Configuration

Three layers, merged in order: the global `config.toml`, a named **profile**, and an optional per-project
`sandbox.toml`. Unknown keys are errors in all three, so a typo never silently falls back to a default.

### `config.toml`

```toml
base_dir = 'E:\sbxm-projects'                          # where projects live
profiles_dir = 'C:\Users\you\.config\sbxm\profiles'    # default: <config dir>/profiles; keep it in git
default_profile = "default"                            # used when --profile isn't given
min_sbx_version = "0.43.0"                             # checked by `sbxm doctor`

[resources]            # per sandbox; sbx would otherwise take all CPUs and 16 GiB
cpus = 4
memory = "8g"
```

There's no `default_harness` setting: use `--harness`.

### Profiles: `<profiles_dir>/<name>/profile.toml`

Pick one with `--profile <name>` (1–40 lowercase letters, digits and `-`, starting with a letter or digit). Every key
is optional.

```toml
description = "Strict profile"          # for you; not part of the config hash

[network]                               # egress is deny-by-default
allow = ["github.com", "*.githubusercontent.com", "registry.npmjs.org"]
deny = []

[env]                                   # set inside the sandbox
RUST_LOG = "info"

[secrets]                               # must be stored in `sbx` (global service secrets)
services = ["anthropic", "github"]

[instructions]                          # paths relative to this profile's folder
mandatory = "instructions/mandatory.md" # always loaded, in each harness's own file (table above)
reference = "instructions/reference.md" # background material the agent can look up

[[setup.install]]                       # run once when the sandbox is created
command = "apt-get install -y ripgrep"
user = "0"                              # root when unset
description = "install ripgrep"

[skills]
store = "readonly"                      # sbx's shared skills store: "readonly" (default) or "off"

[harness.claude]                        # Claude Code only
home_files = "claude-home"              # folder copied into the sandbox's home directory
managed_settings = "managed-settings.json"
```

Rules sbxm checks when a profile loads (before anything is created):

- Env names are shell identifiers and can't start with `SBXM_` (sbxm sets `SBXM_CONFIG_HASH` and `SBXM_PROFILE` in
  every sandbox). Env values can't contain `${{`.
- Every file or folder path must be relative and stay inside the profile's folder, and must exist.
- `skills.store = "readwrite"` is refused: an agent could plant skills that every other sandbox then loads.
- `home_files` may not contain links, `.claude/settings.json` (the Claude kit replaces it; use `managed_settings`),
  or `.claude/CLAUDE.md` while `instructions.mandatory` is set.
- `managed_settings` must hold a JSON object in Claude Code's settings format, e.g. hooks or permission rules. It's
  written to `/etc/claude-code/managed-settings.json`, which takes precedence over settings the agent can edit. It's
  a strong default, not a lock: the agent has passwordless `sudo` in the Claude image.

### Per-project overrides: `<base_dir>/.sbxm/<project>/sandbox.toml`

Same keys as a profile; paths are relative to that folder. Merged over the profile:

- lists are **appended** (`network.allow`, `network.deny`, `secrets.services`, `setup.install`),
- maps are merged per key, the project winning (`env`),
- single values replace the profile's when set (`skills.store`, instruction files, `home_files`,
  `managed_settings`).

A project can add to what its profile allows, never remove from it.

## Config changes, drift and `--rebuild`

Every sandbox records a hash of what it was built from: harness, profile name, the merged profile and project
settings, the *contents* of referenced files, resources and sbxm's version. A setting that doesn't reach a harness
(e.g. Claude-only settings for a Codex sandbox) isn't part of that harness's hash.

- `sbxm list` shows `changed` when the current config no longer matches, with the command to fix it.
- `sbxm open` refuses a changed sandbox instead of attaching to something built from an old config.
- `sbxm open <project> --rebuild` recreates it. The workspace is kept, but the agent's **session history in that
  sandbox is lost**. The old sandbox is removed only after the new kits validate.
- `sbxm config show <project> --harness <h>` prints exactly what's hashed, so you can see what changed.

## Security model

- Egress is deny-by-default and enforced by `sbx`'s proxy; only `network.allow` hosts (plus what the agent's kit
  needs) are reachable. `sbx policy log <sandbox>` shows what was allowed and blocked.
- Config only *names* secrets. Values stay in `sbx`, which injects them at the proxy.
- sbxm's metadata sits outside the mounted workspace, so an agent can't widen its own next sandbox.
- sbxm never changes `sbx`'s settings, secrets, skills store or policies. When one of them blocks something, sbxm
  tells you the command to run.
- Deleting files needs `rm --purge` plus a confirmation showing the exact paths; links and junctions are refused.

## Troubleshooting

- **Start with `sbxm doctor`.** Each failure says what's wrong and how to fix it.
- **`sbx create` fails with `failed to run sandbox container`:** check the workspace isn't on a drive `sbx` can't
  mount (on the development machine, anything on `C:`), or under `%TEMP%`/`AppData`.
- **A host is blocked:** add it to `network.allow`, then `sbxm open <project> --rebuild`.
- **`secret '…' (secrets.services) is not stored in sbx`:** `sbx secret set <service>`, or `sbx setup` to import it
  from your environment.
- **Pi answers `401`:** approve the credential binding (see *Harnesses*).
- **`sandbox … exists but sbxm has no state for it`:** it wasn't created by sbxm here; remove it with `sbx rm` (this
  deletes its session history) and run `sbxm open` again.

## Development

`just check` runs formatting, lints and the tests (no Docker needed); `just real-test` runs the tests against the
real `sbx`. Design decisions are numbered in `decisions.md`, the milestone plan is `milestone-1.md`, and
`AGENTS.md` describes the code layout and workflow for coding agents.

### Working on issues with sbxm sandboxes

`scripts/issue-workers.ps1` (PowerShell 7) hands open GitHub issues to Claude Code agents running in parallel, one
sbxm sandbox per issue. The host picks the issues, so no two workers get the same one. It skips questions, issues
whose **Depends on** issues are still open, and issues **Related** to one that already has a worker. Each worker is a
clone at `<base_dir>\sbxm-issue-<n>` on branch `issue-<n>`, and the agent follows the `sdlc-implementation` skill from
the clone's `.claude/skills/`. The sandbox has no GitHub access: the agent commits locally and writes
`.sbxm-issue/result.md` with evidence for each acceptance criterion, and you push from the host.

A worker never reviews its own change. `review` first runs `cargo fmt --check`, clippy and `cargo test` on the host,
then a fresh reviewer in its own sandbox (`sbxm-review-<n>`, on its own clone, so it can't change the branch)
writes `review.md`. The reviewer is Codex with `gpt-5.6-sol` at high reasoning effort, so it doesn't share the Claude
workers' blind spots; `-ReviewHarness claude` and `-ReviewModel <model>` change that. If the review has must-fix
findings, the worker gets one round to fix them and the review runs once more. Whatever is still open goes into the
PR description; nothing is filed as an issue.

Every change to sbxm goes through a PR, including ones not made by workers, and gets the same review:
`review -Pr <n>` clones the PR's branch, runs the host checks and the reviewer, and posts the review as a comment on
the PR. There's no fix round; the author fixes the findings and runs it again. PRs from forks are refused, because
the host checks run the PR's code on your machine.

One-time setup: copy `profiles/sbxm-dev` into your `profiles_dir`. It installs Rust and a C toolchain, allows
crates.io, and needs the `anthropic` secret; the Codex reviewer also needs the `openai` one (`sbx secret ls`). Also
check that `gh auth status` shows you logged in.

```powershell
./scripts/issue-workers.ps1 start -Workers 2 -DryRun   # which issues would be picked
./scripts/issue-workers.ps1 start -Workers 2           # or choose them: -Issue 1,2
./scripts/issue-workers.ps1 status                     # agent running/finished, commits, result.md, review
./scripts/issue-workers.ps1 review -Issue 1            # host checks, independent review, one fix round
./scripts/issue-workers.ps1 review -Pr 12              # review any open PR and comment the result on it
./scripts/issue-workers.ps1 finish -Issue 1            # push issue-1 and open a PR with "Fixes #1" and the review
./scripts/issue-workers.ps1 remove -Issue 1            # after merging: sandbox and clone (asks first)
```

Agents run for up to `-TimeLimit` (default `2h`), reviewers for up to `-ReviewTimeLimit` (default `45m`). For a
worker, the output goes to `.sbxm-issue/` in its clone (`agent.log`, `gates.log`, `review-<round>.log` and `.md`,
`review.md`, `fix.log`); for `review -Pr <n>`, to `<base_dir>\sbxm-pr-<n>-review\` (`gates.log`, `review-1.log`,
`review.md`). Both keep the reviewer's full session transcripts in `transcripts\`, copied out before its sandbox is
removed.
To take over one interactively, run `sbxm open sbxm-issue-<n>`. `-BaseDir` (default `E:\sbxm-projects`) must match
`base_dir` in `config.toml`. [`sandbox-issues.md`](sandbox-issues.md) has the steps with the expected output.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your
option.
