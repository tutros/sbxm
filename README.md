# sbxm

`sbxm` creates and manages per-project Docker Sandboxes (`sbx`) from one
shared, versioned config. Each project gets a folder on your machine, and each coding agent (Claude Code, Codex,
Gemini CLI, Pi or Antigravity) gets its own sandbox for it, built with your network allowlist, environment, secrets, instructions
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
   sbxm open demo --harness claude
   ```
   The workspace is `<base_dir>\demo`, mounted read-write into the sandbox.

## Commands

| Command | What it does |
|---|---|
| `sbxm config init` | Writes a starter `config.toml` and `default` profile. Refuses to overwrite either. |
| `sbxm new <project> [--harness h] [--profile p] [--seed dir]` | Creates `<base_dir>/<project>` if missing (or reuses it), builds the kits from the profile and the project's `sandbox.toml`, checks them with `sbx kit validate`, and creates the sandbox. Doesn't attach. Refuses a harness that already has a sandbox for the project, before writing anything; open that one with `sbxm open <project> --harness h` (or `--rebuild` it). `--seed` copies a folder into a *new* project. |
| `sbxm open <project> --harness h [--rebuild]` | Attaches to the sandbox, starting it if it's stopped and creating it if it's missing. Refuses if the config changed since the sandbox was built; `--rebuild` recreates it. |
| `sbxm list [--json]` | Lists sbxm's sandboxes with project, harness, status and whether their config is `current` or `changed`. Flags orphans (a sandbox sbxm has no record of, or a record without a sandbox) and says how to fix each. |
| `sbxm stop <project> --harness h` | Stops the sandbox. |
| `sbxm rm <project> --harness h` | Removes the sandbox and sbxm's record of it. The workspace is kept. |
| `sbxm rm <project> --purge [--yes]` | Removes **every** sandbox of the project, then deletes the workspace and its metadata, after you confirm the exact paths. Without a terminal it needs `--yes`. Can't be combined with `--harness`. |
| `sbxm run init [path]` | Writes a starter run-config for a comparison (default `./run.toml`): two contestants (Claude and Codex) live, an Antigravity one commented out, and commented examples for checks, a rubric and a judge. The file is valid as written. Refuses to overwrite. |
| `sbxm run <config>` | Runs a comparison: checks the run-config and that every needed secret is stored in `sbx` (each contestant's provider, the judge's, and the profiles' `secrets.services`), builds and validates the kits, then gives each (contestant, repeat) pair its own throwaway sandbox. A contestant can set its own `profile`; otherwise `[run].profile` (or your `default_profile`) applies, and the judge uses the run's. Contestants of a repeat run in parallel; repeats run one after another. Each sandbox is removed afterwards, even after an error; the workspaces stay under `<base_dir>/runs/<run-id>/<contestant>/<repeat>/`. Prints the run ID and one line per pair (completed, timed out or failed). With `task.seed`, each contestant gets its own copy of the seed as a fresh git repo with one baseline commit (the seed's history and remotes are dropped); without it, an empty plain folder. Each pair's diff (everything the contestant changed or added, even if it committed) is captured before its sandbox is removed. Each pair's results are saved the moment it finishes under `<base_dir>\.sbxm\runs\<run-id>\<contestant>\<repeat>\` (`answer.md`, `diff.patch`, `transcript.jsonl`, `result.json`), next to `run.json` (the run's identity: config hashes, profile, `sbx` version, times) and a copy of the run-config; `run.json` gets `completed_at` only once everything is saved. Any `[[eval.checks]]` run inside each contestant's sandbox after its agent finishes (and after its diff is taken), as `sh -c <command>` with a time limit enforced inside the sandbox; exit code 0 passes. The verdicts are saved in `evals.json` next to the pair's other files and summarised on the pair's line (`; checks 2/3 passed`). With `[eval.judge]` and a rubric, an LLM judge then scores the contestants: after all pairs are done, once per repeat, in its own throwaway sandbox, it sees each contestant's answer and diff under an anonymous label (A, B, ...) and scores every rubric criterion (`pass_fail` or a `scale` of levels). A judge from the same provider as a contestant is allowed, with a warning. Verdicts are saved in each pair's `evals.json` and per repeat in `judge/<repeat>/judge.json` (the label mapping); a judge that fails is warned about and doesn't fail the run. The contestants are then ranked: each criterion is 0 to 1, a contestant's score is the weighted mean over the criteria the judge scored (criteria it skipped are listed, never counted as 0), repeats are averaged, equal scores share a rank, and executable checks are shown alongside without changing the score. The ranking is printed before the last line, `Results: <folder>`. Cosine evaluation is planned but not implemented yet, so `[eval.cosine]` is currently refused. |
| `sbxm run show <run-id> [--diff]` | Prints a saved run from its files: when it started and completed, the profile and `sbx` version, and for each contestant and repeat the status, the answer, a summary of the diff (files changed, lines added and removed) each executable check's verdict and the judge's scores, with the anonymous label each contestant was judged under. `--diff` also prints every full patch. With a judge it also prints the ranking, recomputed from the saved files (edit `run-config.toml` in the run folder to try other weights). A run that is still going or was interrupted shows what has been saved so far. Only reads files; the id must look like `2026-09-30-a1b2c3`. |
| `sbxm task init [path]` | Writes a starter `sbxm-task.toml` (the worker, reviewer, sandbox profile and gates for `sbxm task`) in the repo's root. Valid as written for a Rust repo. Refuses to overwrite. |
| `sbxm task start (--issue N... \| --workers N) [flags]` | Hands GitHub issues to worker agents (the port of `scripts/issue-workers.ps1`). For each chosen issue it makes a task: its own sandbox (`sbxm-task-issue-<n>-<harness>`), a clone of the repo on branch `issue-<n>`, and a headless worker that follows the issue and commits. `--workers N` picks up to N open issues (`must-fix` before `should-fix`; it skips questions, issues whose `**Depends on:**` issues are still open, issues that already have a task and issues `**Related:**` to one that does); `--issue N` starts exactly the issues named (one that is blocked by an open issue is still refused). The tasks run in parallel and one failing doesn't stop the others. The worker has no GitHub access: the issue text is copied into its clone. When it stops, its commits are collected into a bare repo that only sbxm and your git touch (`<base_dir>\.sbxm\tasks\<id>\repo.git`) through a verified `git bundle`: the bundle must be a plain file under 500 MB, only the task branch is fetched, and no git command ever runs inside the agent's folder. Also saved there: `issue.md`, `result.md` (what the worker wrote), the transcript and `task.json`. Flags override `sbxm-task.toml`: `--worker-harness`, `--worker-model`, `--time-limit`, `--profile`, `--base`, `--repo`. `--restart` (with `--issue`) first deletes an existing task of that issue, showing what and asking like `task rm` (`--yes` skips the question), after checking that the issue is still open, the task isn't running and every other input is valid. After the worker (unless it failed) the task's gates run (see `task gates`); then `task review` and `task finish` (or `task run` for start and review in one go). Exits non-zero if any task or its gates failed. |
| `sbxm task review (--issue N \| --pr N) [--repo owner/name] [--base b] [--reviewer-harness h] [--reviewer-model m] [--reviewer-time-limit 45m] [--time-limit 2h] [--profile p]` | Has a task's change reviewed by an independent agent, then lets the worker fix what it found, once. Checks first, before anything is created: the task is ready for review, and the reviewer's provider secret (and the profile's) are stored. The gates run first unless they passed since the last change (a failure stops here, exit 1). The reviewer runs in its own sandbox over its own clone of the task branch (made from the host-owned repo, so it sees exactly the collected commits and can't touch the worker's folder), told to follow the repo's `sdlc-code-review` skill, to change nothing and to write `.sbxm-task/review.md` whose first line is `Must-fix findings: <count>`; Codex runs with high reasoning effort. The review is saved under the reviewer's name (`Reviewer: codex (model)`) as `review-<round>.md` and `review.md`; a review without that first line is kept for reading but not used. If it counts must-fix findings, the worker gets one fix round in its own sandbox (the review and a fix prompt are put in its folder; its new commits are collected like the first time), the gates run again (a failure stops here, exit 1), and a second review (`review-2.md`) follows with the first one as context. The reviewer's sandbox and clone are removed after each round, also after an error. The task ends `ready`; must-fix findings left after the second review are reported, not an error. A reviewer like the worker's harness warns (less independent). For an issue's task nothing is pushed or posted. **`--pr N` instead reviews an open pull request from a branch of this repo** (a fork's PR is refused, as is a closed or merged one, before anything is created): the PR's head is fetched into the host-owned repo (with git's object check on), the PR's description and the issues it closes are the reviewer's context, and the gates run on a clean checkout: the sandbox tier inside the reviewer's own sandbox (a PR has no worker), the host tier as usual. The reviewer runs once, with no fix round, and a valid review is posted as a comment on the PR (`@mentions` are neutralised and a very long review is cut with a note; the whole text is `review.md` in the task folder). Nothing is posted if the gates fail or the review is invalid; if only the posting fails, the review stays saved and the error gives the `gh pr comment` command to post it by hand. A PR task that exists is retried only when its review failed; to review a PR again after it changed, remove its task first with `sbxm task rm --pr N`. `--repo` and `--base` apply to PRs (defaults: this checkout's origin, the repo's default branch). |
| `sbxm task gates --issue N [--tier sandbox\|host\|all] [--dry-run]` | Runs a task's gates, the checks `sbxm` itself runs (an agent's prompt can't skip them) to decide whether its work may go on. They are listed in `sbxm-task.toml` under `[gates]`. The **sandbox tier** (`sandbox = [...]`, default for a Rust repo: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`) runs each command as `sh -c` in the worker's own sandbox, on its folder, under a time limit enforced inside the sandbox (`timeout = "20m"` per command); the first failure stops the tier. The optional **host tier** (`host = [...]`, off while empty) runs on this machine, in a clean checkout of the task branch from the host-owned repo (only committed files, no hooks), only after the sandbox tier passed, and the checkout is removed afterwards. **Host gates run agent-written code on your machine, outside any sandbox**: list only commands you would run on a stranger's pull request. Results go to the task's `task.json` and `gates.log`; a failure marks the task `gates-failed`. `task start` runs them after the worker; this command runs them again on demand, for example after you fixed something by hand in the worker's clone: first it collects what is *committed* there into the task's repo, so the host tier, `task review` and `task finish` use the same commits as the sandbox tier (commit your fix; uncommitted changes are seen by the sandbox tier but are never collected). `--dry-run` prints each tier's commands, where they run and whether the tier is off, and changes nothing. A run of one tier is partial: `task review` skips its own gate run only if the passing runs together covered every configured tier on the task branch's current commit, and `--tier host` is refused unless the sandbox tier passed on that commit. Refused while the worker is running or after the task has moved past its gates. |
| `sbxm task status [--issue N \| --pr N] [--json]` | One line per task: stage, status (`interrupted` when its `sbxm` process is gone), `ahead:` the commits on the task branch past its base in the task's own repo (`-` before the repo exists), and whether `result.md` and `review.md` exist. `--json` prints the records, plus `interrupted` and `commits_ahead` (`null` when unknown). |
| `sbxm task file-findings (--issue N \| --pr N \| --file F) [--create] [--only ID,...] [--standard-criteria] [--keep-paths] [--repo owner/name]` | Files the findings of a review as GitHub issues, one per finding (the port of `scripts/file-review-issues.ps1`). `--file` takes any review file (e.g. `sdlc/reviews/<date>-<scope>.md`); `--issue`/`--pr` take the `review.md` of that task (the built-in reviewer prompts ask for the skill's review format, with an `Issues:` line; a custom `[prompts] reviewer` must do the same). **A dry run unless `--create`**: it prints every issue it would create and the `Issues:` line it would write, and changes nothing. Before any write it checks the finding ids, that the review has exactly one `Issues:` line, your `gh` login, the repo and its labels (`must-fix`, `should-fix`, `question` must exist; none are created), and every finding: a secret-looking line (token, key, password assignment) refuses the whole run, naming the finding and line but never the value, and personal paths (`C:\Users\<name>`, `/home/<name>`) become `~` (`--keep-paths` keeps them). A must-fix or should-fix finding with no acceptance criteria is refused unless `--standard-criteria`. Issues are created in dependency order with permalinks at the reviewed commit and `Depends on`/`Related` links to the other issues; a hidden marker in each body means a rerun never files a finding twice (it skips it and reuses its number), and the links of issues that already exist are patched in those two fields only. Afterwards the review's `Issues:` line is rewritten (`Issues: S-1 #41, S-2 pending`). Exits non-zero if some issue could not be filed; rerun to file the rest. |
| `sbxm task finish --issue N` | Publishes a ready task: pushes branch `issue-<n>` from the task's host-owned `repo.git` (never forced) and opens the PR from it to the task's base, titled like the issue, with `Fixes #<n>`, then `result.md` and `review.md` each under a heading (a file over 25,000 characters is cut, and the body and the output say so). Every check comes before the push: the task must be `ready` and have no PR yet, the repo, branch and base names must be usable, the branch must have commits beyond the base, and `result.md`/`review.md` must not contain a secret-looking line (refused, naming the line, never the value). The task then becomes `finished` and `task.json` keeps the PR's URL. If the push worked but opening the PR failed, the task stays `ready` with a note and the error says so: run `finish` again, which pushes the same commits (nothing to do) and retries only the PR. If someone opened the PR by hand in between, GitHub's refusal is shown and nothing is recorded. **It refuses to push a branch that adds, edits or deletes anything under `.github/workflows/` or `.github/actions/`** (letter case ignored): GitHub would run those files with the repository's secrets the moment the branch is pushed, before anyone has read what the agent wrote, so read them in the task's `repo.git` and push the branch yourself if the change is meant. |
| `sbxm task rm (--issue N \| --pr N) [--yes]` | Deletes a task: its sandboxes (the worker's and the reviewer's, by the names saved in `task.json`), its clones (`<base_dir>\tasks\<id>`, `<id>-review`, `<id>-gates`) and its folder `<base_dir>\.sbxm\tasks\<id>\`. It first prints the exact sandboxes and paths and asks `[y/N]`; without a terminal it refuses unless you pass `--yes`. It refuses while the task is running (an interrupted one can go), and builds paths only from an id that looks like `issue-41` or `pr-7`. It never follows a link or junction: a task folder that is one, or that resolves anywhere but its own place under the base folder, stays (a link *inside* a clone is removed, not followed). Removal carries on after a failure and says what went and what stayed; the task folder is deleted last and only if everything else went, so a second `rm` still knows the sandbox names. Exits non-zero if anything stayed. |
| `sbxm task run --issue N [flags]` | `task start` and then `task review` for one issue, with the flags of both (`--worker-harness`, `--worker-model`, `--reviewer-harness`, `--reviewer-model`, `--time-limit`, `--reviewer-time-limit`, `--profile`, `--base`, `--repo`, `--restart`, `--yes`). The reviewer's flags are checked before the worker starts. It stops at `ready` and prints the `task finish` command: publishing stays a separate step. Exit 1 for any failure, including failing gates; a task that ends `ready` with must-fix findings left is reported, not an error. |
| `sbxm config show [project] [--profile p] [--harness h] [--kits]` | Prints the merged config exactly as it's hashed, the hash, and with `--kits` the generated kits. Creates nothing. |
| `sbxm config profiles-dir` | Prints the folder profiles are read from (`profiles_dir`, else `<config dir>/profiles`), for scripts such as `just deploy-profiles`. Loads only `config.toml`, so it fails like any command on an invalid or missing one. |
| `sbxm doctor` | Checks `sbx` (on `PATH`, new enough, daemon answering), the config, every profile, every project with each of its sandboxes (secrets stored, kits valid), and the base dir (exists, writable, not a temp folder, at least 10 GiB free). Exits non-zero if anything fails. |

`--harness` defaults to `claude` for `new` and `config show`; `open`, `stop` and `rm` (without `--purge`) require it, so they never act on the wrong sandbox by default (issue #33). `sbxm <command> --help` shows every option.

### The `justfile`

`just` lists the recipes. Recipes run sbxm through `cargo run`, so they always use the current code, and the
`justfile` uses PowerShell 7 (`pwsh`) as its shell on every platform.

**Setting up**

| Recipe | What it does |
|---|---|
| `just deploy-profiles` | Copies the repo's `profiles/*` into the `profiles_dir` sbxm reads, asking `sbxm config profiles-dir` (so an invalid or missing `config.toml` stops it before anything is written). Each repo profile replaces its copy there; profiles that exist only in `profiles_dir` are left alone. It prints `new`, `updated` or `unchanged` per profile. Run it after `git pull` changes a profile; sandboxes built from the old one show config drift until you rebuild them. |
| `just init` | Writes a starter config and `default` profile (refuses to overwrite). |
| `just doctor` | Checks `sbx`, the config, every profile and project, and the base dir. |
| `just install` | Installs `sbxm` on your PATH from this checkout. |

**Using sbxm** (harness defaults to `claude`, as in the recipes' arguments)

| Recipe | What it does |
|---|---|
| `just new <project> [harness] [profile]` | Creates a project and its sandbox, e.g. `just new demo codex`. |
| `just seed <project> <dir> [harness]` | Creates a project from a seed folder. |
| `just open <project> [harness]` | Attaches to the sandbox, creating it if needed. |
| `just rebuild <project> [harness]` | Recreates the sandbox from the current config (session history is lost; the workspace is kept). |
| `just stop <project> [harness]` | Stops the sandbox. |
| `just rm <project> [harness]` | Removes the sandbox and state; the workspace is kept. |
| `just purge <project>` | Removes every sandbox of the project and deletes its workspace (asks first). |
| `just list` | Lists sandboxes with status, config drift and orphans. |
| `just show [project] [harness]` | Prints the merged config, its hash and the kits, without creating anything. |
| `just demo [project]` | Creates one project with a sandbox for every harness, then lists them. |

**Developing sbxm**

| Recipe | What it does |
|---|---|
| `just build` | Builds the debug binary. |
| `just test` | Runs the Rust tests (no Docker needed). |
| `just script-test` | Runs the Pester tests for the scripts in `scripts/` (needs Pester 5). |
| `just check` | Everything that must pass before a commit: `cargo fmt --check`, clippy, `cargo test` and `just script-test`. |
| `just real-test [base_dir]` | Runs the tests against the real `sbx` (needs `sbx login` and a base dir not on `C:`). |

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
| `antigravity` | Antigravity (`agy`) | `~/.gemini/AGENTS.md` | Uses the Antigravity kit from Docker Hub, pinned to a fixed tag. With a `google` secret stored (`sbx secret set -g google`), `agy` uses it as a Gemini API key and needs no sign-in (decision 137). `sbx`'s skills store isn't known to serve it: sbxm warns unless `skills.store = "off"`. |
| `pi` | Pi | `~/.pi/agent/AGENTS.md` | Uses the Pi kit from Docker Hub, pinned to a fixed tag. See below. `sbx`'s skills store doesn't serve Pi either: sbxm warns unless `skills.store = "off"`. |

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
- Every file or folder path must be relative and stay inside the profile's folder, and must exist. No symlink or
  junction may sit on the way to it, whether the file itself or a folder above it: `instructions.mandatory`,
  `instructions.reference`, `home_files` and `managed_settings` are all checked, and the same applies when these
  keys are set in a project's `sandbox.toml`.
- `skills.store = "readwrite"` is refused: an agent could plant skills that every other sandbox then loads.
- `home_files` must name a subfolder of the profile's (or project's) folder; it can't resolve to that folder
  itself (e.g. `"."` or `""`).
- `home_files` may not contain links anywhere in its tree (in addition to the check above on the way to the
  folder itself), `.claude/settings.json` (the Claude kit replaces it; use `managed_settings`), or
  `.claude/CLAUDE.md` while `instructions.mandatory` is set.
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
- `sbxm open <project> --harness h --rebuild` recreates it. The workspace is kept, but the agent's **session history in that
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

## Checking on a sandbox

**Which sandboxes exist and are they running?**

| Command | Shows |
|---|---|
| `sbxm list` | sbxm's sandboxes with status, config drift and orphans (`--json` for scripts). |
| `sbx ls` | Every sandbox `sbx` knows about, with agent, `running`/`stopped` and workspace. A sandbox that has gone from this list is finished and removed. |
| `./scripts/issue-workers.ps1 status` | Issue workers: agent running or finished, commits, `result.md` and `review.md`. |

Sandbox names are `sbxm-<project>-<harness>`, so the scripted ones follow from their project names: a worker is
`sbxm-sbxm-issue-<n>-claude`, a PR reviewer `sbxm-sbxm-review-pr-<n>-codex`.

**What is it doing right now?**

- `sbx exec <sandbox> bash -c 'ps aux'` runs a command inside it (this starts a stopped sandbox). Attach to the agent
  with `sbx run --name <sandbox>`.
- `sbx policy log <sandbox>` lists the hosts the sandbox reached and the ones the proxy blocked. A host under
  *Blocked requests* needs adding to `network.allow` (see *Troubleshooting*).
- Inside the sandbox, `/var/log/sbx-kit-startup.log` has the kit's startup commands. The profile's `setup.install`
  steps run when the sandbox is created and print as `✓`/`✗` lines; the `sbxm-dev` profile's take about two and a half
  minutes (mostly `just`, which is compiled).
- `sbxm config show <project> --harness <h> --kits` prints exactly what was applied, and `sbxm config profiles-dir`
  prints the folder profiles are read from.

**A scripted run (worker or `review -Pr`): which file changes when**

A worker's files are in `.sbxm-issue\` in its clone; a PR review's are in `<base_dir>\sbxm-pr-<n>-review\`. Follow one
live with `Get-Content <file> -Tail 20 -Wait`.

| File | Written while | Meaning |
|---|---|---|
| `gates.log` | the host checks run (first) | `cargo fmt`, clippy and `cargo test` on the host. A failure here stops the run before any sandbox starts. |
| `review-1.log` | the reviewer runs | The reviewer's live log. It stays old until the reviewer's sandbox has started. |
| `review.md`, `review-1.md` | the end | The review itself. Until then they are the **previous** run's files, so check their times. |
| `agent.log` | a worker runs | The worker's own output; it ends with `agent exit code: <n>`. |
| `transcripts\` | the end | The reviewer's session transcripts, copied out before its sandbox is removed. |

A review normally takes 6 to 12 minutes after its sandbox has started. The command's own output says which stage it is
in (`cargo fmt`, `cargo test`, `review round 1`).

**Cleaning up.** Each sandbox that builds the project keeps its own `target\`, which is 3 to 4 GB. `sbxm rm <project>
--purge` removes every sandbox of a project and its workspace (asks first); `issue-workers.ps1 remove -Issue <n>` does it
for a worker. Finished review folders are small and can stay or go. Check `sbx ls` for stopped leftovers.

## Troubleshooting

- **Start with `sbxm doctor`.** Each failure says what's wrong and how to fix it.
- **`sbx create` fails with `failed to run sandbox container`:** check the workspace isn't on a drive `sbx` can't
  mount (on the development machine, anything on `C:`), or under `%TEMP%`/`AppData`.
- **A host is blocked:** add it to `network.allow`, then `sbxm open <project> --harness h --rebuild`.
- **`secret '…' (secrets.services) is not stored in sbx`:** `sbx secret set <service>`, or `sbx setup` to import it
  from your environment.
- **Pi answers `401`:** approve the credential binding (see *Harnesses*).
- **`sandbox … exists but sbxm has no state for it`:** it wasn't created by sbxm here; remove it with `sbx rm` (this
  deletes its session history) and run `sbxm open` again.
- **A profile change isn't taking effect:** sbxm reads the copy in `profiles_dir`, not the one in the repo. Run
  `just deploy-profiles` (it refuses to write if `config.toml` is invalid), then rebuild the sandbox.
- **`git` fails with `Permission denied` on Windows (`.git/config`, `.git/objects/…`):** antivirus or the file indexer
  briefly holds a file git just created. It passes by itself, so run the command again.
- **`sbx exec`/`sbx run` print `context deadline exceeded` after the sandbox was created:** the sandbox exists; only
  the attach step timed out in a non-interactive shell. Check `sbx ls`.

## Development

`just check` runs formatting, lints, the Rust tests and the script (Pester) tests, with no Docker needed;
`just real-test` runs the tests against the real `sbx`. See [The `justfile`](#the-justfile) for every recipe. Design decisions are numbered in `sdlc/decisions.md`, the milestone plan is `sdlc/milestone-1.md`, and
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

One-time setup: run `just deploy-profiles` to copy `profiles/sbxm-dev` into your `profiles_dir` (run it again after
the profile changes). It installs Rust, a C toolchain, PowerShell 7,
Pester 5 and `just` (so `just script-test` runs in the sandbox), allows crates.io, Microsoft's package host and the
PowerShell Gallery, and needs the `anthropic` secret; the Codex reviewer also needs the `openai` one (`sbx secret ls`). Also
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
To take over one interactively, run `sbxm open sbxm-issue-<n> --harness claude`. `-BaseDir` (default `E:\sbxm-projects`) must match
`base_dir` in `config.toml`. [`sandbox-issues.md`](sandbox-issues.md) has the steps with the expected output.

### Filing review findings as issues

Reviews run in sandboxes without GitHub access, so their findings sit in `sdlc/reviews/<date>-<scope>.md` marked
`Issues: pending`. `scripts/file-review-issues.ps1` (PowerShell 7, run from a host where `gh` works) files one issue
per finding. It is a dry run unless you pass `-Create`, so read the dry run first: it prints every issue body, and
path and secret scrubbing is heuristic (personal paths become `~`; a secret-looking line refuses the finding).

```powershell
./scripts/file-review-issues.ps1 sdlc/reviews/2026-09-30-milestone-2a.md                     # dry run
./scripts/file-review-issues.ps1 sdlc/reviews/2026-09-30-milestone-2a.md -Create             # file the issues
./scripts/file-review-issues.ps1 sdlc/reviews/2026-09-30-milestone-2a.md -Create -Only M2A-1 # just these finding ids
```

`-StandardCriteria` files a finding that has no acceptance criteria with only the standard ones, `-KeepPaths` keeps
personal paths as they are, and `-Repo <owner/name>` overrides the repo taken from `origin`. A rerun never
duplicates issues (a hidden marker in each body); it also fills in the `#n` links of issues filed earlier. Afterwards
the review's `Issues:` line is rewritten to `Issues: <id> #<n>, ...`. Decision 134 has the details.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your
option.
